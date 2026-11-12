package dev.heka.ghost.call.link

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * The live connection, one per process, with or without JavaScript: it owns the stream's
 * thread (`LinkClient`), the phone's copy of the list (`LinkState`), the notifications
 * (`LinkNotifier`) and the signed actions. The foreground service keeps the process (and so
 * this) alive with the app closed; with the service off it runs only while the app is in
 * front. Notification buttons, the ring screen and the app all act through here, so every
 * action goes out the same way: signed, with its `id`, sent again until the desk signs an
 * answer.
 */
object LinkHub {
  private const val TAG = "GhostLink"
  /** A taken action whose result the stream never mentions gets its buttons back after this. */
  private const val SETTLE_MS = 10_000L
  /** A ring's answer is worth sending only while the ring could still be live. */
  private const val RING_GIVE_UP_MS = 20_000L
  /** The desk waits 15 seconds for `seen`; past that it has moved on to the next route. */
  private const val SEEN_GIVE_UP_MS = 15_000L

  private lateinit var ctx: Context
  private lateinit var store: LinkStore
  lateinit var notifier: LinkNotifier
    private set
  private val state = LinkState()
  private var client: LinkClient? = null
  private var cfg: LinkConfig? = null
  private val main = Handler(Looper.getMainLooper())
  private val pool = Executors.newScheduledThreadPool(2)

  @Volatile var link: Link = Link.OFF
    private set
  @Volatile var address: String? = null
    private set
  /** Wall-clock time of the last message, for "as of 2 min ago" in the app. */
  @Volatile private var heardAt = 0L
  /** Monotonic time of the last message, for the liveness check. */
  @Volatile private var heardMono = 0L
  private var foreground = false
  private var serviceUp = false
  private var lastNetwork: Network? = null
  private var networkCallback: ConnectivityManager.NetworkCallback? = null
  private val ringReplies = HashMap<String, MutableList<(Boolean) -> Unit>>()
  private val ringResults = LinkedHashMap<String, Boolean>()

  /** Told whenever anything the app shows changed. */
  val listeners = CopyOnWriteArraySet<() -> Unit>()
  /** Told when the connection's state changed (the service redraws its notification). */
  val linkListeners = CopyOnWriteArraySet<() -> Unit>()

  private val tickRun = Runnable { tick() }

  @Synchronized
  fun init(context: Context) {
    if (::ctx.isInitialized) return
    ctx = context.applicationContext
    store = LinkStore(ctx)
    notifier = LinkNotifier(ctx)
    notifier.ensureChannels()
    cfg = store.config()
    store.loadState()?.let {
      try {
        state.restore(JSONObject(it))
      } catch (e: Exception) {
        Log.w(TAG, "the saved list didn't read; starting from the desk's snapshot")
      }
    }
    if (cfg == null) link = Link.UNPAIRED
  }

  fun store(): LinkStore = store
  fun config(): LinkConfig? = cfg
  @Synchronized fun streamPath(): String = state.streamPath()

  // ------------------------------------------------------------------ running or not

  /** The app came to the front or left it. */
  fun appForeground(on: Boolean) {
    synchronized(this) { foreground = on }
    if (on) ensureService()
    reconcile()
  }

  fun serviceUp(on: Boolean) {
    synchronized(this) { serviceUp = on }
    reconcile()
  }

  /** Start the foreground service when the operator wants the connection kept. */
  fun ensureService() {
    if (store.enabled && cfg != null) GhostLinkService.start(ctx) else GhostLinkService.stop(ctx)
  }

  @Synchronized
  private fun reconcile() {
    val want = cfg != null && (serviceUp || foreground)
    if (want && client == null) {
      client = LinkClient(this).also { it.isDaemon = true; it.start() }
      watchNetwork(true)
    } else if (!want && client != null) {
      client?.shutdown()
      client = null
      watchNetwork(false)
      setLink(if (cfg == null) Link.UNPAIRED else Link.OFF, null)
    }
  }

  /** Drop the stream and open it again now (the app's Try again, a network change). */
  fun nudge() = client?.nudge()

  /** The service's backstop under Doze: a stream that went quiet without an error is dead. */
  fun checkAlive() {
    if (client != null && link == Link.CONNECTED && SystemClock.elapsedRealtime() - heardMono > LinkClient.DEAD_SECS * 1000) {
      Log.i(TAG, "nothing heard for a while; opening the stream again")
      nudge()
    }
  }

  private fun watchNetwork(on: Boolean) {
    val cm = ctx.getSystemService(ConnectivityManager::class.java) ?: return
    if (on && networkCallback == null) {
      val cb = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
          val before = lastNetwork
          lastNetwork = network
          // Wi-Fi to mobile, or the tunnel coming up (a VPN is a network of its own).
          if (before != null && before != network) nudge()
          else if (before == null && link != Link.CONNECTED) nudge()
        }

        override fun onLost(network: Network) {
          if (network == lastNetwork) {
            lastNetwork = null
            nudge()
          }
        }
      }
      try {
        cm.registerDefaultNetworkCallback(cb)
        networkCallback = cb
      } catch (e: Exception) {
        Log.w(TAG, "no network callback: ${e.message}")
      }
    } else if (!on) {
      networkCallback?.let { runCatching { cm.unregisterNetworkCallback(it) } }
      networkCallback = null
      lastNetwork = null
    }
  }

  // ------------------------------------------------------------------ pairing

  /** The app is paired: the connection uses this desk from now on, with the app closed too. */
  fun configure(deviceId: String, secretB64: String, addresses: List<String>, workstation: String) {
    val before = cfg?.deviceId
    if (!store.configure(deviceId, secretB64, addresses, workstation)) {
      if (cfg == null) cfg = store.config()
      return
    }
    synchronized(this) {
      cfg = store.config()
      if (before != null && before != deviceId) run(state.reset())
    }
    nudge()
    reconcile()
    emit()
  }

  /** Unpaired: stop, forget, take down everything Ghost put up. */
  fun forget() {
    store.forget()
    synchronized(this) {
      cfg = null
      run(state.reset())
    }
    GhostLinkService.stop(ctx)
    reconcile()
    persist()
    emit()
  }

  fun setEnabled(on: Boolean) {
    store.enabled = on
    ensureService()
    reconcile()
    emit()
  }

  fun setBootStart(on: Boolean) {
    store.bootStart = on
    emit()
  }

  @Synchronized
  fun setInSession(on: Boolean) {
    state.inSession = on
  }

  // ------------------------------------------------------------------ the stream

  fun setLink(next: Link, at: String?) {
    val changed = next != link || at != address && next == Link.CONNECTED
    link = next
    if (next == Link.CONNECTED) address = at else if (next != Link.CONNECTING) address = null
    if (changed) {
      linkListeners.forEach { it() }
      emit()
    }
  }

  /** A message that verified but this app doesn't read: the stream is alive all the same. */
  fun heard() {
    heardMono = SystemClock.elapsedRealtime()
    heardAt = System.currentTimeMillis()
  }

  fun onMessage(msg: Msg) {
    heard()
    synchronized(this) {
      run(state.apply(msg, SystemClock.elapsedRealtime()))
      if (msg !is Msg.Heartbeat) persist()
    }
    scheduleTick()
    emit()
  }

  private fun tick() {
    synchronized(this) { run(state.tick(SystemClock.elapsedRealtime())) }
    scheduleTick()
    emit()
  }

  private fun scheduleTick() {
    val next = synchronized(this) { state.rings.nextDeadline() }
    main.removeCallbacks(tickRun)
    if (next != null) main.postDelayed(tickRun, (next - SystemClock.elapsedRealtime()).coerceAtLeast(0) + 5)
  }

  private fun persist() {
    try {
      store.saveState(state.save().toString())
    } catch (e: Exception) {
      Log.w(TAG, "could not keep the list: ${e.javaClass.simpleName}")
    }
  }

  /** Carry out what the state decided. Called holding the lock. */
  private fun run(fx: List<Fx>) {
    val posted = HashSet<String>()
    for (f in fx) {
      when (f) {
        is Fx.Post -> if (notifier.post(f.note)) posted += f.note.key
        is Fx.Clear -> notifier.clear(f.key)
        is Fx.Seen -> {
          // Delivered only when every notification is really up: without that, the desk moves
          // on to its next route rather than believe a phone that showed nothing.
          if (f.keys.isNotEmpty() && f.keys.all { it in posted }) send(Action.seen(f.alertId), SEEN_GIVE_UP_MS) { _, _ -> }
          else Log.i(TAG, "alert not shown (notifications off?): not telling the desk it was seen")
        }
        is Fx.Ringing -> when (val r = f.fx) {
          is RingFx.Show -> {
            if (!notifier.ring(r.call, r.remainingMs, cfg?.workstation ?: "")) Log.i(TAG, "ring not shown (notifications off?)")
          }
          is RingFx.Dismiss -> notifier.stopRing(r.ringId)
        }
      }
    }
  }

  private fun run(f: Fx?) {
    if (f != null) run(listOf(f))
  }

  // ------------------------------------------------------------------ what the operator does

  /** Send one action; `done` gets null when the desk took it, or why it didn't. */
  fun act(action: Action, done: (String?) -> Unit) {
    val key = action.key
    if (key != null) synchronized(this) { run(state.sendingNow(key)) }
    emit()
    send(action, Retry.GIVE_UP_MS) { error, _ ->
      if (key != null) {
        synchronized(this) { run(state.answered(key, error)) }
        if (error == null) {
          // The stream says what came of it; if it never does (it dropped), the buttons come back.
          pool.schedule({
            synchronized(this) { run(state.stillSending(key)) }
            emit()
          }, SETTLE_MS, TimeUnit.MILLISECONDS)
        }
        emit()
      }
      done(error)
    }
  }

  /**
   * The operator answered a ring (the call screen or the notification's Answer). The ringing
   * stops at once; `POST /ring answered` goes once, and every caller (the ring screen, then the
   * app opening the call) gets the same answer: true while the ring was still live.
   */
  fun answerRing(ringId: String, done: (Boolean) -> Unit) {
    // null: already answered; false: an answer is on its way; true: this call sends it.
    val first: Boolean? = synchronized(this) {
      val waiting = ringReplies[ringId]
      when {
        ringResults.containsKey(ringId) -> null
        waiting != null -> {
          waiting += done
          false
        }
        else -> {
          ringReplies[ringId] = mutableListOf(done)
          run(state.rings.over(ringId).map { Fx.Ringing(it) })
          true
        }
      }
    }
    if (first == null) return done(synchronized(this) { ringResults[ringId] } ?: false)
    if (!first) return
    notifier.stopRing(ringId)
    send(Action.ring(ringId, "answered"), RING_GIVE_UP_MS) { error, body ->
      val ok = error == null && body?.optBoolean("ok", false) == true
      val waiters = synchronized(this) {
        ringResults[ringId] = ok
        while (ringResults.size > 32) ringResults.remove(ringResults.keys.first())
        ringReplies.remove(ringId).orEmpty()
      }
      waiters.forEach { it(ok) }
    }
    emit()
  }

  fun declineRing(ringId: String) {
    synchronized(this) { run(state.rings.over(ringId).map { Fx.Ringing(it) }) }
    notifier.stopRing(ringId)
    send(Action.ring(ringId, "declined"), RING_GIVE_UP_MS) { _, _ -> }
    emit()
  }

  @Synchronized
  fun dismissed(key: String) = state.dismissed(key)

  /**
   * Send `action` until the desk signs an answer, or give up. `done` gets the error text (null
   * when the desk took it) and the signed body.
   */
  private fun send(action: Action, giveUpMs: Long, done: (String?, JSONObject?) -> Unit) {
    val started = SystemClock.elapsedRealtime()
    val unreachable = "Couldn’t reach your desk. Try again."
    fun attempt(n: Int) {
      val config = cfg ?: return done("This phone isn’t paired.", null)
      val tried = tryEvery(config, action)
      val elapsed = SystemClock.elapsedRealtime() - started
      when (val next = Retry.next(tried, n, elapsed)) {
        is Next.Done -> done(Retry.errorText(next.status, next.body)?.takeIf { next.status !in 200..299 }, next.body)
        is Next.Again ->
          if (elapsed + next.afterMs > giveUpMs) done(unreachable, null)
          else pool.schedule({ attempt(n + 1) }, next.afterMs, TimeUnit.MILLISECONDS)
        Next.GiveUp -> done(unreachable, null)
      }
    }
    pool.execute { attempt(0) }
  }

  /** The stream's address first, then the others in order (home, then the tunnel). */
  private fun tryEvery(config: LinkConfig, action: Action): Tried {
    val order = (listOfNotNull(address) + config.addresses).distinct()
    for (a in order) {
      val t = LinkClient.post(config, a, action)
      if (t is Tried.Answered) return t
    }
    return Tried.Failed
  }

  // ------------------------------------------------------------------ what the app reads

  fun emit() = listeners.forEach { runCatching { it() } }

  @Synchronized
  fun view(): JSONObject {
    val c = cfg
    return state.view()
      .put("link", link.word)
      .put("address", address ?: JSONObject.NULL)
      .put("via", viaOf(c?.addresses.orEmpty(), address) ?: JSONObject.NULL)
      .put("heardAt", if (heardAt > 0) heardAt else JSONObject.NULL)
      .put("enabled", store.enabled)
      .put("bootStart", store.bootStart)
      .put("paired", c != null)
      .put("ringing", JSONArray().also { a ->
        state.rings.ringing().forEach { r ->
          a.put(JSONObject().put("ringId", r.ringId).put("agent", r.agent).put("key", r.key))
        }
      })
  }
}
