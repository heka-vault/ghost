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
}

let state: PhoneState = {
  hydrated: false,
  pairing: null,
  prefs: { captions: false, keepAwake: true, haptics: true },
  startedAt: null,
  muted: false,
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
