/**
 * The pairing link (docs/phone-protocol.md, #36): what the desktop's QR code carries, and
 * what the phone app reads from its camera or from a `ghost://` link the system camera opened.
 *
 *   ghost://pair?code=482913&address=192.168.1.20%3A8765&address=10.8.0.4%3A8765
 *
 * The daemon builds it (`PhonePairing.link`); this builds the same string for the mock desk
 * and parses it on the phone. Parsing is by hand, not `URL`: React Native's `URL` has no
 * `searchParams`. A link that isn't exactly this shape is not a pairing link.
 */

export const PAIR_LINK = "ghost://pair";
/** More addresses than any desk serves (one home, one tunnel, a spare). */
const MAX_ADDRESSES = 8;
/** host:port, where host is a name, an IPv4 address or a bracketed IPv6 one. */
const ADDRESS = /^(?:\[[0-9A-Fa-f:.]+(?:%[\w.-]+)?\]|[A-Za-z0-9.-]+):\d{1,5}$/;

export interface PairLink {
  /** Six digits. */
  code: string;
  /** Every address the desk listens on, in its order: home first, then the tunnel. */
  addresses: string[];
}

const encode = (s: string) => encodeURIComponent(s);

/** The link for a code and the desk's addresses, as the daemon spells it. */
export function pairLink(code: string, addresses: string[]): string {
  return `${PAIR_LINK}?code=${encode(code)}${addresses.map((a) => `&address=${encode(a)}`).join("")}`;
}

/** The code and addresses in a pairing link, or null for anything else. */
export function parsePairLink(text: string): PairLink | null {
  const raw = text.trim();
  const q = raw.indexOf("?");
  if (q < 0 || raw.slice(0, q).toLowerCase().replace(/\/+$/, "") !== PAIR_LINK) return null;
  let code: string | null = null;
  const addresses: string[] = [];
  for (const part of raw.slice(q + 1).split("#")[0].split("&")) {
    if (!part) continue;
    const eq = part.indexOf("=");
    let key: string;
    let value: string;
    try {
      key = decodeURIComponent((eq < 0 ? part : part.slice(0, eq)).replace(/\+/g, " "));
      value = eq < 0 ? "" : decodeURIComponent(part.slice(eq + 1).replace(/\+/g, " "));
    } catch {
      return null;
    }
    if (key === "code") {
      if (code !== null) return null;
      code = value;
    } else if (key === "address") {
      addresses.push(value.trim());
    }
    // Other keys are left for later versions of the link.
  }
  if (code === null || !/^\d{6}$/.test(code)) return null;
  if (addresses.length === 0 || addresses.length > MAX_ADDRESSES) return null;
  if (!addresses.every((a) => a.length <= 100 && ADDRESS.test(a))) return null;
  return { code, addresses };
}
