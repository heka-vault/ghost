/**
 * Pairing with the desk's phone listener (ADR 0016): the operator types the address and the
 * six-digit code the desktop app shows, the desk answers with a device id and a secret, and
 * the phone keeps both in secure storage. Every later request is signed with that secret.
 *
 * The web export and `EXPO_PUBLIC_GHOST_DESK=mock` builds pair with the mock desk instead.
 */
import * as SecureStore from "expo-secure-store";
import { Platform } from "react-native";

import { fromB64url } from "./signing";
import { type Pairing, update } from "./store";

const MOCK_ADDRESS = "192.168.1.20:8765";
const PAIRING_KEY = "ghost.pairing";
const SECRET_KEY = "ghost.secret";
const PAIR_MS = 8000;

/** True when this build talks to the mock desk rather than ghostd. */
export const MOCK_DESK = Platform.OS === "web" || process.env.EXPO_PUBLIC_GHOST_DESK === "mock";

/** The pairing secret, read from secure storage at launch and held only in memory. */
let secret: Uint8Array | null = null;
export const pairingSecret = () => secret;

/** Read the saved pairing, if any, and mark the phone store ready. */
export async function loadPairing() {
  let pairing: Pairing | null = null;
  if (!MOCK_DESK) {
    try {
      const [p, s] = await Promise.all([SecureStore.getItemAsync(PAIRING_KEY), SecureStore.getItemAsync(SECRET_KEY)]);
      if (p && s) {
        pairing = JSON.parse(p) as Pairing;
        secret = fromB64url(s);
      }
    } catch {
      pairing = null;
    }
  }
  update({ pairing, hydrated: true });
}

/** Forget the desk on this phone. The desktop app can forget the phone on its side too. */
export async function forgetPairing() {
  secret = null;
  update({ pairing: null });
  if (!MOCK_DESK) await Promise.all([SecureStore.deleteItemAsync(PAIRING_KEY), SecureStore.deleteItemAsync(SECRET_KEY)]);
}

export function pairWithMock(address = MOCK_ADDRESS) {
  update({ pairing: { deviceId: "mock", workstation: "workstation", address, via: "Wi-Fi", pairedAt: new Date().toISOString() } });
}

/** Six digits as 3+3, the way the desk shows them: “482913” → “482 913”. */
export function groupCode(raw: string): string {
  const d = raw.replace(/\D/g, "").slice(0, 6);
  return d.length > 3 ? `${d.slice(0, 3)} ${d.slice(3)}` : d;
}

export type PairResult = { ok: true } | { ok: false; field: "code" | "address"; message: string };

/** “192.168.1.20”, “192.168.1.20:8765” or a pasted “http://…/” all mean the same desk. */
export function normalizeAddress(raw: string): string {
  const a = raw.trim().replace(/^https?:\/\//i, "").replace(/\/+$/, "");
  return /:\d+$/.test(a) ? a : `${a}:8765`;
}

export async function pairByCode(rawAddress: string, code: string): Promise<PairResult> {
  const address = normalizeAddress(rawAddress);
  const digits = code.replace(/\D/g, "");
  if (MOCK_DESK) return pairMock(address, digits);

  const unreachable: PairResult = {
    ok: false,
    field: "address",
    message: `Nothing answered at ${address}. Check that Ghost is running with the phone listener on, and that this phone is on the same Wi-Fi.`,
  };
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), PAIR_MS);
  let res: Response;
  try {
    res = await fetch(`http://${address}/pair`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code: digits, device_name: deviceName() }),
      signal: abort.signal,
    });
  } catch (e) {
    console.warn(`pair: no answer from ${address}: ${e instanceof Error ? `${e.name}: ${e.message}` : String(e)}`);
    return unreachable;
  } finally {
    clearTimeout(timer);
  }
  if (res.status === 403) {
    return { ok: false, field: "code", message: "That code didn’t match, or it ran out. Show a new code on your desk and type it again." };
  }
  if (!res.ok) {
    console.warn(`pair: ${address} answered ${res.status}`);
    return unreachable;
  }

  const body = (await res.json()) as { device_id: string; secret: string; workstation: string };
  const pairing: Pairing = { deviceId: body.device_id, workstation: body.workstation, address, via: "Wi-Fi", pairedAt: new Date().toISOString() };
  secret = fromB64url(body.secret);
  await Promise.all([SecureStore.setItemAsync(SECRET_KEY, body.secret), SecureStore.setItemAsync(PAIRING_KEY, JSON.stringify(pairing))]);
  update({ pairing });
  return { ok: true };
}

/** Mock rules: 000000 is a wrong code, an address starting 10.0.0. doesn't answer, anything else pairs. */
async function pairMock(address: string, digits: string): Promise<PairResult> {
  await new Promise((r) => setTimeout(r, 1100));
  if (address.startsWith("10.0.0.")) {
    return { ok: false, field: "address", message: `Nothing answered at ${address}. Check that Ghost is running and that this phone is on the same Wi-Fi.` };
  }
  if (digits === "000000") {
    return { ok: false, field: "code", message: "That code didn’t match. Show a new code on your desk and type it again." };
  }
  pairWithMock(address);
  return { ok: true };
}

function deviceName(): string {
  return (Platform.constants as { Model?: string }).Model || "Android phone";
}

export { MOCK_ADDRESS };
