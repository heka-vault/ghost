package dev.heka.ghost.call.link

import org.json.JSONObject
import java.util.UUID
import kotlin.math.min
import kotlin.random.Random

/**
 * One thing the operator did, as the signed `POST` that tells the desk. The body is built once,
 * with its `id` (docs/phone-protocol.md: the desk answers the same `id` the same way for ten
 * minutes and does nothing twice), and every retry sends exactly these bytes.
 */
data class Action(
  val kind: String,
  /** The waiting-list key it is about, when it is about one. */
  val key: String?,
  val path: String,
  val body: String,
  val id: String?,
) {
  companion object {
    private fun newId() = UUID.randomUUID().toString()

    /** Acknowledge and snooze have no `id` in the protocol; doing either twice is harmless. */
    fun ack(key: String) = Action("ack", key, "/waiting/ack", JSONObject().put("key", key).toString(), null)

    fun snooze(key: String, minutes: Int) =
      Action("snooze", key, "/waiting/snooze", JSONObject().put("key", key).put("minutes", minutes.coerceIn(1, 10080)).toString(), null)

    /** A choice as written, or the operator's own words. */
    fun answer(key: String, text: String, id: String = newId()) =
      Action("answer", key, "/questions/answer", JSONObject().put("id", id).put("key", key).put("text", text.take(2000)).toString(), id)

    fun decline(key: String, id: String = newId()) =
      Action("decline", key, "/questions/decline", JSONObject().put("id", id).put("key", key).toString(), id)

    /** Yes to a change waiting for one: the gate's confirm by id (never `/questions/answer`). */
    fun confirm(key: String, changeId: String, id: String = newId()) =
      Action("confirm", key, "/changes/confirm", JSONObject().put("id", id).put("change", changeId).toString(), id)

    fun cancel(key: String, changeId: String, id: String = newId()) =
      Action("cancel", key, "/changes/cancel", JSONObject().put("id", id).put("change", changeId).toString(), id)

    /** The alert's notifications are up: the phone route delivered. `seen` twice answers `ok: false`. */
    fun seen(alertId: String) = Action("seen", null, "/seen", JSONObject().put("alert_id", alertId).toString(), null)

    fun ring(ringId: String, answer: String) =
      Action("ring", null, "/ring", JSONObject().put("ring_id", ringId).put("answer", answer).toString(), null)
  }
}

/** What one try got. Only a signed answer counts: anything else is tried again. */
sealed class Tried {
  data class Answered(val status: Int, val body: JSONObject?) : Tried()
  object Failed : Tried()
}

/** After a try: done, or wait and send the same bytes again, or stop. */
sealed class Next {
  data class Done(val status: Int, val body: JSONObject?) : Next()
  data class Again(val afterMs: Long) : Next()
  object GiveUp : Next()
}

object Retry {
  /** Short of the ten minutes the desk remembers an `id`: past that a resend could act twice. */
  const val GIVE_UP_MS = 9 * 60 * 1000L

  fun next(tried: Tried, attempt: Int, elapsedMs: Long): Next = when (tried) {
    is Tried.Answered -> Next.Done(tried.status, tried.body)
    Tried.Failed -> {
      val wait = Backoff.delayMs(attempt, Random.Default)
      if (elapsedMs + wait > GIVE_UP_MS) Next.GiveUp else Next.Again(wait)
    }
  }

  /** What a signed refusal means to the operator; null when the desk took it. */
  fun errorText(status: Int, body: JSONObject?): String? {
    if (status in 200..299) {
      return if (body != null && body.has("ok") && !body.optBoolean("ok")) "Ghost no longer needed that." else null
    }
    return when (body?.optString("error")) {
      "settled" -> "Already ${body.optString("state", "settled")}."
      "not_waiting" -> "That change is no longer waiting for a yes."
      "bad_answer" -> body.optString("detail").ifEmpty { "That answer doesn’t fit the question." }
      "not_found" -> "Ghost no longer has that."
      "unauthorized" -> "Your desk doesn’t recognise this phone any more. Pair it again."
      "bad_request" -> "Ghost couldn’t read that."
      else -> "Your desk said no ($status)."
    }
  }
}

/** 1, 2, 4 … 30 seconds, each with a quarter either way of jitter so a phone and a desk that
 * came back together don't retry in step. */
object Backoff {
  const val MAX_MS = 30_000L

  fun baseMs(attempt: Int): Long = min(MAX_MS, 1000L shl attempt.coerceIn(0, 5))

  fun delayMs(attempt: Int, random: Random): Long {
    val base = baseMs(attempt)
    val jitter = (base / 4)
    return (base - jitter + random.nextLong(0, 2 * jitter + 1)).coerceAtLeast(250)
  }
}

/** Which of the desk's addresses this is: the first is home, any other the tunnel (ADR 0020). */
fun viaOf(addresses: List<String>, address: String?): String? = when {
  address == null -> null
  addresses.firstOrNull() == address -> "home"
  else -> "tunnel"
}
