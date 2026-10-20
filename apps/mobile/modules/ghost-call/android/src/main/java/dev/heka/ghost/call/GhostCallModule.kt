package dev.heka.ghost.call

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

/**
 * JS face of GhostCallService: start/update/stop, an "end" event from the notification, and a
 * "tick" event once a second while the service runs (JS timers sleep in the background).
 */
class GhostCallModule : Module() {
  private val context
    get() = requireNotNull(appContext.reactContext) { "no React context" }

  override fun definition() = ModuleDefinition {
    Name("GhostCall")
    Events("end", "tick")

    OnCreate {
      GhostCallService.onEnd = { sendEvent("end", emptyMap<String, Any>()) }
      GhostCallService.onTick = { sendEvent("tick", emptyMap<String, Any>()) }
    }

    OnDestroy {
      GhostCallService.onEnd = null
      GhostCallService.onTick = null
    }

    Function("start") { title: String, body: String ->
      GhostCallService.start(context, title, body)
    }

    Function("update") { body: String ->
      GhostCallService.update(context, body)
    }

    Function("stop") {
      GhostCallService.stop(context)
    }

    Function("isRunning") {
      GhostCallService.running
    }
  }
}
