/**
 * The phone's own state, kept for the page's life: its preferences, whether it thinks it is
 * driving, and the last session it ended (for the summary). None of it has an API yet; on a
 * real phone it would live in the app's storage. A tiny external store so every screen
 * reads the same values.
 */
import type { Turn } from "@ghost/client-ts";
import { useSyncExternalStore } from "react";

import type { MobileFixture } from "../../design/mock/fixtures/mobile";

export interface EndedSession {
  sessionId: string;
  line: { slug: string; name: string };
  startedAt: string;
  endedAt: string;
  turns: Turn[];
  /** The phone couldn't tell the desk to end the session (it was unreachable). */
  endFailed: boolean;
}

export interface PhoneState {
  prefs: MobileFixture["prefs"];
  motion: MobileFixture["motion"];
  paired: boolean;
  /** When this phone opened its mic for the current session (Start), or null. */
  startedAt: string | null;
  /** The phone's mic is muted for this session. Phone-local: mic routing has no API yet. */
  muted: boolean;
  ended: EndedSession | null;
}

let state: PhoneState | null = null;
const listeners = new Set<() => void>();

/** Seeds the store once from the fixture, then from ?motion=parked, ?captions=1 and ?muted=1 for screenshots. */
export function seed(f: MobileFixture) {
  if (state) return;
  const q = new URLSearchParams(window.location.search);
  state = {
    prefs: { ...f.prefs, captions: q.has("captions") ? q.get("captions") !== "0" : f.prefs.captions },
    motion: q.get("motion") === "parked" ? "parked" : f.motion,
    paired: q.has("paired") ? q.get("paired") !== "0" : f.pairing.paired,
    startedAt: null,
    muted: q.get("muted") === "1",
    ended: null,
  };
}

export function update(patch: Partial<PhoneState> | ((s: PhoneState) => Partial<PhoneState>)) {
  if (!state) return;
  state = { ...state, ...(typeof patch === "function" ? patch(state) : patch) };
  listeners.forEach((l) => l());
}

export function setPref<K extends keyof PhoneState["prefs"]>(key: K, value: PhoneState["prefs"][K]) {
  update((s) => ({ prefs: { ...s.prefs, [key]: value } }));
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function usePhone(): PhoneState | null {
  return useSyncExternalStore(subscribe, () => state, () => state);
}
