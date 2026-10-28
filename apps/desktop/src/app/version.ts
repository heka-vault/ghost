/**
 * The desktop app's own version: its package.json's, which scripts/version.py keeps the same
 * as tauri.conf.json, Cargo.toml and every other copy (docs/releasing.md), and the commit it
 * was built from, which vite.config.ts reads from git when it builds or serves the window. The
 * daemon says its own in the status; the two differ when, say, a window from a checkout talks
 * to the deployed Ghost, and the sidebar and Settings › General say so quietly.
 */
import type { Status } from "@ghost/client-ts";

import { version } from "../../package.json";

/** Set by Vite's `define` (vite.config.ts); absent where Vite didn't build the module. */
declare const __GHOST_WINDOW_BUILD__: WindowBuild | undefined;

export interface WindowBuild {
  /** The checkout's HEAD when the window was built, or the dev server started; null outside git. */
  commit: string | null;
  /** Tracked files had uncommitted changes then. */
  dirty: boolean;
}

export const DESKTOP_VERSION: string = version;
export const DESKTOP_BUILD: WindowBuild =
  typeof __GHOST_WINDOW_BUILD__ !== "undefined" && __GHOST_WINDOW_BUILD__ ? __GHOST_WINDOW_BUILD__ : { commit: null, dirty: false };

const short = (commit: string) => commit.slice(0, 7);

/** This window as the sidebar and General name it: "2026.10.1", or "2026.10.1 · a1b2c3d". */
export function windowLine(withCommit: boolean): string {
  return withCommit && DESKTOP_BUILD.commit ? `${DESKTOP_VERSION} · ${short(DESKTOP_BUILD.commit)}` : DESKTOP_VERSION;
}

/** How the daemon differs from this window: each side in words, the version alone when that differs. */
export interface Difference {
  kind: "version" | "commit";
  ghost: string;
  window: string;
}

/**
 * The daemon and this window, when they aren't the same build: a different version, or the
 * same version at different commits when both know theirs (a `make dev` window on the deployed
 * Ghost). Unknown counts as the same, so the notice is never a guess. Uncommitted changes
 * aren't compared: two dirty trees at one commit can't be told apart from here.
 */
export function difference(status: Pick<Status, "version" | "build"> | null | undefined): Difference | null {
  const theirs = status?.version;
  if (!theirs) return null;
  if (theirs !== DESKTOP_VERSION) return { kind: "version", ghost: theirs, window: DESKTOP_VERSION };
  const a = status?.build?.commit;
  const b = DESKTOP_BUILD.commit;
  if (!a || !b) return null;
  const n = Math.min(a.length, b.length);
  if (n < 7 || a.slice(0, n) === b.slice(0, n)) return null;
  return { kind: "commit", ghost: `${theirs} · ${short(a)}`, window: windowLine(true) };
}
