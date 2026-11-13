import type { ScheduleInfo, ScheduleRunInfo } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { activityOf } from "../src/screens/conversations/roster";

const run = (id: string, over: Partial<ScheduleRunInfo>): ScheduleRunInfo => ({ id, source: "schedule", agent: "accounting", name: "inbox", status: "ok", scheduled_for: "2026-11-12T10:00:00Z", started_at: "2026-11-12T10:00:00Z", finished_at: "2026-11-12T10:01:00Z", retries: 0, model_calls: 1, tool_calls: 1, answer: "One invoice.", ...over });
const sched = (id: string, over: Partial<ScheduleInfo>): ScheduleInfo =>
  ({ id, name: id, agent: "accounting", instruction: "", kind: "cron", cron: "0 9 * * 1-5", every: null, at: null, time_zone: "America/Detroit", summary: "", enabled: true, created_by: "operator", created_at: "", updated_at: "", running: false, grace_minutes: 0, max_runtime_minutes: 10, max_turns: 30, delete_after_run: false, skills: [], urgency: "notify", notify_failure: true, next_run: "2026-11-13T14:00:00Z", ...over }) as ScheduleInfo;

describe("activityOf", () => {
  it("says what an agent is doing, last did and does next", () => {
    const a = activityOf(
      "accounting",
      [run("old", { finished_at: "2026-11-12T08:00:00Z" }), run("new", { finished_at: "2026-11-12T11:00:00Z", answer: "[SILENT]" }), run("going", { status: "running", finished_at: null }), run("other", { agent: "legal" })],
      [sched("later", { next_run: "2026-11-20T14:00:00Z" }), sched("soon", {}), sched("off", { enabled: false, next_run: "2026-11-12T12:00:00Z" })],
      "America/Detroit",
    );
    expect(a.running?.id).toBe("going");
    expect(a.last?.run.id).toBe("new");
    expect(a.last?.said).toBe("Nothing to report.");
    expect(a.next?.schedule.id).toBe("soon");
    expect(a.next?.when).toBe("Weekdays at 9 AM");
    expect(a.scheduled).toBe(2);
  });
  it("is empty for an agent that has never run", () => {
    expect(activityOf("brand", [], [], "UTC")).toEqual({ running: null, last: null, next: null, scheduled: 0 });
  });
});
