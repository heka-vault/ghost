package dev.heka.ghost.call.link

import android.app.AlarmManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

/**
 * Keeps the live connection to the desk with the app closed (ADR 0051): a foreground service
 * of type `connectedDevice` (the paired desk is the device), separate from the session's
 * microphone service and never holding the microphone. Its one notification is on a minimal
 * channel and says whether Ghost is connected, and how.
 *
 * Doze: a process with a foreground service keeps its network in Doze and App Standby, so
 * the desk's heartbeat (every 15 seconds) still arrives and wakes the phone for a moment;
 * nothing here holds a wake lock. What Doze does stop is this side's own clock: a tunnel that
 * dies without a word would go unnoticed until the phone next wakes. An inexact alarm allowed
 * while idle checks every 15 minutes at most (Doze's own floor for such alarms is about 9) and
 * opens the stream again if nothing was heard; a network change does at once.
 */
class GhostLinkService : Service() {
  private val redraw = { draw() }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    super.onCreate()
    LinkHub.init(this)
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_CHECK) {
      LinkHub.checkAlive()
      scheduleCheck()
      return START_STICKY
    }
    if (!LinkHub.store().enabled || LinkHub.config() == null) {
      stopSelf()
      return START_NOT_STICKY
    }
    try {
      val n = LinkHub.notifier.status(LinkHub.link, viaNow(), LinkHub.config()?.workstation ?: "")
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        startForeground(LinkNotifier.STATUS_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
      } else {
        startForeground(LinkNotifier.STATUS_ID, n)
      }
    } catch (e: Exception) {
      // Not allowed from where it was started (a background start Android refuses): the
      // connection runs while the app is open, and comes back here the next time it is.
      Log.w(TAG, "could not go foreground: ${e.javaClass.simpleName}")
      stopSelf()
      return START_NOT_STICKY
    }
    running = true
    LinkHub.linkListeners.add(redraw)
    LinkHub.serviceUp(true)
    scheduleCheck()
    return START_STICKY
  }

  /** The app's screen was swiped away: the connection is exactly what should stay. */
  override fun onTaskRemoved(rootIntent: Intent?) {}

  override fun onDestroy() {
    running = false
    LinkHub.linkListeners.remove(redraw)
    LinkHub.serviceUp(false)
    cancelCheck()
    super.onDestroy()
  }

  private fun viaNow() = viaOf(LinkHub.config()?.addresses.orEmpty(), LinkHub.address)

  private fun draw() {
    if (!running) return
    try {
      NotificationManagerCompat.from(this).notify(LinkNotifier.STATUS_ID, LinkHub.notifier.status(LinkHub.link, viaNow(), LinkHub.config()?.workstation ?: ""))
    } catch (e: SecurityException) {
      // notifications off: the service still runs, its notification just isn't drawn
    }
  }

  private fun checkIntent(): PendingIntent =
    PendingIntent.getService(this, 2, Intent(this, GhostLinkService::class.java).setAction(ACTION_CHECK), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)

  private fun scheduleCheck() {
    val am = getSystemService(AlarmManager::class.java) ?: return
    am.setAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, SystemClock.elapsedRealtime() + CHECK_MS, checkIntent())
  }

  private fun cancelCheck() {
    getSystemService(AlarmManager::class.java)?.cancel(checkIntent())
  }

  companion object {
    private const val TAG = "GhostLink"
    private const val ACTION_CHECK = "dev.heka.ghost.link.CHECK"
    private const val CHECK_MS = 15 * 60 * 1000L
    @Volatile var running = false
      private set

    fun start(context: Context) {
      try {
        ContextCompat.startForegroundService(context, Intent(context, GhostLinkService::class.java))
      } catch (e: Exception) {
        Log.w(TAG, "could not start: ${e.javaClass.simpleName}")
      }
    }

    fun stop(context: Context) {
      context.stopService(Intent(context, GhostLinkService::class.java))
    }
  }
}
