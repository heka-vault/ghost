/**
 * Pairing with the desk's phone listener (ADR 0016): the phone scans the QR code the desktop
 * app shows (a `ghost://pair` link with the code and every address), or the operator types the
 * address and the six-digit code; the desk answers with a device id and a secret, and the
 * phone keeps both in secure storage. Every later request is signed with that secret. The desk
 * pairs with one phone: pairing this one replaces any other.
 *
 * The web export and `EXPO_PUBLIC_GHOST_DESK=mock` builds pair with the mock desk instead.
 */
import { parsePairLink } from "@ghost/client-ts/pairLink";
import * as SecureStore from "expo-secure-store";
import { Platform } from "react-native";

import { fromB64url } from "./signing";
import { DEFAULT_PREFS, type Pairing, persistPrefsWith, type Prefs, stateOf, update } from "./store";

const MOCK_ADDRESS = "192.168.1.20:8765";
const PAIRING_KEY = "ghost.pairing";
const SECRET_KEY = "ghost.secret";
const PREFS_KEY = "ghost.prefs";
const PAIR_MS = 8000;

/** True when this build talks to the mock desk rather than ghostd. */
export const MOCK_DESK = Platform.OS === "web" || process.env.EXPO_PUBLIC_GHOST_DESK === "mock";

/** The pairing secret, read from secure storage at launch and held only in memory. */
let secret: Uint8Array | null = null;
export const pairingSecret = () => secret;

/** Read the saved pairing, if any, and mark the phone store ready. */
export async function loadPairing() {
  let pairing: Pairing | null = null;
  let prefs: Prefs = DEFAULT_PREFS;
  if (!MOCK_DESK) {
    try {
      const [p, s, pr] = await Promise.all([SecureStore.getItemAsync(PAIRING_KEY), SecureStore.getItemAsync(SECRET_KEY), SecureStore.getItemAsync(PREFS_KEY)]);
      if (p && s) {
        pairing = migrate(JSON.parse(p));
        secret = fromB64url(s);
      }
      if (pr) prefs = { ...DEFAULT_PREFS, ...(JSON.parse(pr) as Partial<Prefs>) };
    } catch {
      pairing = null;
    }
    persistPrefsWith((next) => void SecureStore.setItemAsync(PREFS_KEY, JSON.stringify(next)).catch(() => {}));
  }
  update({ pairing, prefs, hydrated: true });
}

/** A pairing saved before ADR 0020 had one address; keep it as the list's only entry. */
function migrate(saved: Partial<Pairing> & { address?: string }): Pairing {
  const addresses = saved.addresses?.length ? saved.addresses : saved.address ? [saved.address] : [];
  return { deviceId: saved.deviceId ?? "", workstation: saved.workstation ?? "", addresses, pairedAt: saved.pairedAt ?? new Date().toISOString() };
}

/** The desk told the phone its addresses changed (a tunnel added or removed); keep the new list. */
export async function rememberAddresses(addresses: string[]) {
  const current = stateOf().pairing;
  if (!current || sameList(current.addresses, addresses)) return;
  const pairing: Pairing = { ...current, addresses };
  update({ pairing });
  if (!MOCK_DESK) await SecureStore.setItemAsync(PAIRING_KEY, JSON.stringify(pairing));
}

const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);

/** Forget the desk on this phone. The desktop app can forget the phone on its side too. */
export async function forgetPairing() {
  secret = null;
  update({ pairing: null });
  if (!MOCK_DESK) await Promise.all([SecureStore.deleteItemAsync(PAIRING_KEY), SecureStore.deleteItemAsync(SECRET_KEY)]);
}

export function pairWithMock(address = MOCK_ADDRESS) {
  update({ pairing: { deviceId: "mock", workstation: "workstation", addresses: [address], pairedAt: new Date().toISOString() } });
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

/** Pair with the address and code the operator typed. */
export async function pairByCode(rawAddress: string, code: string): Promise<PairResult> {
  return pairAt([normalizeAddress(rawAddress)], code.replace(/\D/g, ""));
}

/**
 * Pair from the desk's QR code, or a `ghost://pair` link the phone's own camera opened
 * (docs/phone-protocol.md). Null when the text isn't a pairing link.
 */
export async function pairByLink(text: string): Promise<PairResult | null> {
  const link = parsePairLink(text);
  return link ? pairAt(link.addresses, link.code, "scan") : null;
}

/**
 * Spend the code at the first address that answers, in the desk's order (home before the
 * tunnel). Only an address that doesn't answer moves on to the next; a wrong code stops,
 * since the desk counts every try.
 */
async function pairAt(addresses: string[], digits: string, how: "type" | "scan" = "type"): Promise<PairResult> {
  if (MOCK_DESK) return pairMock(addresses[0], digits, how);
  const shown = addresses.join(" or ");
  const unreachable: PairResult = {
    ok: false,
    field: "address",
    message:
      how === "scan"
        ? `Nothing answered at ${shown}. Check that Ghost is running, and that this phone is on your home Wi-Fi or your tunnel is on.`
        : `Nothing answered at ${shown}. Check that Ghost is running with the phone listener on, and that this phone is on the same Wi-Fi.`,
  };
  const per = addresses.length > 1 ? PAIR_MS / 2 : PAIR_MS;
  for (const address of addresses) {
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), per);
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
      continue;
    } finally {
      clearTimeout(timer);
    }
    if (res.status === 403) {
      return {
        ok: false,
        field: "code",
        message: how === "scan" ? "That code ran out or was already used. Show a new code on your desk and scan it again." : "That code didn’t match, or it ran out. Show a new code on your desk and type it again.",
      };
    }
    if (!res.ok) {
      console.warn(`pair: ${address} answered ${res.status}`);
      continue;
    }
    return save(address, (await res.json()) as PairBody);
  }
  return unreachable;
}

type PairBody = { device_id: string; secret: string; workstation: string; addresses?: string[] };

async function save(address: string, body: PairBody): Promise<PairResult> {
  // The desk lists every address it serves (home first, then the tunnel); an older desk lists none.
  const addresses = body.addresses?.length ? body.addresses : [address];
  const pairing: Pairing = { deviceId: body.device_id, workstation: body.workstation, addresses, pairedAt: new Date().toISOString() };
  secret = fromB64url(body.secret);
  await Promise.all([SecureStore.setItemAsync(SECRET_KEY, body.secret), SecureStore.setItemAsync(PAIRING_KEY, JSON.stringify(pairing))]);
  update({ pairing });
  return { ok: true };
}

/** Mock rules: 000000 is a wrong code, an address starting 10.0.0. doesn't answer, anything else pairs. */
async function pairMock(address: string, digits: string, how: "type" | "scan"): Promise<PairResult> {
  await new Promise((r) => setTimeout(r, 1100));
  if (address.startsWith("10.0.0.")) {
    return { ok: false, field: "address", message: `Nothing answered at ${address}. Check that Ghost is running and that this phone is on the same Wi-Fi.` };
  }
  if (digits === "000000") {
    return { ok: false, field: "code", message: `That code didn’t match. Show a new code on your desk and ${how === "scan" ? "scan" : "type"} it again.` };
  }
  pairWithMock(address);
  return { ok: true };
}

function deviceName(): string {
  return (Platform.constants as { Model?: string }).Model || "Android phone";
}

export { MOCK_ADDRESS };
