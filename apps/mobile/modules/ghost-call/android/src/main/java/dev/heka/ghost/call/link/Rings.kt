package dev.heka.ghost.call.link

/** A ring from the desk (ADR 0051, issue #22), and when this phone first heard it. */
data class RingCall(
  val ringId: String,
  val key: String,
  val agent: String,
  val title: String,
  val ringSecs: Int,
  val arrivedAt: Long,
) {
  fun endsAt(): Long = arrivedAt + ringSecs * 1000L
}

sealed class RingFx {
  /** Raise the incoming call; it stops by itself after `remainingMs`. */
  data class Show(val call: RingCall, val remainingMs: Long) : RingFx()
  data class Dismiss(val ringId: String) : RingFx()
}

/**
 * The ring's lifecycle, pure so it is tested off the phone. A ring is live from its arrival
 * until its `ring_over` or `ring_secs` later, whichever is first. It rings only after a short
 * settle: a ring replayed after a reconnect is followed in the same burst by its `ring_over`
 * when it is already over, and then it never rings. A ring id rings once, ever: replayed again
 * after another reconnect it is ignored. During a session on this phone a ring is not shown
 * (the desk then hears nothing, and moves on as for a ring nobody answered). A ring never
 * answers itself.
 */
class Rings(private val settleMs: Long = SETTLE_MS) {
  private val pending = LinkedHashMap<String, RingCall>()
  private val showing = LinkedHashMap<String, RingCall>()
  private val done = LinkedHashSet<String>()

  fun arrive(r: Msg.Ring, now: Long) {
    if (r.ringId in done || r.ringId in pending || r.ringId in showing) return
    pending[r.ringId] = RingCall(r.ringId, r.key, r.agent, r.title, r.ringSecs.coerceIn(1, 120), now)
  }

  /** `ring_over` from the desk, or the operator answered or declined here. */
  fun over(ringId: String): List<RingFx> {
    finish(ringId)
    pending.remove(ringId)
    return if (showing.remove(ringId) != null) listOf(RingFx.Dismiss(ringId)) else emptyList()
  }

  /** What to do now: rings to raise once settled, rings whose time ran out. */
  fun tick(now: Long, inSession: Boolean): List<RingFx> {
    val out = mutableListOf<RingFx>()
    for (call in pending.values.toList()) {
      if (now - call.arrivedAt < settleMs) continue
      pending.remove(call.ringId)
      if (now >= call.endsAt() || inSession) {
        finish(call.ringId)
        continue
      }
      showing[call.ringId] = call
      out += RingFx.Show(call, call.endsAt() - now)
    }
    for (call in showing.values.toList()) {
      if (now >= call.endsAt()) {
        showing.remove(call.ringId)
        finish(call.ringId)
        out += RingFx.Dismiss(call.ringId)
      }
    }
    return out
  }

  /** When `tick` next has something to do; null when nothing is ringing or waiting to. */
  fun nextDeadline(): Long? =
    (pending.values.map { it.arrivedAt + settleMs } + showing.values.map { it.endsAt() }).minOrNull()

  fun live(ringId: String): RingCall? = showing[ringId] ?: pending[ringId]

  fun ringing(): List<RingCall> = showing.values.toList()

  fun clear() {
    pending.clear()
    showing.clear()
  }

  private fun finish(ringId: String) {
    done += ringId
    while (done.size > KEEP) done.remove(done.first())
  }

  companion object {
    const val SETTLE_MS = 400L
    private const val KEEP = 128
  }
}
