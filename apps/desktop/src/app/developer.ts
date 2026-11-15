/**
 * Settings › Developer (ADR 0072): shown in the settings nav once the version on About has
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
const listeners = new Set<() => void>();

function read(): boolean {
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem(KEY) === "on";
  } catch {
    return false;
  }
}

export function developerOn(): boolean {
  return on;
}

/** Show Developer in the settings nav, or hide it again. */
export function setDeveloper(next: boolean) {
  on = next;
  try {
    if (next) localStorage.setItem(KEY, "on");
    else localStorage.removeItem(KEY);
  } catch {
    // Remembered for this window only.
  }
  for (const l of listeners) l();
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
