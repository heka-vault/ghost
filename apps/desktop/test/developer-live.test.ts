/**
 * Developer's live views (ADR 0073 as amended 2026-11-14): the timing overlay's rows and its
 * copied table from turns' records, the meter's decisions in words (never anyone's words), the
 * overlay's switch going off with Developer, and the mock meter's rate and close.
 */
import type { MeterView, TimingView } from "@ghost/client-ts";
import { createDevLive, decisionWords, liveRow, modelLine, rowOf, TIMINGS_KEPT, timingTable } from "@ghost/ui";
import { afterEach, describe, expect, it, vi } from "vitest";

import { meterAt, MockLive, seededTimings } from "../src/design/mock/live";

/** A tool turn as `ghost_api::live::timing_of` builds it from the latency record its test uses. */
const TOOL_TURN: TimingView = {
  session_id: "s1",
  at: "2026-11-14T10:00:05Z",
  turn: 3,
  kind: "voice",
  model: "claude-haiku-5-5",
  provider: "openrouter",
  fast: true,
  handover: true,
  closed: 0.62,
  stt: 0.18,
  first_word: 0.96,
  first_tool: 1.05,
  tool_result: 1.47,
  ack: 1.2,
  first_sound: 2.41,
  reply_end: 5.91,
  barge_in: null,
  calls: 2,
  tools: 1,
};

describe("the timing overlay's rows", () => {
  it("shows each stage of a tool turn and the model it ran on", () => {
    const r = rowOf(TOOL_TURN);
    expect(r.cells).toEqual({ closed: "0.62", stt: "0.18", first_word: "0.96", first_tool: "1.05", tool_result: "1.47", first_sound: "2.41", reply_end: "5.91" });
    expect(r.model).toBe("haiku-5-5 · openrouter · fast→main");
    expect(r.cut).toBe(false);
  });

  it("marks a stage the turn didn't have with a dash, and a cut reply", () => {
    const typed: TimingView = { ...TOOL_TURN, kind: "text", closed: null, stt: null, first_tool: null, tool_result: null, fast: false, handover: false, barge_in: 0.4, model: "anthropic/claude-opus-5-5-20261101" };
    const r = rowOf(typed);
    expect(r.cells.closed).toBe("–");
    expect(r.cells.first_tool).toBe("–");
    expect(r.cut).toBe(true);
    expect(modelLine(typed)).toBe("opus-5-5 · openrouter · main");
  });

  it("builds the turn under way from the activity, until its record is in", () => {
    const act = [
      { kind: "you_started" as const, session_id: "s1", at: "2026-11-14T10:01:00.000Z" },
      { kind: "you_stopped" as const, session_id: "s1", at: "2026-11-14T10:01:02.000Z" },
      { kind: "call" as const, session_id: "s1", at: "2026-11-14T10:01:03.250Z", name: "github_issue_read", id: "c1" },
    ];
    const live = liveRow(act, TOOL_TURN.at);
    expect(live?.live).toBe(true);
    expect(live?.cells.first_tool).toBe("1.25");
    expect(live?.cells.tool_result).toBe("…");
    // The record of this turn arrived: no row under way.
    expect(liveRow(act, "2026-11-14T10:01:09Z")).toBeNull();
  });

  it("copies the last turns as a plain table of numbers", () => {
    const text = timingTable([TOOL_TURN, { ...TOOL_TURN, turn: 4, barge_in: 0.3 }]);
    const lines = text.split("\n");
    expect(lines[1]).toMatch(/^turn\s+kind\s+closed\s+stt\s+word\s+tool\s+result\s+sound\s+end\s+model$/);
    expect(lines[2]).toContain("0.62");
    expect(lines[2]).toContain("5.91");
    expect(lines[3]).toContain("· cut");
    expect(lines).toHaveLength(4);
  });

  it("keeps the last ten turns, each once", () => {
    const dev = createDevLive();
    for (let i = 1; i <= 14; i++) dev.pushTiming({ ...TOOL_TURN, turn: i, at: `2026-11-14T10:00:${String(i).padStart(2, "0")}Z` });
    dev.pushTiming({ ...TOOL_TURN, turn: 14, at: "2026-11-14T10:00:14Z" });
    expect(dev.timings()).toHaveLength(TIMINGS_KEPT);
    expect(dev.timings()[0].turn).toBe(5);
  });
});

describe("the meter says decisions by kind, never words", () => {
  it("has a sentence for every kind, with only its numbers", () => {
    const all: MeterView["decisions"] = [
      { kind: "taken", ago_ms: 1, words: 7, dbfs: -24.2 },
      { kind: "no_words", ago_ms: 1, secs: 0.4, dbfs: -50 },
      { kind: "held_room", ago_ms: 1, dbfs: -41, person: -22 },
      { kind: "held_room", ago_ms: 1, dbfs: -41, person: null },
      { kind: "let_go", ago_ms: 1, secs: 1.6, dbfs: -41 },
      { kind: "taken_by_name", ago_ms: 1 },
      { kind: "echo", ago_ms: 1 },
      { kind: "too_short", ago_ms: 1 },
      { kind: "barge_in", ago_ms: 1 },
      { kind: "wake", ago_ms: 1, score: 0.81, woke: false },
      { kind: "near_miss", ago_ms: 1, score: 0.27 },
    ];
    for (const d of all) expect(decisionWords(d)).toMatch(/\S/);
    expect(decisionWords(all[0])).toBe("Taken as your turn: 7 words at -24 dBFS");
    expect(decisionWords(all[2])).toBe("Held as the room: -41 dBFS, you at -22 dBFS");
    expect(decisionWords(all[9])).toBe("Wake attempt, score 0.81: refused by the check");
  });

  it("can't show text a hostile reading carries: a field it doesn't know is never read", () => {
    const hostile = { kind: "taken", ago_ms: 1, words: 2, dbfs: -20, text: "SECRET-WORDS", transcript: "SECRET-WORDS" } as unknown as MeterView["decisions"][number];
    expect(decisionWords(hostile)).not.toContain("SECRET");
  });
});

describe("the mock meter (synthetic rooms)", () => {
  it("plays a quiet room, a talker held, and Ghost's echo on the array", () => {
    const kinds = (room: "quiet" | "talker" | "echo") => {
      const seen = new Set<string>();
      for (let ms = 0; ms < 8000; ms += 67) for (const d of meterAt(room, ms, true, false).decisions) seen.add(d.kind);
      return seen;
    };
    expect(kinds("quiet").has("taken")).toBe(true);
    expect(kinds("talker")).toEqual(new Set(["held_room", "let_go", "taken"]));
    expect(kinds("echo").has("echo") && kinds("echo").has("barge_in")).toBe(true);
    const array = meterAt("echo", 100, true, false);
    expect(array.room_gate).toMatchObject({ on: false, why: "on_chip_array" });
  });

  it("shows nothing measured with listening off", () => {
    const m = meterAt("quiet", 2500, true, true);
    expect(m.state).toBe("listening_off");
    expect(m.level).toBeNull();
    expect(m.decisions).toEqual([]);
  });

  it("sends about fifteen a second while held, and stops with one closing reading", () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    const got: MeterView[] = [];
    const live = new MockLive("quiet", (e) => e.type === "meter" && got.push(e.meter), () => ({ session_id: "s1", not_listening: null }) as never);
    live.handle("PUT", "/developer/meter", { open: true });
    vi.advanceTimersByTime(1000);
    expect(got.length).toBeGreaterThanOrEqual(14);
    expect(got.length).toBeLessThanOrEqual(16);
    live.handle("PUT", "/developer/meter", { open: false });
    expect(got[got.length - 1].open).toBe(false);
    const n = got.length;
    vi.advanceTimersByTime(2000);
    expect(got.length).toBe(n);
  });

  it("seeds a session's turns for the overlay", () => {
    expect(seededTimings("s9").every((t) => t.session_id === "s9")).toBe(true);
  });
});

describe("the overlay's switch", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  it("is off by default, remembered, and off when Developer is hidden", async () => {
    const store = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v), removeItem: (k: string) => store.delete(k) });
    vi.resetModules();
    const d = await import("../src/app/developer");
    expect(d.timingOverlayOn()).toBe(false);
    d.setTimingOverlay(true);
    expect(d.timingOverlayOn()).toBe(false); // Developer isn't on
    d.setDeveloper(true);
    d.setTimingOverlay(true);
    expect(d.timingOverlayOn()).toBe(true);
    expect(store.get(d.TIMING_KEY)).toBe("on");
    d.setDeveloper(false);
    expect(d.timingOverlayOn()).toBe(false);
    expect(store.has(d.TIMING_KEY)).toBe(false);
    d.setDeveloper(true);
    expect(d.timingOverlayOn()).toBe(false); // starts off again
  });
});

afterEach(() => {
  vi.useRealTimers();
});
