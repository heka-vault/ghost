package dev.heka.ghost.call

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/**
 * Keeps a session alive with the screen off. Android 12+ silences an app's microphone once it
 * leaves the foreground unless a foreground service of type "microphone" is running, so the
 * app starts this one right after the call is up and stops it when the call ends. It shows one
 * quiet, ongoing notification ("Ghost · in a session") with an End action, and holds a partial
 * wake lock so the audio threads are not throttled in a pocket.
 */
class GhostCallService : Service() {
  private val handler = Handler(Looper.getMainLooper())
  /**
   * React Native runs JavaScript timers off the screen's frame clock, so they stop while the
   * app is in the background and the call's keepalive to the desk would stop with them. This
   * tick reaches JS as an event instead, once a second, for as long as the service runs.
   */
  private val tick = object : Runnable {
    override fun run() {
      onTick?.invoke()
      handler.postDelayed(this, TICK_MS)
    }
  }
  private var wakeLock: PowerManager.WakeLock? = null
  private var title = "Ghost"
  private var body = ""
  private var foreground = false

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_START -> {
        title = intent.getStringExtra(EXTRA_TITLE) ?: title
        body = intent.getStringExtra(EXTRA_BODY) ?: body
        ensureChannel()
        try {
          if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            startForeground(NOTIFICATION_ID, build(), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
          } else {
            startForeground(NOTIFICATION_ID, build())
          }
        } catch (e: Exception) {
          // Not allowed right now (app in the background, or a permission missing): the call
          // still runs while the screen is on; it just won't survive the screen turning off.
          Log.w(TAG, "could not go foreground: ${e.message}")
          stopSelf()
          return START_NOT_STICKY
        }
        foreground = true
        running = true
        acquire()
        handler.removeCallbacks(tick)
        handler.postDelayed(tick, TICK_MS)
      }
      ACTION_UPDATE -> {
        body = intent.getStringExtra(EXTRA_BODY) ?: body
        if (foreground) notificationManager().notify(NOTIFICATION_ID, build())
      }
      ACTION_END -> {
        // The End action on the notification. JS ends the call and then stops this service;
        // with no JS listening, stop now so the mic is not held by a dead call.
        val handler = onEnd
        if (handler != null) handler() else stopSelf()
      }
      else -> stopSelf()
    }
    return START_NOT_STICKY
  }

  override fun onTaskRemoved(rootIntent: Intent?) {
    // The app was swiped away: nothing can end the call cleanly any more, so release the mic.
    // The desk notices the missing keepalive and ends the session on its side.
    stopSelf()
  }

  override fun onDestroy() {
    handler.removeCallbacks(tick)
    running = false
    foreground = false
    wakeLock?.let { if (it.isHeld) it.release() }
    wakeLock = null
    stopForeground(STOP_FOREGROUND_REMOVE)
    super.onDestroy()
  }

  private fun acquire() {
    if (wakeLock != null) return
    val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
    wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "ghost:call").also {
      it.setReferenceCounted(false)
      it.acquire(MAX_CALL_MS)
    }
  }

  private fun notificationManager() = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

  private fun ensureChannel() {
    val channel = NotificationChannel(CHANNEL_ID, "Session", NotificationManager.IMPORTANCE_LOW).apply {
      description = "Shown while Ghost has the microphone open."
      setShowBadge(false)
      setSound(null, null)
      enableVibration(false)
    }
    notificationManager().createNotificationChannel(channel)
  }

  private fun build(): Notification {
    val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
      PendingIntent.getActivity(this, 0, it, flags)
    }
    val end = PendingIntent.getService(
      this, 1, Intent(this, GhostCallService::class.java).setAction(ACTION_END), flags
    )
    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(R.drawable.ghost_call_mic)
      .setContentTitle(title)
      .setContentText(body)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setSilent(true)
      .setCategory(NotificationCompat.CATEGORY_CALL)
      .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .apply { if (open != null) setContentIntent(open) }
      .addAction(0, "End", end)
      .build()
  }

  companion object {
    private const val TAG = "GhostCall"
    const val CHANNEL_ID = "ghost-session"
    const val NOTIFICATION_ID = 7
    const val ACTION_START = "dev.heka.ghost.call.START"
    const val ACTION_UPDATE = "dev.heka.ghost.call.UPDATE"
    const val ACTION_END = "dev.heka.ghost.call.END"
    const val EXTRA_TITLE = "title"
    const val EXTRA_BODY = "body"
    /** A session longer than this releases the wake lock; the call itself goes on. */
    private const val MAX_CALL_MS = 4L * 60 * 60 * 1000
    private const val TICK_MS = 1000L

    /** Set by the module while JS is listening; the notification's End action calls it. */
    @Volatile var onEnd: (() -> Unit)? = null
    /** Set by the module while JS is listening; called once a second while the service runs. */
    @Volatile var onTick: (() -> Unit)? = null
    @Volatile var running = false
      private set

    fun start(context: Context, title: String, body: String) {
      val intent = Intent(context, GhostCallService::class.java)
        .setAction(ACTION_START)
        .putExtra(EXTRA_TITLE, title)
        .putExtra(EXTRA_BODY, body)
      ContextCompat.startForegroundService(context, intent)
    }

    fun update(context: Context, body: String) {
      if (!running) return
      try {
        context.startService(
          Intent(context, GhostCallService::class.java).setAction(ACTION_UPDATE).putExtra(EXTRA_BODY, body)
        )
      } catch (e: Exception) {
        Log.w(TAG, "could not update the notification: ${e.message}")
      }
    }

    fun stop(context: Context) {
      context.stopService(Intent(context, GhostCallService::class.java))
    }
  }
}
