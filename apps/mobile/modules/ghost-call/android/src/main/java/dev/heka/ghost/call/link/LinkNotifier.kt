package dev.heka.ghost.call.link

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.app.RemoteInput
import androidx.core.content.ContextCompat
import dev.heka.ghost.call.R

/**
 * Draws what the hub decided: the connection's own quiet notification, a notification per
 * waiting item or question with its actions, and the incoming call for a ring. Every word on
 * them comes from the desk as data (an agent's question, a watch's title): it is set as text,
 * never markup, and always with who it is from. On the lock screen only "Ghost: something is
 * waiting" shows until the phone is unlocked.
 */
class LinkNotifier(context: Context) {
  private val ctx = context.applicationContext
  private val nm = NotificationManagerCompat.from(ctx)

  fun ensureChannels() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return // no channels before Android 8
    val sys = ctx.getSystemService(NotificationManager::class.java) ?: return
    sys.createNotificationChannel(NotificationChannel(STATUS_CHANNEL, "Connection to your desk", NotificationManager.IMPORTANCE_MIN).apply {
      description = "Shown while Ghost keeps its connection to your desk open, so questions and calls reach you with the app closed."
      setShowBadge(false)
    })
    sys.createNotificationChannel(NotificationChannel(Channel.ASK.id, "Questions and changes", NotificationManager.IMPORTANCE_HIGH).apply {
      description = "An agent asks you something, or a change waits for your yes."
    })
    sys.createNotificationChannel(NotificationChannel(Channel.WAITING.id, "Waiting for you", NotificationManager.IMPORTANCE_DEFAULT).apply {
      description = "Something on your waiting list: a watch, a webhook, an agent's note."
    })
    sys.createNotificationChannel(NotificationChannel(Channel.QUIET.id, "Quiet questions", NotificationManager.IMPORTANCE_LOW).apply {
      description = "Questions that can wait: no sound."
    })
    sys.createNotificationChannel(NotificationChannel(RING_CHANNEL, "Calls from Ghost", NotificationManager.IMPORTANCE_HIGH).apply {
      description = "Ghost calls when something is urgent and you allowed calls at your desk."
      // The phone's ringtone, as loud as calls are: the ringer mode (silent, vibrate) applies.
      setSound(
        RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE),
        AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE).setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build(),
      )
      enableVibration(true)
      vibrationPattern = longArrayOf(0, 800, 600, 800, 600)
      lockscreenVisibility = Notification.VISIBILITY_PUBLIC
    })
  }

  /** Notifications are allowed for the app, and this channel isn't turned off. */
  fun canPost(channel: String): Boolean {
    if (Build.VERSION.SDK_INT >= 33 && ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return false
    if (!nm.areNotificationsEnabled()) return false
    val c = nm.getNotificationChannel(channel) ?: return true
    return c.importance != NotificationManager.IMPORTANCE_NONE
  }

  /** Android 14+ asks the operator before an app may show a call over the lock screen. */
  fun canRingFullScreen(): Boolean = Build.VERSION.SDK_INT < 34 || ctx.getSystemService(NotificationManager::class.java)?.canUseFullScreenIntent() == true

  // ------------------------------------------------------------------ the connection's own

  fun status(link: Link, via: String?, workstation: String): Notification {
    val desk = workstation.ifEmpty { "your desk" }
    val text = when (link) {
      Link.CONNECTED -> "Connected to $desk" + when (via) { "home" -> " on your home network"; "tunnel" -> " through your tunnel"; else -> "" }
      Link.CONNECTING -> "Connecting to $desk"
      Link.RECONNECTING -> "Reconnecting to $desk"
      Link.OFFLINE -> "Can’t reach $desk. Trying again."
      Link.REFUSED -> "$desk doesn’t recognise this phone. Pair it again."
      Link.UNPAIRED -> "Not paired with a desk"
      Link.OFF -> "Off"
    }
    return NotificationCompat.Builder(ctx, STATUS_CHANNEL)
      .setSmallIcon(R.drawable.ghost_link)
      .setContentTitle("Ghost")
      .setContentText(text)
      .setOngoing(true)
      .setSilent(true)
      .setShowWhen(false)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setPriority(NotificationCompat.PRIORITY_MIN)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_DEFERRED)
      .setContentIntent(openApp(0))
      .build()
  }

  // ------------------------------------------------------------------ the waiting list

  fun post(note: Note): Boolean {
    if (!canPost(note.channel.id)) return false
    val lines = buildList {
      add(note.text)
      note.detail?.takeIf { it.isNotBlank() }?.let { add(it) }
      if (note.more > 0) add(if (note.more == 1) "One more choice in Ghost." else "${note.more} more choices in Ghost.")
      note.error?.let { add(it) }
    }
    val b = NotificationCompat.Builder(ctx, note.channel.id)
      .setSmallIcon(R.drawable.ghost_link)
      .setContentTitle(note.title)
      .setContentText(note.text)
      .setStyle(NotificationCompat.BigTextStyle().bigText(lines.joinToString("\n")))
      .setSubText(if (note.sending) "Sending…" else null)
      .setOnlyAlertOnce(true)
      .setSilent(note.silent)
      .setAutoCancel(false)
      .setCategory(if (note.channel == Channel.WAITING) NotificationCompat.CATEGORY_REMINDER else NotificationCompat.CATEGORY_MESSAGE)
      .setGroup(GROUP)
      .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
      .setPublicVersion(
        NotificationCompat.Builder(ctx, note.channel.id)
          .setSmallIcon(R.drawable.ghost_link)
          .setContentTitle("Ghost")
          .setContentText("Something is waiting for you")
          .build()
      )
      .setContentIntent(openApp(note.key.hashCode()))
      .setDeleteIntent(broadcast(LinkActionReceiver.DISMISSED, note.key, null))
    for (a in note.actions) {
      when (a) {
        NoteAction.Reply -> b.addAction(
          NotificationCompat.Action.Builder(0, a.label, broadcast(LinkActionReceiver.REPLY, note.key, null, mutable = true))
            .addRemoteInput(RemoteInput.Builder(LinkActionReceiver.REPLY_KEY).setLabel("Your answer").build())
            .setAllowGeneratedReplies(false)
            .setSemanticAction(NotificationCompat.Action.SEMANTIC_ACTION_REPLY)
            .build()
        )
        else -> b.addAction(0, a.label, broadcast(actionName(a), note.key, actionArg(a)))
      }
    }
    return try {
      nm.notify(note.key, NOTE_ID, b.build())
      true
    } catch (e: SecurityException) {
      false
    }
  }

  fun clear(key: String) = nm.cancel(key, NOTE_ID)

  // ------------------------------------------------------------------ the ring

  /**
   * The incoming call: a call-style notification with a full-screen intent, shown over the lock
   * screen. It says which agent is calling and nothing of what about until it is answered.
   */
  fun ring(call: RingCall, remainingMs: Long, workstation: String): Boolean {
    if (!canPost(RING_CHANNEL)) return false
    val name = agentName(call.agent)
    val caller = Person.Builder().setName(name).setImportant(true).build()
    val full = PendingIntent.getActivity(ctx, call.ringId.hashCode(), RingActivity.intent(ctx, call, name, answer = false), IMMUTABLE)
    val answer = PendingIntent.getActivity(ctx, call.ringId.hashCode() + 1, RingActivity.intent(ctx, call, name, answer = true), IMMUTABLE)
    val decline = broadcast(LinkActionReceiver.RING_DECLINE, call.key, call.ringId)
    val n = NotificationCompat.Builder(ctx, RING_CHANNEL)
      .setSmallIcon(R.drawable.ghost_link)
      .setContentTitle(name)
      .setContentText("Calling through Ghost on ${workstation.ifEmpty { "your desk" }}")
      .setStyle(NotificationCompat.CallStyle.forIncomingCall(caller, decline, answer))
      .setCategory(NotificationCompat.CATEGORY_CALL)
      .setPriority(NotificationCompat.PRIORITY_MAX)
      .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
      .setFullScreenIntent(full, true)
      .setContentIntent(full)
      .setOngoing(true)
      .setAutoCancel(false)
      .setTimeoutAfter(remainingMs.coerceAtLeast(1000))
      .build()
    n.flags = n.flags or Notification.FLAG_INSISTENT // ring until answered, declined or over
    return try {
      nm.notify(ringTag(call.ringId), RING_ID, n)
      true
    } catch (e: SecurityException) {
      false
    }
  }

  fun stopRing(ringId: String) {
    nm.cancel(ringTag(ringId), RING_ID)
    RingActivity.finish(ringId)
  }

  // ------------------------------------------------------------------ intents

  private fun openApp(code: Int): PendingIntent? =
    ctx.packageManager.getLaunchIntentForPackage(ctx.packageName)?.let {
      PendingIntent.getActivity(ctx, code, it.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), IMMUTABLE)
    }

  private fun broadcast(action: String, key: String, arg: String?, mutable: Boolean = false): PendingIntent {
    val intent = Intent(ctx, LinkActionReceiver::class.java)
      .setAction(action)
      // A distinct data URI per key and choice, so one notification's buttons never replace another's.
      .setData(Uri.parse("ghost-action://$action/${Uri.encode(key)}/${Uri.encode(arg ?: "")}"))
      .putExtra(LinkActionReceiver.KEY, key)
      .putExtra(LinkActionReceiver.ARG, arg)
    val flags = PendingIntent.FLAG_UPDATE_CURRENT or if (mutable) PendingIntent.FLAG_MUTABLE else PendingIntent.FLAG_IMMUTABLE
    return PendingIntent.getBroadcast(ctx, 0, intent, flags)
  }

  private fun actionName(a: NoteAction): String = when (a) {
    NoteAction.Ack -> LinkActionReceiver.ACK
    is NoteAction.Snooze -> LinkActionReceiver.SNOOZE
    is NoteAction.Choice -> LinkActionReceiver.CHOICE
    NoteAction.Decline -> LinkActionReceiver.DECLINE
    is NoteAction.Yes -> LinkActionReceiver.YES
    is NoteAction.No -> LinkActionReceiver.NO
    NoteAction.Reply -> LinkActionReceiver.REPLY
  }

  private fun actionArg(a: NoteAction): String? = when (a) {
    is NoteAction.Snooze -> a.minutes.toString()
    is NoteAction.Choice -> a.text
    is NoteAction.Yes -> a.changeId
    is NoteAction.No -> a.changeId
    else -> null
  }

  companion object {
    const val STATUS_CHANNEL = "ghost-link"
    const val RING_CHANNEL = "ghost-ring"
    const val STATUS_ID = 8
    private const val NOTE_ID = 9
    private const val RING_ID = 10
    private const val GROUP = "ghost.waiting"
    private const val IMMUTABLE = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT

    private fun ringTag(ringId: String) = "ring:$ringId"

    /** An agent's slug as its name, as `agentName` in @ghost/ui: `heka-workshop` is "Heka Workshop". */
    fun agentName(slug: String): String = Notes.agentName(slug)
  }
}
