/**
 * The session's foreground service (Android): keeps the microphone alive with the screen
 * off and shows one ongoing notification with an End action. A local Expo module
 * (`android/`); on the web and in mock builds there is no native side and these are no-ops.
 */
import { NativeModule, requireOptionalNativeModule } from "expo";

type GhostCallEvents = { end: () => void; tick: () => void };

declare class GhostCallNative extends NativeModule<GhostCallEvents> {
  start(title: string, body: string): void;
  update(body: string): void;
  stop(): void;
  isRunning(): boolean;
}

const native = requireOptionalNativeModule<GhostCallNative>("GhostCall");

/** Start holding the mic open in the background; call once the call is up and the app is in front. */
export function startCall(title: string, body: string) {
  native?.start(title, body);
}

/** Change the notification's second line (the line the session is on). */
export function updateCall(body: string) {
  native?.update(body);
}

export function stopCall() {
  native?.stop();
}

/** The operator tapped End on the notification. Returns the unsubscribe. */
export function onCallEnd(cb: () => void): () => void {
  const sub = native?.addListener("end", cb);
  return () => sub?.remove();
}

/**
 * Once a second while the service runs, in the background too. JavaScript's own timers stop
 * when the app leaves the screen, so anything the call must keep doing hangs off this.
 */
export function onCallTick(cb: () => void): () => void {
  const sub = native?.addListener("tick", cb);
  return () => sub?.remove();
}
