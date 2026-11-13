import { describe, expect, it } from "vitest";

import type { SpeechModelsView } from "@ghost/client-ts";

import { resolve } from "../src/app/routes";
import { opensFirstRun } from "../src/app/setup";
import { availableLine, designedUpdate, plainNotes } from "../src/app/update";
import { speechLine } from "../src/screens/onboarding/speech";

const idle = { state: "idle" as const, done_bytes: 0, total_bytes: 0, file: null, error: null };
const view = (over: Partial<SpeechModelsView>): SpeechModelsView => ({
  ready: false,
  missing: [{ model: "Parakeet", name: "encoder-model.onnx.data", bytes: 2_435_420_160 }],
  missing_bytes: 2_903_552_740,
  folder: "/home/x/.cache/ghost/models",
  free_bytes: 100e9,
  fetch: idle,
  ...over,
});

describe("Ghost's ears and voice (ADR 0066)", () => {
  it("says the size once, and the free space only when it is short", () => {
    expect(speechLine(view({})).text).toContain("2.9 GB to download, once");
    expect(speechLine(view({})).text).not.toContain("free");
    expect(speechLine(view({ free_bytes: 1e9 })).text).toContain("There is 1.0 GB free there; it needs 2.9 GB.");
  });
  it("shows the download as it goes, and the failure in the daemon's words", () => {
    const going = speechLine(view({ fetch: { state: "fetching", done_bytes: 1.2e9, total_bytes: 2.9e9, file: "encoder-model.onnx.data", error: null } }));
    expect(going).toEqual({ state: "fetching", text: "Downloading 1.2 GB of 2.9 GB · encoder-model.onnx.data" });
    const failed = speechLine(view({ fetch: { ...idle, state: "failed", error: "Ghost could not reach huggingface.co" } }));
    expect(failed).toEqual({ state: "failed", text: "Ghost could not reach huggingface.co" });
  });
  it("is here only when nothing is missing and the daemon has loaded them", () => {
    expect(speechLine(view({ missing: [], missing_bytes: 0 })).state).toBe("loading");
    expect(speechLine(view({ missing: [], missing_bytes: 0, ready: true })).state).toBe("here");
    expect(speechLine(null).state).toBe("unknown");
  });
});

describe("updates (ADR 0066)", () => {
  it("says a new version quietly, and nothing for a development build", () => {
    expect(availableLine(designedUpdate("available", "2026.11.0"))).toBe("Update available: 2026.11.1");
    expect(availableLine(designedUpdate("current", "2026.11.0"))).toBeNull();
    expect(availableLine({ ...designedUpdate("available", "2026.11.0")!, unavailable: "dev" })).toBeNull();
    expect(designedUpdate("dev", "2026.11.0")?.installer).toBeNull();
  });
  it("shows the release notes as plain text", () => {
    const notes = "## What's changed\n* Fix by @someone in [#12](https://x/12)\n**Full Changelog**: `v1...v2`";
    expect(plainNotes(notes)).toBe("What's changed\n• Fix by @someone in #12\nFull Changelog: v1...v2");
  });
  it("the rail's link lands on the Updates section of General", () => {
    const r = resolve("/settings/general/updates");
    expect(r?.route.id).toBe("settings");
    expect(r?.params.section).toBe("updates");
  });
});

describe("a fresh install lands in the first run (ADR 0066)", () => {
  const base = { path: "/", needed: true, locked: false, welcomeSeen: false, design: false };
  it("opens it once, from Today, while Ghost can't answer", () => {
    expect(opensFirstRun(base)).toBe(true);
    expect(opensFirstRun({ ...base, welcomeSeen: true })).toBe(false);
    expect(opensFirstRun({ ...base, needed: false })).toBe(false);
    expect(opensFirstRun({ ...base, path: "/settings/general" })).toBe(false);
    expect(opensFirstRun({ ...base, locked: true })).toBe(false);
    expect(opensFirstRun({ ...base, design: true })).toBe(false);
  });
});
