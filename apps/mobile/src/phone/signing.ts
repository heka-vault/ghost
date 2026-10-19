/**
 * Request and response signatures for the desk's phone listener (docs/phone-protocol.md).
 * HMAC-SHA256 under the secret from pairing; the secret itself never goes over the wire again.
 */
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, utf8ToBytes } from "@noble/hashes/utils.js";
import { getRandomBytes } from "expo-crypto";

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/** base64url without padding. */
export function b64url(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const n = (bytes[i] << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63];
    if (i + 1 < bytes.length) out += B64[(n >> 6) & 63];
    if (i + 2 < bytes.length) out += B64[n & 63];
  }
  return out;
}

export function fromB64url(text: string): Uint8Array {
  const clean = text.replace(/=+$/, "");
  const out: number[] = [];
  let bits = 0;
  let acc = 0;
  for (const ch of clean) {
    const v = B64.indexOf(ch === "+" ? "-" : ch === "/" ? "_" : ch);
    if (v < 0) throw new Error("bad base64url");
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((acc >> bits) & 0xff);
    }
  }
  return Uint8Array.from(out);
}

const bodyHash = (body: string) => bytesToHex(sha256(utf8ToBytes(body)));

function mac(secret: Uint8Array, lines: string[]): string {
  return b64url(hmac(sha256, secret, utf8ToBytes(lines.join("\n"))));
}

/** Constant-time compare of two signatures. */
function same(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

export interface Signed {
  headers: Record<string, string>;
  nonce: string;
}

export function signRequest(deviceId: string, secret: Uint8Array, method: string, pathAndQuery: string, body: string): Signed {
  const time = String(Math.floor(Date.now() / 1000));
  const nonce = bytesToHex(getRandomBytes(16));
  const signature = mac(secret, ["ghost-v1", method.toUpperCase(), pathAndQuery, time, nonce, bodyHash(body)]);
  return {
    nonce,
    headers: {
      "X-Ghost-Device": deviceId,
      "X-Ghost-Time": time,
      "X-Ghost-Nonce": nonce,
      "X-Ghost-Signature": signature,
    },
  };
}

/** Whether a response really came from the paired desk, in answer to this request. */
export function verifyResponse(secret: Uint8Array, status: number, nonce: string, time: string | null, signature: string | null, body: string): boolean {
  if (!time || !signature) return false;
  return same(signature, mac(secret, ["ghost-v1-response", String(status), nonce, time, bodyHash(body)]));
}
