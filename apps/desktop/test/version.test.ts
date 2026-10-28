import { afterEach, describe, expect, it, vi } from "vitest";

import { version as DESKTOP_VERSION } from "../package.json";

const WINDOW = "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER = "2222222bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/** version.ts with this window built at `commit` (what Vite's define would say). */
async function load(commit: string | null) {
  vi.resetModules();
  vi.stubGlobal("__GHOST_WINDOW_BUILD__", { commit, dirty: false });
  return import("../src/app/version");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("difference(): how the daemon differs from this window", () => {
  it("is nothing without a status or a version", async () => {
    const { difference } = await load(WINDOW);
    expect(difference(null)).toBeNull();
    expect(difference(undefined)).toBeNull();
    expect(difference({ version: "", build: { kind: "dev", commit: OTHER, dirty: false } })).toBeNull();
  });

  it("names both versions when they differ, whatever the commits", async () => {
    const { difference } = await load(WINDOW);
    expect(difference({ version: "2026.9.4", build: { kind: "deploy", commit: WINDOW, dirty: false } })).toEqual({
      kind: "version",
      ghost: "2026.9.4",
      window: DESKTOP_VERSION,
    });
  });

  it("names both commits at one version when they differ", async () => {
    const { difference } = await load(WINDOW);
    expect(difference({ version: DESKTOP_VERSION, build: { kind: "deploy", commit: OTHER, dirty: false } })).toEqual({
      kind: "commit",
      ghost: `${DESKTOP_VERSION} · 2222222`,
      window: `${DESKTOP_VERSION} · 1111111`,
    });
  });

  it("is the same build at one commit, a short hash matching its full one", async () => {
    const { difference } = await load(WINDOW);
    expect(difference({ version: DESKTOP_VERSION, build: { kind: "dev", commit: WINDOW, dirty: true } })).toBeNull();
    expect(difference({ version: DESKTOP_VERSION, build: { kind: "dev", commit: WINDOW.slice(0, 7), dirty: false } })).toBeNull();
  });

  it("never guesses: an unknown commit on either side counts as the same", async () => {
    const known = await load(WINDOW);
    expect(known.difference({ version: DESKTOP_VERSION, build: { kind: "dev", commit: null, dirty: false } })).toBeNull();
    expect(known.difference({ version: DESKTOP_VERSION } as never)).toBeNull();
    const unknown = await load(null);
    expect(unknown.difference({ version: DESKTOP_VERSION, build: { kind: "deploy", commit: OTHER, dirty: false } })).toBeNull();
  });
});
