/**
 * An agent as someone with a job (the agent's page): what it is doing now, what it did today
 * (its own slice of Today's timeline), when it works on its own, and how much it read. Pure,
 * over the records Today already holds, so the page costs no request of its own.
 */
import type { ReachCall, ReachView, ScheduleInfo, ScheduleRunInfo } from "@ghost/client-ts";

import type { Moment } from "../today/model";
import { EMPTY_TALLY, type Tally, tallyOf } from "../today/telemetry";

export interface Brief {
  /** The run it is on now, if any. */
  running: ScheduleRunInfo | null;
  /** Its latest call while it runs: what it is touching. */
  touching: ReachCall | null;
  /** Today's moments that are its own, newest first, clusters opened. */
  today: Moment[];
  schedules: ScheduleInfo[];
  /** How much it read today. */
  reach: Tally;
}

/** One agent's share of a day's reach: its row in `by_agent` and its live calls. */
export function agentTally(base: ReachView | null, live: readonly ReachCall[], slug: string): Tally {
  const row = base?.by_agent.find((a) => a.key === slug);
  const mine = live.filter((c) => c.agent === slug);
  if (!row && !mine.length) return EMPTY_TALLY;
  const view: ReachView | null = base && row ? { ...base, total: row } : null;
  return tallyOf(view, mine);
}

export function briefOf(
  slug: string,
  r: { working: ScheduleRunInfo[]; moments: Moment[]; schedules: ScheduleInfo[]; reachDay: ReachView | null; calls: readonly ReachCall[] },
): Brief {
  const running = r.working.find((w) => w.agent === slug && w.status === "running") ?? r.working.find((w) => w.agent === slug) ?? null;
  const touching = running ? ([...r.calls].reverse().find((c) => c.session_id === `run-${running.id}`) ?? null) : null;
  const today = r.moments.flatMap((m) => m.children ?? [m]).filter((m) => (m.agent ?? "ghost") === slug).sort((a, b) => b.at.localeCompare(a.at));
  return {
    running,
    touching,
    today,
    schedules: r.schedules.filter((s) => s.agent === slug),
    reach: agentTally(r.reachDay, r.calls, slug),
  };
}
