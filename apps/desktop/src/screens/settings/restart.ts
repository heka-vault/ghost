/** Owner: settings. Which restart the running Ghost needs, from what it says it is (docs/releasing.md). */
import type { Status } from "@ghost/client-ts";

export const RESTART_COMMAND = "systemctl --user restart ghostd";
/** A checkout's daemon (make dev, make daemon): restarting the service instead would start a second Ghost. */
export const DEV_RESTART_COMMAND = "make reload";
/** A checkout's daemon make start left in the background. */
export const START_RESTART_COMMAND = "make restart";
/** A checkout's daemon started some other way (a unit running the checkout, cargo run): no one command is right. */
export const RESTART_WORDS = "Restart Ghost the way you started it (make reload under make dev).";

/** How to restart the Ghost this window talks to, by its build: the service for a deploy or a
 *  release, or when unknown; for a checkout, the make target that started it (`build.launcher`),
 *  and null when it didn't say, for RESTART_WORDS instead of a command. */
export function restartCommand(build: Status["build"] | null | undefined): string | null {
  if (build?.kind !== "dev") return RESTART_COMMAND;
  if (build.launcher === "make-dev") return DEV_RESTART_COMMAND;
  if (build.launcher === "make-start") return START_RESTART_COMMAND;
  return null;
}
