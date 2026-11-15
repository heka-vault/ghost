/**
 * The words a row leads with (2026-11-14): how long a run of calls took, said as a person says
 * a duration and measured over the calls themselves; a change's target and tool id behind the
 * row's disclosure; no watch's, webhook's or tool's identifier in a row's line.
 */
import type { Activity } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { activityItems, durationWords, workSeconds } from "../src/screens/session/timeline";
import { type Moment, momentsOf, plainChange, runReason } from "../src/screens/today/model";
import { rowSentence } from "../src/screens/today/Timeline";

describe("durationWords", () => {
  it("says seconds, minutes, hours and days, never a bare count of seconds past a minute", () => {
    expect(durationWords(0.42)).toBe("0.4 s");
    expect(durationWords(12.4)).toBe("12 s");
    expect(durationWords(60)).toBe("1 min");
    expect(durationWords(185)).toBe("3 min 5 s");
    expect(durationWords(7440)).toBe("2 h 4 min");
    expect(durationWords(125680)).toBe("1 day 11 h");
    expect(durationWords(3 * 86400)).toBe("3 days");
    expect(durationWords(-5)).toBe("0.0 s");
  });
});

describe("workSeconds: how long the calls kept Ghost working", () => {
  const at = (s: number) => new Date(Date.UTC(2026, 10, 14, 10, 0, 0) + s * 1000).toISOString();
  it("covers the calls, overlapping ones once, not the span from the group's first moment", () => {
    expect(workSeconds([{ startedAt: at(0), endedAt: at(1.5) }])).toBe(1.5);
    expect(workSeconds([{ startedAt: at(0), endedAt: at(2) }, { startedAt: at(1), endedAt: at(3) }])).toBe(3);
    // Two calls a day and a half apart are two short calls, not 35 hours.
    expect(workSeconds([{ startedAt: at(0), endedAt: at(0.8) }, { startedAt: at(125_000), endedAt: at(125_000.6) }])).toBeCloseTo(1.4);
  });
  it("counts a call still running up to now, and one with no result as nothing", () => {
    expect(workSeconds([{ startedAt: at(0), endedAt: null }], Date.parse(at(4)))).toBe(4);
    expect(workSeconds([{ startedAt: at(5), endedAt: at(2) }])).toBe(0);
  });
});

describe("a run of calls is one row per session", () => {
  const act = (session: string, kind: Activity["kind"], at: string, id: string): Activity => ({ kind, session_id: session, at, id, name: "github_issue_list" });
  it("never joins calls from two sessions into one row", () => {
    const list = [act("s1", "call", "2026-11-12T09:00:00.000Z", "a"), act("s1", "result", "2026-11-12T09:00:01.000Z", "a"), act("s2", "call", "2026-11-13T20:00:00.000Z", "b"), act("s2", "result", "2026-11-13T20:00:00.500Z", "b")];
    const rows = activityItems(list, [], ["github"]).filter((r) => r.kind === "work");
    expect(rows.length).toBe(2);
  });
});

describe("Today's rows say the plain sentence", () => {
  it("leads a change with what was done; the target goes behind the disclosure", () => {
    expect(plainChange("On ghost-mock/app#208: approved, with a comment.")).toEqual({ what: "Approved, with a comment.", on: "ghost-mock/app#208" });
    expect(plainChange("Pinned a note.")).toEqual({ what: "Pinned a note.", on: null });
    const day = new Date("2026-11-14T12:00:00");
    const moments = momentsOf(
      {
        audit: [{ id: "1", created_at: day.toISOString(), status: "executed", summary: "On ghost-mock/app#17: close the issue.", source: "github", operation: "close", conversation: "accounting" } as never],
        runs: [],
        waiting: [],
        questions: [],
      },
      day,
    );
    expect(moments[0]).toMatchObject({ title: "Close the issue.", mono: "ghost-mock/app#17 · github.close" });
    const s = rowSentence(moments[0], "Accounting");
    expect(`${s.verb} ${s.what} ${s.meta}`).not.toMatch(/#\d|github\.|ghost-mock/);
  });
  it("says where a watch's find or a note came from in words, not by its key", () => {
    const m = (kind: Moment["kind"], mono: string): Moment => ({ id: "x", kind, at: "2026-11-14T10:00:00Z", agent: null, title: "CI failed on main", tone: "neutral", mono });
    expect(rowSentence(m("watch", "watch ci"), null).meta).toBe("a watch found it");
    expect(rowSentence(m("watch", "webhook jobs"), null).meta).toBe("from a webhook");
    expect(rowSentence(m("note", "outside tool claude-code"), null).meta).toBe("from an outside tool");
    expect(rowSentence(m("answer", "by phone"), null).meta).toBe("on your phone");
    expect(runReason({ source: "watch:ci" } as never)).toBe("for a watch");
  });
});
