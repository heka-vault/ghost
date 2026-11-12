package dev.heka.ghost.call.link

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import expo.modules.kotlin.Promise
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import org.json.JSONObject

/**
 * JS face of the live connection (`modules/ghost-call/link.ts`): hand it the pairing, read the
 * list it keeps, act on it, and the settings and permissions behind it. The connection itself
 * lives in `LinkHub` and does not need JavaScript; this only lets the app see and steer it.
 */
class GhostLinkModule : Module() {
  private val context
    get() = requireNotNull(appContext.reactContext) { "no React context" }

  private val onChange = { sendEvent("state", mapOf("json" to LinkHub.view().toString())) }

  override fun definition() = ModuleDefinition {
    Name("GhostLink")
    Events("state")

    OnCreate {
      LinkHub.init(context)
      LinkHub.listeners.add(onChange)
    }

    OnDestroy {
      LinkHub.listeners.remove(onChange)
    }

    OnActivityEntersForeground { LinkHub.appForeground(true) }
    OnActivityEntersBackground { LinkHub.appForeground(false) }

    Function("configure") { deviceId: String, secret: String, addresses: List<String>, workstation: String ->
      LinkHub.configure(deviceId, secret, addresses, workstation)
      LinkHub.appForeground(true)
    }

    Function("forget") { LinkHub.forget() }

    Function("getState") { LinkHub.view().toString() }

    Function("setEnabled") { on: Boolean -> LinkHub.setEnabled(on) }

    Function("setBootStart") { on: Boolean -> LinkHub.setBootStart(on) }

    Function("setInSession") { on: Boolean -> LinkHub.setInSession(on) }

    Function("retry") { LinkHub.nudge() }

    /** One action on the waiting list; resolves with null when the desk took it, or why not. */
    AsyncFunction("act") { kind: String, key: String, arg: String?, promise: Promise ->
      val action = when (kind) {
        "ack" -> Action.ack(key)
        "snooze" -> Action.snooze(key, arg?.toIntOrNull() ?: Notes.LATER_MINUTES)
        "answer" -> arg?.let { Action.answer(key, it) }
        "decline" -> Action.decline(key)
        "confirm" -> arg?.let { Action.confirm(key, it) }
        "cancel" -> arg?.let { Action.cancel(key, it) }
        else -> null
      }
      if (action == null) return@AsyncFunction promise.resolve("Ghost couldn’t read that.")
      LinkHub.act(action) { error -> promise.resolve(error) }
    }

    /** The ring the app is opening the call for: true while it was still live at the desk. */
    AsyncFunction("answerRing") { ringId: String, promise: Promise ->
      LinkHub.answerRing(ringId) { ok -> promise.resolve(ok) }
    }

    Function("declineRing") { ringId: String -> LinkHub.declineRing(ringId) }

    Function("permissions") {
      val n = LinkHub.notifier
      JSONObject()
        .put("notifications", n.canPost(Channel.WAITING.id) || n.canPost(Channel.ASK.id))
        .put("asks", n.canPost(Channel.ASK.id))
        .put("calls", n.canPost(LinkNotifier.RING_CHANNEL))
        .put("fullScreen", n.canRingFullScreen())
        .put("needsRuntimePermission", Build.VERSION.SDK_INT >= 33)
        .toString()
    }

    Function("openNotificationSettings") {
      open(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
    }

    Function("openChannelSettings") { channel: String ->
      open(
        Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS)
          .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
          .putExtra(Settings.EXTRA_CHANNEL_ID, channel)
      )
    }

    Function("openFullScreenSettings") {
      if (Build.VERSION.SDK_INT >= 34) {
        open(Intent(Settings.ACTION_MANAGE_APP_USE_FULL_SCREEN_INTENT, Uri.parse("package:${context.packageName}")))
      } else {
        open(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
      }
    }
  }

  private fun open(intent: Intent) {
    try {
      context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (e: Exception) {
      context.startActivity(
        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      )
    }
  }
}
