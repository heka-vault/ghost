/**
 * The phone's own state: whether it is paired, its preferences and the session it opened.
 * None of it lives on the desk. The pairing is
 * kept in secure storage (`./pairing.ts`); the rest lasts as long as the app does.
 */
import { useSyncExternalStore } from "react";

export interface Pairing {
  /** The id the desk gave this phone; it signs every request with the matching secret. */
  deviceId: string;
  workstation: string;
  /**
   * Every address the desk listens on, as it reported them at pairing: the home Wi-Fi one
   * first, then the tunnel one for away (ADR 0020). The desk picks whichever answers.
   */
  addresses: string[];
  pairedAt: string;
}

export interface Prefs {
  captions: boolean;
  keepAwake: boolean;
  haptics: boolean;
  /** Open the mic as soon as the app comes up and the desk answers: "Hey Google, open Ghost". */
  autoStart: boolean;
}

export const DEFAULT_PREFS: Prefs = { captions: false, keepAwake: true, haptics: true, autoStart: false };

export interface PhoneState {
  /** False until the saved pairing has been read, so nothing redirects to /pair too early. */
  hydrated: boolean;
  pairing: Pairing | null;
  prefs: Prefs;
  /** When this phone opened its mic for the current session, or null. */
  startedAt: string | null;
  muted: boolean;
}

let state: PhoneState = {
  hydrated: false,
  pairing: null,
  prefs: DEFAULT_PREFS,
  startedAt: null,
  muted: false,
};
const listeners = new Set<() => void>();

/** The current state, for code that runs outside React. */
export const stateOf = () => state;

export function update(patch: Partial<PhoneState> | ((s: PhoneState) => Partial<PhoneState>)) {
  state = { ...state, ...(typeof patch === "function" ? patch(state) : patch) };
  listeners.forEach((l) => l());
}

/** Set by the pairing code once storage is available; preferences outlive the app then. */
let persistPrefs: (prefs: Prefs) => void = () => {};
export function persistPrefsWith(fn: (prefs: Prefs) => void) {
  persistPrefs = fn;
}

export function setPref<K extends keyof Prefs>(key: K, value: Prefs[K]) {
  update((s) => ({ prefs: { ...s.prefs, [key]: value } }));
  persistPrefs(state.prefs);
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
