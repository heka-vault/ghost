import type { HearPlan, HearSession, HearTake, HearView } from "@ghost/client-ts";
import { createHearLive } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { dbfs, entriesOf, lineParts, moved, nextTake, pct, progressOf, refusal, speechSpan, splitCode, takeOfKey, takeStatus, freeTakeId } from "../src/screens/hear/model";
import { IDLE, MIN_HOLD_MS, type Press, type PressContext, endingFor, press, shortcutFor } from "../src/screens/hear/press";

const ready: PressContext = { state: "ready", take: "A01", ending: "hold", canStart: true };

describe("hold to talk", () => {
  it("starts a held take on the key going down and ends it on the key going up", () => {
    const down = press(IDLE, { type: "down", source: "key", at: 0 }, ready);
    expect(down.call).toEqual({ call: "start", take: "A01", ending: "hold" });
    expect(down.next.held?.source).toBe("key");
    const up = press(down.next, { type: "up", source: "key", at: 2000 }, { ...ready, state: "recording" });
    expect(up.call).toEqual({ call: "stop" });
    expect(up.next).toEqual(IDLE);
  });

  it("ignores the key's repeats while it is held", () => {
    const down = press(IDLE, { type: "down", source: "key", at: 0 }, ready);
    const again = press(down.next, { type: "down", source: "key", at: 40, repeat: true }, { ...ready, state: "recording" });
    expect(again.call).toBeNull();
    expect(again.next).toBe(down.next);
  });

  it("ignores a second source while one holds, and that source's release", () => {
    const down = press(IDLE, { type: "down", source: "pointer", at: 0 }, ready);
    expect(press(down.next, { type: "down", source: "key", at: 100 }, { ...ready, state: "recording" }).call).toBeNull();
    expect(press(down.next, { type: "up", source: "key", at: 900 }, { ...ready, state: "recording" }).call).toBeNull();
    expect(press(down.next, { type: "up", source: "pointer", at: 900 }, { ...ready, state: "recording" }).call).toEqual({ call: "stop" });
  });

  it("throws a tap away instead of keeping it as a take", () => {
    const down = press(IDLE, { type: "down", source: "key", at: 0 }, ready);
    expect(press(down.next, { type: "up", source: "key", at: MIN_HOLD_MS - 1 }, ready).call).toEqual({ call: "cancel", short: true });
  });

  it("ends the take when the window loses focus mid-hold", () => {
    const down = press(IDLE, { type: "down", source: "pointer", at: 0 }, ready);
    const blur = press(down.next, { type: "blur", at: 3000 }, { ...ready, state: "recording" });
    expect(blur.call).toEqual({ call: "stop" });
    expect(blur.next).toEqual(IDLE);
    expect(press(IDLE, { type: "blur", at: 3000 }, ready).call).toBeNull();
  });

  it("cancels with Escape, held or counting down, and does nothing between takes", () => {
    const down = press(IDLE, { type: "down", source: "key", at: 0 }, ready);
    expect(press(down.next, { type: "escape" }, { ...ready, state: "recording" })).toEqual({ next: IDLE, call: { call: "cancel" } });
    expect(press(IDLE, { type: "escape" }, { ...ready, state: "countdown", ending: "quiet" }).call).toEqual({ call: "cancel" });
    expect(press(IDLE, { type: "escape" }, ready).call).toBeNull();
    expect(press(IDLE, { type: "escape" }, { ...ready, state: "checking" }).call).toBeNull();
  });

  it("starts a take across the room or a silent one with a single press, and its release does nothing", () => {
    for (const ending of ["quiet", "timed"] as const) {
      const ctx = { ...ready, ending };
      const down = press(IDLE, { type: "down", source: "key", at: 0 }, ctx);
      expect(down.call).toEqual({ call: "start", take: "A01", ending });
      expect(down.next).toEqual(IDLE);
      expect(press(down.next, { type: "up", source: "key", at: 50 }, { ...ctx, state: "countdown" }).call).toBeNull();
    }
  });

  it("starts nothing unless the recorder is ready and the screen allows it", () => {
    const m: Press = IDLE;
    expect(press(m, { type: "down", source: "key", at: 0 }, { ...ready, canStart: false }).call).toBeNull();
    expect(press(m, { type: "down", source: "key", at: 0 }, { ...ready, state: "checking" }).call).toBeNull();
    expect(press(m, { type: "down", source: "key", at: 0 }, { ...ready, state: "closed" }).call).toBeNull();
    expect(press(m, { type: "down", source: "key", at: 0 }, { ...ready, take: null }).call).toBeNull();
  });

  it("ends a take the way the daemon expects", () => {
    expect(endingFor("speech", false)).toBe("hold");
    expect(endingFor("reply", undefined)).toBe("hold");
    expect(endingFor("speech", true)).toBe("quiet");
    expect(endingFor("silence", true)).toBe("timed");
    expect(endingFor("silence", false)).toBe("timed");
  });

  it("maps the single keys between takes only", () => {
    const ctx = { state: "ready" as const, checked: true, recorded: true };
    expect(shortcutFor("Enter", ctx)).toBe("next");
    expect(shortcutFor("r", ctx)).toBe("redo");
    expect(shortcutFor("P", ctx)).toBe("play");
    expect(shortcutFor("s", ctx)).toBe("skip");
    expect(shortcutFor("r", { ...ctx, checked: false })).toBeNull();
    expect(shortcutFor("p", { ...ctx, recorded: false })).toBeNull();
    expect(shortcutFor("Enter", { ...ctx, state: "recording" })).toBeNull();
  });
});

const plan: HearPlan = {
  version: 1,
  blocks: [
    { block: "A", condition: "desk", takes: [{ id: "A01", kind: "speech", text: "One." }, { id: "A02", kind: "speech", text: "Two." }, { id: "A03", kind: "speech", text: "Three." }] },
    { block: "H", condition: "desk-silence", takes: [{ id: "H01", kind: "silence", say: "(sit still)", seconds: 6 }] },
  ],
};
const take = (id: string, patch: Partial<HearTake> = {}): HearTake => ({
  id, block: id[0], kind: "speech", text: "", state: "recorded", seconds: 3, peak: -12, speech_start: 0.5, speech_end: 2.4, ending: "hold", warnings: [], raw: false, time: null, ...patch,
});
const session = (takes: HearTake[]): HearSession => ({ name: "2026-11-06", takes });

describe("the take list", () => {
  it("lists every take in order with its block", () => {
    const all = entriesOf(plan);
    expect(all.map((e) => e.take.id)).toEqual(["A01", "A02", "A03", "H01"]);
    expect(all.map((e) => e.first)).toEqual([true, false, false, true]);
    expect(all[3].block.block).toBe("H");
  });

  it("goes to the next take not yet recorded, then wraps to the top", () => {
    expect(nextTake(plan, session([]), null)).toBe("A01");
    expect(nextTake(plan, session([take("A01")]), "A01")).toBe("A02");
    expect(nextTake(plan, session([take("A01"), take("A02", { state: "skipped" })]), "A01")).toBe("A03");
    expect(nextTake(plan, session([take("A03"), take("H01")]), "H01")).toBe("A01");
    expect(nextTake(plan, session([take("A01"), take("A02"), take("A03"), take("H01")]), "A02")).toBeNull();
    // a flagged take is still recorded: the operator redoes it on purpose
    expect(nextTake(plan, session([take("A01", { warnings: ["cut_off"] })]), null)).toBe("A02");
  });

  it("calls a take flagged only for the warnings that ask for a redo", () => {
    expect(takeStatus(undefined)).toBe("todo");
    expect(takeStatus(take("A01"))).toBe("recorded");
    expect(takeStatus(take("A01", { warnings: ["too_quiet", "missed_end"] }))).toBe("recorded");
    for (const w of ["no_speech", "started_early", "cut_off", "clipping", "speech_in_silence"]) expect(takeStatus(take("A01", { warnings: [w] }))).toBe("flagged");
    expect(takeStatus(take("A01", { state: "skipped", warnings: ["cut_off"] }))).toBe("skipped");
  });

  it("counts a session against the take list", () => {
    expect(progressOf(plan, session([take("A01"), take("A02", { warnings: ["clipping"] }), take("A03", { state: "skipped" })]))).toEqual({ total: 4, recorded: 2, skipped: 1, flagged: 1, left: 1 });
  });

  it("splits a line at its pauses", () => {
    expect(lineParts("Read me issue four one three ... from the acme research project.")).toEqual(["Read me issue four one three", "from the acme research project."]);
    expect(lineParts("I noticed that when I pause ... in the middle ... you cut me off.")).toHaveLength(3);
    expect(lineParts("Stop.")).toEqual(["Stop."]);
    expect(lineParts("")).toEqual([]);
  });

  it("moves an item and keeps the list when it can't", () => {
    expect(moved([1, 2, 3], 0, 1)).toEqual([2, 1, 3]);
    expect(moved([1, 2, 3], 2, -2)).toEqual([3, 1, 2]);
    const l = [1, 2];
    expect(moved(l, 0, -1)).toBe(l);
  });

  it("finds a free take id in a block", () => {
    expect(freeTakeId(plan, "A")).toBe("A04");
    expect(freeTakeId(plan, "B")).toBe("B01");
  });
});

describe("the words a report and a check show", () => {
  it("splits code out of a report's text", () => {
    expect(splitCode("`stage` against `turn`: what changes")).toEqual([
      { code: true, text: "stage" },
      { code: false, text: " against " },
      { code: true, text: "turn" },
      { code: false, text: ": what changes" },
    ]);
    expect(splitCode("no code")).toEqual([{ code: false, text: "no code" }]);
    expect(splitCode("an `unclosed tick")).toEqual([{ code: false, text: "an `unclosed tick" }]);
  });

  it("writes a word error rate as a percent", () => {
    expect(pct(0.072)).toBe("7.2%");
    expect(pct(0)).toBe("0.0%");
    expect(pct(1)).toBe("100.0%");
    expect(pct(null)).toBe("–");
    expect(pct(Number.NaN)).toBe("–");
  });

  it("writes a level in dBFS with a real minus", () => {
    expect(dbfs(-12.4)).toBe("−12 dBFS");
    expect(dbfs(0)).toBe("0 dBFS");
    expect(dbfs(null)).toBe("–");
  });

  it("places the speech in a take", () => {
    expect(speechSpan(0.5, 2.5, 5)).toEqual({ from: 0.1, to: 0.5 });
    expect(speechSpan(0.5, 9, 5)).toEqual({ from: 0.1, to: 1 });
    expect(speechSpan(null, 2, 5)).toBeNull();
    expect(speechSpan(1, 2, 0)).toBeNull();
  });

  it("reads the take out of a report row's key", () => {
    expect(takeOfKey("A01|take")).toBe("A01");
    expect(takeOfKey("B07|kokoro:bm_george")).toBe("B07");
    expect(takeOfKey(null)).toBeNull();
  });

  it("says what the daemon refused", () => {
    expect(refusal({ status: 422, detail: "take A01 is in the plan twice", message: "x" })).toBe("take A01 is in the plan twice");
    expect(refusal({ status: 422, detail: [{ loc: ["body", "blocks", 0, "takes"], msg: "missing field" }] })).toBe("blocks.0.takes: missing field");
    expect(refusal(new Error("boom"))).toBe("boom");
  });
});

const view = (patch: Partial<HearView> = {}): HearView => ({
  state: "recording", session: "2026-11-06", take: "A01", ending: "hold", countdown: null, elapsed: 0, raw_device: null, unavailable: null, scoring: null, playing: false, checked: null, problem: null, ...patch,
});

describe("the recorder's store", () => {
  it("tells the screen only when something but the running time changes", () => {
    const live = createHearLive();
    let changes = 0;
    live.subscribe(() => changes++);
    live.set(view({ elapsed: 0.05 }), 0.2, true);
    live.set(view({ elapsed: 0.1 }), 0.4, true);
    live.set(view({ elapsed: 0.15 }), 0.1, false);
    expect(changes).toBe(1);
    expect(live.level()).toMatchObject({ level: 0.1, speaking: false, elapsed: 0.15 });
    live.set(view({ state: "stopping", elapsed: 0.2 }), 0, false);
    expect(changes).toBe(2);
    expect(live.view()?.state).toBe("stopping");
  });

  it("drops the level when the recorder closes or the stream does", () => {
    const live = createHearLive();
    live.set(view(), 0.8, true);
    live.set(view({ state: "closed", take: null, ending: null }), 0.8, true);
    expect(live.level()).toMatchObject({ level: 0, speaking: false });
    live.set(view(), 0.6, true);
    live.reset();
    expect(live.level().level).toBe(0);
    expect(live.view()?.state).toBe("recording");
  });
});
