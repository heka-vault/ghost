import type { AuditEntry, QuestionView, ScheduleRunInfo, WaitingItemView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { byHour, cluster, countsOf, dayBounds, dayLabel, density, filterMoments, headline, momentsOf, needKind, needsOf, reachOf, replySpeed, runAnswer, shiftDay } from "../src/screens/today/model";

// A fixed local day at noon, so every test reads the same hours.
const DAY = new Date(2026, 10, 12, 15, 0, 0);
const at = (h: number, m = 0) => new Date(2026, 10, 12, h, m).toISOString();
const NOW = DAY.getTime();

const item = (over: Partial<WaitingItemView>): WaitingItemView => ({ key: "k", source: "watch:ci", title: "CI failed", state: "new", first_seen: at(10), changed_at: at(10), age_secs: 0, ...over });
const change = (id: string, h: number, m = 0, over: Partial<AuditEntry> = {}): AuditEntry => ({
  id,
  session_id: "s1",
  source: "github",
  operation: "close",
  target: "app#1",
  summary: `change ${id}`,
  autonomy: "allow",
  status: "executed",
  created_at: at(h, m),
  events: [],
  conversation: "legal",
  ...over,
});
const run = (id: string, h: number, over: Partial<ScheduleRunInfo> = {}): ScheduleRunInfo => ({ id, source: "schedule", agent: "accounting", name: "vendor inbox", status: "ok", scheduled_for: at(h), started_at: at(h), finished_at: at(h, 2), retries: 0, model_calls: 2, tool_calls: 3, answer: "One invoice.", ...over });
const question = (over: Partial<QuestionView> = {}): QuestionView => ({
  key: "q1",
  kind: "question",
  asked_by: "Accounting",
  source: "agent:accounting",
  question: "Refund it?",
  choices: ["Yes", "No"],
  choices_only: false,
  urgency: "notify",
  asked_at: at(11),
  expires_at: at(23),
  state: "open",
  in_run: false,
  ...over,
});

describe("needs", () => {
  it("sorts each open item by what the operator does about it", () => {
    const pending = { id: "p", source: "github", operation: "close", target: "x", summary: "Close x.", rationale: "", expires_at: at(16), expires_in_secs: 60 } as WaitingItemView["change"];
    const n = needsOf(
      [
        item({ key: "a", change: pending }),
        item({ key: "b", question: question() }),
        item({ key: "c" }),
        item({ key: "d", state: "acknowledged" }),
        item({ key: "e", state: "snoozed", snoozed_until: new Date(NOW + 60_000).toISOString() }),
      ],
      NOW,
    );
    expect(n.decide.map((i) => i.key)).toEqual(["a"]);
    expect(n.answer.map((i) => i.key)).toEqual(["b"]);
    expect(n.know.map((i) => i.key)).toEqual(["c"]);
  });

  it("brings a snoozed item back when its time is up, and drops an answered question", () => {
    expect(needKind(item({ state: "snoozed", snoozed_until: new Date(NOW - 1).toISOString() }), NOW)).toBe("know");
    expect(needKind(item({ question: question({ state: "answered" }) }), NOW)).toBeNull();
  });
});

describe("momentsOf", () => {
  const records = {
    audit: [change("1", 14, 5), change("2", 9, 0, { status: "failed" }), change("3", 8, 0, { status: "pending" }), change("old", 9, 0, { created_at: shiftDay(DAY, -1).toISOString() })],
    runs: [run("r1", 13), run("r2", 12, { status: "running", finished_at: null })],
    waiting: [item({ key: "w", source: "watch:ci", first_seen: at(10) }), item({ key: "agent:accounting:schedule:x", source: "agent:accounting", first_seen: at(13) })],
    questions: [question({ state: "answered", answer: "Yes", answered_at: at(11, 30), answered_via: "desktop" })],
  };

  it("keeps the day's changes, runs, watch findings and questions, newest first", () => {
    const m = momentsOf(records, DAY);
    expect(m.map((x) => x.kind)).toEqual(["change", "run", "run", "answer", "question", "watch", "change"]);
    expect(m.every((x, i) => i === 0 || m[i - 1].at >= x.at)).toBe(true);
  });

  it("leaves out the change still waiting (it is in Decide), other days, and a run's delivered answer", () => {
    const ids = momentsOf(records, DAY).map((x) => x.id);
    expect(ids).not.toContain("c-3");
    expect(ids).not.toContain("c-old");
    expect(ids.some((id) => id.includes("agent:accounting"))).toBe(false);
  });

  it("says a silent run had nothing to report, and a check that saw no change didn't need the model", () => {
    expect(runAnswer(run("x", 9, { answer: "[SILENT]" }))).toBe("Nothing to report.");
    expect(runAnswer(run("x", 9, { status: "unchanged", answer: null }))).toMatch(/didn’t need the model/);
  });
});

describe("cluster", () => {
  it("folds three or more changes by one agent close together, and leaves pairs alone", () => {
    const m = momentsOf({ audit: [change("a", 14, 10), change("b", 14, 5), change("c", 14, 0), change("d", 13, 0), change("e", 12, 59)], runs: [], waiting: [], questions: [] }, DAY);
    const c = cluster(m);
    expect(c).toHaveLength(3);
    expect(c[0].children?.map((x) => x.id)).toEqual(["c-a", "c-b", "c-c"]);
    expect(c[0].title).toBe("3 changes");
  });
});

describe("byHour and density", () => {
  it("groups by local hour and counts the quiet hours between", () => {
    const m = momentsOf({ audit: [change("a", 14), change("b", 14, 1), change("c", 9)], runs: [], waiting: [], questions: [] }, DAY);
    const blocks = byHour(m);
    expect(blocks.map((b) => [b.hour, b.moments.length, b.quietBefore])).toEqual([
      [14, 2, 0],
      [9, 1, 4],
    ]);
    const d = density(m);
    expect(d[14].count).toBe(2);
    expect(d[9].count).toBe(1);
  });
});

describe("filterMoments", () => {
  it("narrows by kind and by agent", () => {
    const m = momentsOf({ audit: [change("a", 14)], runs: [run("r", 13)], waiting: [], questions: [] }, DAY);
    expect(filterMoments(m, "runs").map((x) => x.kind)).toEqual(["run"]);
    expect(filterMoments(m, "all", "legal").map((x) => x.id)).toEqual(["c-a"]);
  });
});

describe("headline", () => {
  const zero = { decide: 0, answer: 0, know: 0, changes: 0, failedChanges: 0, runs: 0, modelCalls: 0, toolCalls: 0, found: 0, answered: 0 };
  it("leads with what needs the operator, then what Ghost did, with real counts", () => {
    expect(headline({ ...zero, decide: 1, answer: 1, changes: 8, runs: 5 })).toEqual({ lead: "2 things need you.", rest: "Ghost made 8 changes and ran 5 jobs today." });
    expect(headline({ ...zero, decide: 1 }).lead).toBe("One thing needs you.");
  });
  it("is honest on a quiet day", () => {
    expect(headline(zero)).toEqual({ lead: "Nothing needs you.", rest: "Ghost hasn’t had to do anything yet today." });
    expect(headline({ ...zero, know: 2 }).lead).toBe("Nothing to decide. 2 things to know.");
  });
  it("reads a past day as a record", () => {
    expect(headline({ ...zero, changes: 1, runs: 3 }, false).lead).toBe("1 change, 3 runs.");
    expect(headline(zero, false).lead).toBe("A quiet day.");
  });
  it("counts from the moments, a cluster's members included", () => {
    const m = cluster(momentsOf({ audit: [change("a", 14, 10), change("b", 14, 5), change("c", 14, 0)], runs: [run("r", 13)], waiting: [], questions: [] }, DAY));
    const c = countsOf({ decide: [], answer: [], know: [] }, m);
    expect(c.changes).toBe(3);
    expect(c.runs).toBe(1);
    expect(c.toolCalls).toBe(3);
  });
});

describe("days", () => {
  it("names today and yesterday, and spans local midnight to midnight", () => {
    expect(dayLabel(DAY, DAY)).toBe("Today");
    expect(dayLabel(shiftDay(DAY, -1), DAY)).toBe("Yesterday");
    const b = dayBounds(DAY);
    expect(new Date(b.start).getHours()).toBe(0);
    expect(b.end - b.start).toBeGreaterThanOrEqual(23 * 3600_000);
  });
});

describe("reach and speed", () => {
  it("counts only connected apps' tools, and the apps that are down", () => {
    const app = (state: string, n: number) => ({ name: state, title: state, kind: "url", state, actions: Array(n).fill({}), settings: [], limits: {} }) as never;
    expect(reachOf([app("connected", 4), app("connected", 2), app("failed", 9), app("needs_sign_in", 1)])).toEqual({ apps: 2, tools: 6, down: 2 });
  });
  it("takes the quickest reachable model's first word", () => {
    const m = (model: string, ms: number | null, reachable = true) => ({ role: "main", model, reachable, first_word: { median_ms: ms, worst_ms: null } }) as never;
    expect(replySpeed({ checked_at: at(9), where: "x", models: [m("big", 1400), m("fast", 700), m("down", 10, false)] })?.text).toBe("0.7 s");
    expect(replySpeed(null)).toBeNull();
  });
});
