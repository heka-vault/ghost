import type { ReachCall, ScheduleInfo, ScheduleRunInfo } from "@ghost/client-ts";
import { AGENT_SHAPES, lookOf, parseLook } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { briefOf } from "../src/screens/conversations/brief";
import { MockReach } from "../src/design/mock/reach";
import type { Moment } from "../src/screens/today/model";

const run = (over: Partial<ScheduleRunInfo>): ScheduleRunInfo => ({ id: "r1", source: "schedule", agent: "ledger", name: "vendor inbox", status: "running", scheduled_for: "2026-11-12T15:00:00Z", started_at: "2026-11-12T15:00:00Z", finished_at: null, retries: 0, model_calls: 1, tool_calls: 2, answer: null, ...over });
const call = (over: Partial<ReachCall>): ReachCall => ({ at: "2026-11-12T15:01:00Z", session_id: "run-r1", agent: "ledger", app: "github", tool: "search_issues", read: true, ok: true, items: 30, bytes: 7000, ms: 300, ...over });
const moment = (id: string, agent: string | null, at: string, children?: Moment[]): Moment => ({ id, kind: "change", at, agent, title: id, tone: "ok", children });

describe("an agent's brief", () => {
  it("is its run, what that run touches, its own moments and schedules, and its reach", () => {
    const b = briefOf("ledger", {
      working: [run({ id: "r0", agent: "legal" }), run({})],
      moments: [moment("c1", "ledger", "2026-11-12T14:00:00Z"), moment("cl", "ledger", "2026-11-12T15:00:00Z", [moment("c2", "ledger", "2026-11-12T15:00:00Z"), moment("c3", "ledger", "2026-11-12T14:50:00Z")]), moment("x", "legal", "2026-11-12T15:30:00Z")],
      schedules: [{ id: "s1", agent: "ledger" } as ScheduleInfo, { id: "s2", agent: "legal" } as ScheduleInfo],
      reachDay: null,
      calls: [call({}), call({ at: "2026-11-12T15:02:00Z", tool: "issue_read", items: 1 }), call({ agent: "legal", session_id: "run-r0" })],
    });
    expect(b.running?.id).toBe("r1");
    expect(b.touching?.tool).toBe("issue_read");
    expect(b.today.map((m) => m.id)).toEqual(["c2", "c3", "c1"]);
    expect(b.schedules.map((s) => s.id)).toEqual(["s1"]);
    expect(b.reach.calls).toBe(2);
    expect(b.reach.items).toBe(31);
  });

  it("the mock's reach totals the same way the daemon does", () => {
    const m = new MockReach(["github"]);
    m.seed([run({ status: "ok", tool_calls: 5, finished_at: "2026-11-12T15:05:00Z" })], null);
    const v = m.view("2026-11-12T00:00:00Z", "2026-11-13T00:00:00Z", []);
    expect(v.total.calls).toBe(5);
    expect(v.by_session[0].key).toBe("run-r1");
    expect(v.recent.length).toBe(5);
  });
});

describe("an agent's look", () => {
  it("reads as the daemon checks it, falls back to Wisp in its slug's colour, and Ghost is Wisp", () => {
    expect(parseLook("lantern:3")).toEqual({ shape: "lantern", hue: 3 });
    expect(parseLook("dragon:1")).toBeNull();
    expect(parseLook("moth:8")).toBeNull();
    expect(lookOf("ghost", "moth:2")).toEqual({ shape: "wisp", hue: 0 });
    expect(lookOf("ledger", null).shape).toBe("wisp");
    expect(lookOf("ledger", "star:5")).toEqual({ shape: "star", hue: 5 });
    expect(AGENT_SHAPES.length).toBe(8);
  });
});
