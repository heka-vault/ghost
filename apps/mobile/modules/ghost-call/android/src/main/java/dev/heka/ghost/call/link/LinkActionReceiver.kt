package dev.heka.ghost.call.link

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.RemoteInput

/**
 * A button on one of Ghost's notifications, with the app closed or open: each becomes a
 * signed action through the hub (`LinkHub.act`), with a fresh `id` when the route takes one.
 * The notification loses its buttons while the action is on its way and comes down only when
 * the stream says what came of it. `goAsync` keeps the process up until the desk answers, in
 * case the connection's service is off.
 */
class LinkActionReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    LinkHub.init(context)
    val key = intent.getStringExtra(KEY) ?: return
    val arg = intent.getStringExtra(ARG)
    val action = when (intent.action) {
      ACK -> Action.ack(key)
      SNOOZE -> Action.snooze(key, arg?.toIntOrNull() ?: Notes.LATER_MINUTES)
      CHOICE -> arg?.let { Action.answer(key, it) }
      REPLY -> RemoteInput.getResultsFromIntent(intent)?.getCharSequence(REPLY_KEY)?.toString()?.trim()?.takeIf { it.isNotEmpty() }?.let { Action.answer(key, it) }
      DECLINE -> Action.decline(key)
      YES -> arg?.let { Action.confirm(key, it) }
      NO -> arg?.let { Action.cancel(key, it) }
      RING_DECLINE -> {
        if (arg != null) LinkHub.declineRing(arg)
        null
      }
      DISMISSED -> {
        LinkHub.dismissed(key)
        null
      }
      else -> null
    } ?: return
    // A receiver may hold the process for a few seconds only; retries past that go on in the
    // connection's service (or stop with the process when it is off).
    val pending = goAsync()
    val finished = java.util.concurrent.atomic.AtomicBoolean(false)
    val finish = { if (finished.compareAndSet(false, true)) pending.finish() }
    android.os.Handler(android.os.Looper.getMainLooper()).postDelayed(finish, HOLD_MS)
    LinkHub.act(action) { finish() }
  }

  companion object {
    private const val HOLD_MS = 8_000L
    const val KEY = "key"
    const val ARG = "arg"
    const val REPLY_KEY = "answer"
    const val ACK = "dev.heka.ghost.link.ACK"
    const val SNOOZE = "dev.heka.ghost.link.SNOOZE"
    const val CHOICE = "dev.heka.ghost.link.CHOICE"
    const val REPLY = "dev.heka.ghost.link.REPLY"
    const val DECLINE = "dev.heka.ghost.link.DECLINE"
    const val YES = "dev.heka.ghost.link.YES"
    const val NO = "dev.heka.ghost.link.NO"
    const val RING_DECLINE = "dev.heka.ghost.link.RING_DECLINE"
    const val DISMISSED = "dev.heka.ghost.link.DISMISSED"
  }
}

/**
 * After a restart (or an update of the app), the connection comes back by itself only when
 * the operator turned on "Start when the phone starts". Android allows a foreground service
 * to start from these two broadcasts.
 */
class LinkBootReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    if (intent.action != Intent.ACTION_BOOT_COMPLETED && intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
    LinkHub.init(context)
    val store = LinkHub.store()
    if (store.enabled && store.bootStart && LinkHub.config() != null) GhostLinkService.start(context)
  }
}
