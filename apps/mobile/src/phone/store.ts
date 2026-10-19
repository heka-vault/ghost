/**
 * The phone's own state: whether it is paired, its preferences, the session it opened, and
 * the last session it ended (for the summary). None of it lives on the desk. The pairing is
 * kept in secure storage (`./pairing.ts`); the rest lasts as long as the app does.
 */
import type { Turn } from "@ghost/client-ts";
import { useSyncExternalStore } from "react";

export interface EndedSession {
  sessionId: string;
  line: { slug: string; name: string };
  startedAt: string;
  endedAt: string;
  turns: Turn[];
  /** The phone couldn't tell the desk to end the session (it was unreachable). */
  endFailed: boolean;
}

export interface Pairing {
  /** The id the desk gave this phone; it signs every request with the matching secret. */
  deviceId: string;
  workstation: string;
  /** What the operator typed or scanned, shown in Settings. */
  address: string;
  via: "Wi-Fi" | "Tailscale";
  pairedAt: string;
}

export interface PhoneState {
  /** False until the saved pairing has been read, so nothing redirects to /pair too early. */
  hydrated: boolean;
  pairing: Pairing | null;
  prefs: {
    captions: boolean;
    keepAwake: boolean;
    haptics: boolean;
  };
  /** When this phone opened its mic for the current session, or null. */
  startedAt: string | null;
  muted: boolean;
  ended: EndedSession | null;
}

let state: PhoneState = {
  hydrated: false,
  pairing: null,
  prefs: { captions: false, keepAwake: true, haptics: true },
  startedAt: null,
  muted: false,
  ended: null,
};
const listeners = new Set<() => void>();

export function update(patch: Partial<PhoneState> | ((s: PhoneState) => Partial<PhoneState>)) {
  state = { ...state, ...(typeof patch === "function" ? patch(state) : patch) };
  listeners.forEach((l) => l());
}

export function setPref<K extends keyof PhoneState["prefs"]>(key: K, value: PhoneState["prefs"][K]) {
  update((s) => ({ prefs: { ...s.prefs, [key]: value } }));
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

export function usePhone(): PhoneState {
  return useSyncExternalStore(subscribe, () => state, () => state);
}
