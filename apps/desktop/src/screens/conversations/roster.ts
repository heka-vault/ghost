/**
 * The roster's line for each agent: what it is doing right now, what it last did on its own,
 * and when it next will. From the runs and the schedules (ADR 0047); plain, so it is tested.
 */
import type { ScheduleInfo, ScheduleRunInfo } from "@ghost/client-ts";

import { runAnswer } from "../today/model";
import { whenWords } from "../watching/model";

export interface AgentActivity {
  /** A run going now. */
  running: ScheduleRunInfo | null;
  /** The newest finished run, with what it said in a line. */
  last: { run: ScheduleRunInfo; said: string | null } | null;
  /** The next scheduled run, its time and the schedule in words. */
  next: { schedule: ScheduleInfo; at: string; when: string } | null;
  /** Schedules of this agent that are on. */
  scheduled: number;
}

export function activityOf(slug: string, runs: ScheduleRunInfo[], schedules: ScheduleInfo[], zone: string): AgentActivity {
  const mine = runs.filter((r) => r.agent === slug);
  const running = mine.find((r) => r.status === "running" || r.status === "queued") ?? null;
  const done = mine.filter((r) => r.finished_at).sort((a, b) => (b.finished_at ?? "").localeCompare(a.finished_at ?? ""))[0] ?? null;
  const on = schedules.filter((s) => s.agent === slug && s.enabled);
  const upcoming = on.filter((s) => s.next_run).sort((a, b) => a.next_run!.localeCompare(b.next_run!))[0] ?? null;
  return {
    running,
    last: done ? { run: done, said: runAnswer(done) } : null,
    next: upcoming ? { schedule: upcoming, at: upcoming.next_run!, when: whenWords(upcoming, zone).text } : null,
    scheduled: on.length,
  };
}
