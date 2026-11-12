package dev.heka.ghost.call.link

import android.util.Log
import okhttp3.Call
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlin.random.Random

/** What the connection is doing, as the app and the status notification say it. */
enum class Link(val word: String) {
  OFF("off"),
  UNPAIRED("unpaired"),
  CONNECTING("connecting"),
  CONNECTED("connected"),
  RECONNECTING("reconnecting"),
  /** No address answered; trying again with backoff. */
  OFFLINE("offline"),
  /** The desk answered and refused this phone (401): it was unpaired at the desk, or the phone's clock is off. */
  REFUSED("refused"),
}

/**
 * The signed live stream (docs/phone-protocol.md, "The live stream"), held on its own thread.
 * It tries the desk's addresses in order (home, then the tunnel), keeps the first that opens,
 * reads Server-Sent Events off it, checks every message's signature over this request's nonce
 * before it is parsed, and hands each to the hub. A stream that goes quiet for 35 seconds (two
 * heartbeats and a margin) is dead: OkHttp's read timeout closes it. Then it backs off 1, 2, 4
 * … 30 seconds with jitter and tries again, home first, from its cursor. A network change cuts
 * the wait short (`nudge`).
 */
class LinkClient(private val hub: LinkHub) : Thread("ghost-link") {
  @Volatile private var running = true
  @Volatile private var call: Call? = null
  private val wake = Object()
  @Volatile private var nudged = false

  fun shutdown() {
    running = false
    nudge()
  }

  /** Drop the stream now and open it again without waiting out the backoff. */
  fun nudge() {
    nudged = true
    call?.cancel()
    synchronized(wake) { wake.notifyAll() }
  }

  private sealed class Outcome {
    /** Nothing answered at this address: try the next. */
    object Unreachable : Outcome()
    /** The stream was open and ended. */
    data class Dropped(val heard: Boolean) : Outcome()
    data class Refused(val status: Int) : Outcome()
  }

  override fun run() {
    var attempt = 0
    while (running) {
      nudged = false
      val cfg = hub.config()
      if (cfg == null) {
        hub.setLink(Link.UNPAIRED, null)
        pause(60_000)
        continue
      }
      var outcome: Outcome = Outcome.Unreachable
      for (address in cfg.addresses) {
        if (!running) break
        outcome = streamOnce(cfg, address)
        if (outcome != Outcome.Unreachable) break
      }
      if (!running) break
      when (val o = outcome) {
        is Outcome.Dropped -> {
          if (o.heard) attempt = 0
          hub.setLink(Link.RECONNECTING, null)
        }
        is Outcome.Refused -> {
          hub.setLink(if (o.status == 401) Link.REFUSED else Link.RECONNECTING, null)
          if (o.status == 401) attempt = 5 // straight to the longest wait: it won't fix itself fast
        }
        Outcome.Unreachable -> hub.setLink(Link.OFFLINE, null)
      }
      if (nudged) {
        attempt = 0
        continue
      }
      pause(Backoff.delayMs(attempt, Random.Default))
      attempt = (attempt + 1).coerceAtMost(8)
    }
    Log.i(TAG, "stopped")
  }

  private fun pause(ms: Long) {
    synchronized(wake) {
      if (!nudged && running) wake.wait(ms)
    }
  }

  private fun streamOnce(cfg: LinkConfig, address: String): Outcome {
    val path = hub.streamPath()
    val nonce = Sign.nonce()
    val req = Request.Builder()
      .url("http://$address$path")
      .header("Accept", "text/event-stream")
      .apply { Sign.headers(cfg.deviceId, cfg.secret, "GET", path, ByteArray(0), System.currentTimeMillis() / 1000, nonce).forEach { (k, v) -> header(k, v) } }
      .build()
    if (hub.link != Link.CONNECTED) hub.setLink(Link.CONNECTING, null)
    val c = STREAM.newCall(req)
    call = c
    var open = false
    var heard = false
    try {
      c.execute().use { resp ->
        if (resp.code != 200) {
          val body = resp.body?.bytes() ?: ByteArray(0)
          val signed = Sign.verifyResponse(cfg.secret, resp.code, nonce, resp.header("X-Ghost-Time"), resp.header("X-Ghost-Signature"), body)
          // An unsigned answer may not be the desk at all; a 401 is believed either way,
          // since a desk that forgot this phone has no secret to sign it with.
          if (!signed && resp.code != 401) return Outcome.Unreachable
          Log.i(TAG, "the desk answered ${resp.code} at $address")
          return Outcome.Refused(resp.code)
        }
        if (!Sign.verifyResponse(cfg.secret, 200, nonce, resp.header("X-Ghost-Time"), resp.header("X-Ghost-Signature"), ByteArray(0))) {
          Log.w(TAG, "a stream at $address that isn't signed by the desk; not reading it")
          return Outcome.Unreachable
        }
        open = true
        hub.setLink(Link.CONNECTED, address)
        val source = resp.body?.source() ?: return Outcome.Dropped(false)
        val parser = SseParser()
        while (running) {
          val line = source.readUtf8Line() ?: break
          val ev = parser.line(line) ?: continue
          when (val d = decode(ev.data, cfg.secret, nonce)) {
            Decoded.BadSignature -> {
              Log.w(TAG, "a message that doesn't verify; opening the stream again")
              break
            }
            Decoded.Unreadable -> hub.heard()
            is Decoded.Ok -> {
              heard = true
              hub.onMessage(d.msg)
            }
          }
        }
      }
    } catch (e: IOException) {
      if (!open) {
        Log.i(TAG, "no answer at $address: ${e.javaClass.simpleName}")
        return Outcome.Unreachable
      }
    } finally {
      call = null
    }
    return Outcome.Dropped(heard)
  }

  companion object {
    private const val TAG = "GhostLink"
    /** Two heartbeats (15 s each) and a margin: a stream quiet this long is dead. */
    const val DEAD_SECS = 35L

    val STREAM: OkHttpClient = OkHttpClient.Builder()
      .connectTimeout(4, TimeUnit.SECONDS)
      .readTimeout(DEAD_SECS, TimeUnit.SECONDS)
      .writeTimeout(10, TimeUnit.SECONDS)
      .retryOnConnectionFailure(false)
      .build()

    private val ACTIONS: OkHttpClient = STREAM.newBuilder().readTimeout(10, TimeUnit.SECONDS).build()
    private val JSON = "application/json".toMediaType()

    /** One signed POST to one address; Answered only with a reply the desk signed. */
    fun post(cfg: LinkConfig, address: String, action: Action): Tried {
      val bytes = action.body.toByteArray(Charsets.UTF_8)
      val nonce = Sign.nonce()
      val req = Request.Builder()
        .url("http://$address${action.path}")
        .post(bytes.toRequestBody(JSON))
        .apply { Sign.headers(cfg.deviceId, cfg.secret, "POST", action.path, bytes, System.currentTimeMillis() / 1000, nonce).forEach { (k, v) -> header(k, v) } }
        .build()
      return try {
        ACTIONS.newCall(req).execute().use { resp ->
          val body = resp.body?.bytes() ?: ByteArray(0)
          if (!Sign.verifyResponse(cfg.secret, resp.code, nonce, resp.header("X-Ghost-Time"), resp.header("X-Ghost-Signature"), body)) {
            // Unsigned is unverified, a 401 included (docs/phone-protocol.md): tried again.
            Tried.Failed
          } else {
            val json = try {
              if (body.isEmpty()) null else JSONObject(String(body, Charsets.UTF_8))
            } catch (e: Exception) {
              null
            }
            Tried.Answered(resp.code, json)
          }
        }
      } catch (e: IOException) {
        Tried.Failed
      }
    }
  }
}
