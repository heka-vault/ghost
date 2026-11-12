/**
 * The app's request signing against the daemon's own vectors (packages/api/phone-vectors.json,
 * written by crates/ghostd/src/phone/vectors.rs): what the phone signs is what the desk checks.
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import vectors from "../../../../packages/api/phone-vectors.json";

const hexBytes = (hex: string) => Uint8Array.from(hex.match(/../g)!.map((b) => parseInt(b, 16)));

vi.mock("expo-crypto", () => ({ getRandomBytes: () => hexBytes(vectors.nonce) }));

const { b64url, fromB64url, signRequest, verifyResponse } = await import("./signing");

const secret = fromB64url(vectors.secret);

afterEach(() => {
  vi.useRealTimers();
});

describe("signing", () => {
  it("signs every request as the daemon does", () => {
    vi.useFakeTimers();
    vi.setSystemTime(Number(vectors.time) * 1000);
    for (const r of vectors.requests) {
      const { headers, nonce } = signRequest(vectors.device, secret, r.method, r.path, r.body);
      expect(nonce).toBe(vectors.nonce);
      expect(headers["X-Ghost-Time"]).toBe(vectors.time);
      expect(headers["X-Ghost-Signature"]).toBe(r.signature);
    }
  });

  it("verifies the desk's responses and drops a changed one", () => {
    for (const r of vectors.responses) {
      expect(verifyResponse(secret, r.status, vectors.nonce, vectors.time, r.signature, r.body)).toBe(true);
      expect(verifyResponse(secret, r.status + 1, vectors.nonce, vectors.time, r.signature, r.body)).toBe(false);
      expect(verifyResponse(secret, r.status, vectors.nonce, vectors.time, null, r.body)).toBe(false);
    }
  });

  it("round-trips the secret through base64url", () => {
    expect(secret.length).toBe(32);
    expect(b64url(secret)).toBe(vectors.secret);
  });
});
