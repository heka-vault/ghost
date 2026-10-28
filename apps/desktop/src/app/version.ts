/**
 * The desktop app's own version: its package.json's, which scripts/version.py keeps the same
 * as tauri.conf.json, Cargo.toml and every other copy (docs/releasing.md). The daemon says
 * its own in the status; the two differ when, say, a window from a checkout talks to the
 * deployed Ghost, and the sidebar and Settings › General say so quietly.
 */
import { version } from "../../package.json";

export const DESKTOP_VERSION: string = version;

/** The daemon's version when it isn't this window's, else null (unknown counts as the same). */
export function otherVersion(daemonVersion: string | null | undefined): string | null {
  return daemonVersion && daemonVersion !== DESKTOP_VERSION ? daemonVersion : null;
}
