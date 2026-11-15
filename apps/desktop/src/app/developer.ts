/**
 * Settings › Developer (ADR 0073): shown in the settings nav once the version on About has
 * been clicked seven times, and remembered on this computer until it is switched off on the
 * Developer page. It is not a lock and never said to be one: everything behind it is
 * something the operator can already do with a file or a terminal, and every page under it is
 * reachable by its address whether or not it is shown.
 *
 * The count is plain logic (`tap`), so a test runs it without a page. The remembered choice
 * is a per-viewer convenience in localStorage, read and written in try/catch: a private
 * window or blocked storage just starts with it hidden.
 */
import { useSyncExternalStore } from "react";

/** Clicks on the version that turn it on. */
export const UNLOCK_TAPS = 7;
/** A pause longer than this between two clicks starts the count again. */
export const TAP_GAP_MS = 2000;
/** The count says how many are left from this many on. */
export const HINT_FROM = 4;

const KEY = "ghost.settings.developer";
/** Developer's live timing overlay (ADR 0073 as amended 2026-11-14), on this computer. */
export const TIMING_KEY = "ghost.developer.timing";

export interface Taps {
  count: number;
  at: number;
}

export const NO_TAPS: Taps = { count: 0, at: 0 };

/**
 * One more click (or Enter or Space) on the version at `now`: the new count, whether it has
 * just turned Developer on, and the words a screen reader hears (null for none).
 */
export function tap(prev: Taps, now: number, unlocked: boolean): { taps: Taps; unlock: boolean; hint: string | null } {
  if (unlocked) return { taps: NO_TAPS, unlock: false, hint: "Developer settings are already on, under Settings." };
  const count = prev.count > 0 && now - prev.at <= TAP_GAP_MS ? prev.count + 1 : 1;
  if (count >= UNLOCK_TAPS) return { taps: NO_TAPS, unlock: true, hint: "Developer settings are on. They are under Settings › Developer." };
  const left = UNLOCK_TAPS - count;
  const hint = count >= HINT_FROM ? `${left} more ${left === 1 ? "click" : "clicks"} for developer settings.` : null;
  return { taps: { count, at: now }, unlock: false, hint };
}

let on = read();
let timing = read(TIMING_KEY);
const listeners = new Set<() => void>();

function read(key = KEY): boolean {
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem(key) === "on";
  } catch {
    return false;
  }
}

function write(key: string, next: boolean) {
  try {
    if (next) localStorage.setItem(key, "on");
    else localStorage.removeItem(key);
  } catch {
    // Remembered for this window only.
  }
}

export function developerOn(): boolean {
  return on;
}

/** Show Developer in the settings nav, or hide it again. Hiding it turns the timing overlay off too. */
export function setDeveloper(next: boolean) {
  on = next;
  write(KEY, next);
  if (!next) {
    timing = false;
    write(TIMING_KEY, false);
  }
  for (const l of listeners) l();
}

/** The timing overlay is shown: switched on, and Developer is on. */
export function timingOverlayOn(): boolean {
  return on && timing;
}

/** Show the timing overlay, or hide it. Remembered on this computer until Developer is hidden. */
export function setTimingOverlay(next: boolean) {
  timing = next && on;
  write(TIMING_KEY, timing);
  for (const l of listeners) l();
}

/**
 * Read both again from storage: the floating view is a second process of the app, which sees
 * the main window's choice when it next looks (on focus, and every few seconds while it runs).
 */
export function rereadDeveloper() {
  const a = read();
  const b = read(TIMING_KEY);
  if (a === on && b === timing) return;
  on = a;
  timing = b;
  for (const l of listeners) l();
}

export function useTimingOverlay(): boolean {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    timingOverlayOn,
    () => false,
  );
}

export function useDeveloper(): boolean {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => on,
    () => false,
  );
}
