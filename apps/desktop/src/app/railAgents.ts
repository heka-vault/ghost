/**
 * The agents the rail lists, in the order that keeps it calm with many (the owner has six or
 * more): the one in the session first, then any waiting on the operator, then any at work,
 * then the rest by when they last worked. Ghost is the rail's presence at the top, never a row;
 * retired agents are off the rail (Settings › Agents has them). Pure, so it is tested.
 */
import type { ConversationView, ScheduleRunInfo, WaitingItemView } from "@ghost/client-ts";

export type RailState = "speaking" | "listening" | "thinking" | "working" | "waiting" | null;

export interface RailAgent {
  slug: string;
  name: string;
  state: RailState;
  /** How many decisions or answers it waits on the operator for. */
  waiting: number;
  /** The state in words, for its tooltip and accessible name: “speaking”, “working”, “waiting on you (2)”. */
  stateWords: string;
}

/** Whose an item is: `agent:<slug>` as its source, or the asker of its question. */
function ownerOf(i: WaitingItemView): string | null {
  const src = i.question?.source ?? i.source;
  return src.startsWith("agent:") ? src.slice("agent:".length).split(":")[0] : null;
}

export function railAgents(r: { agents: ConversationView[]; working: ScheduleRunInfo[]; needs: WaitingItemView[]; session: string | null; phase: string }): RailAgent[] {
  const waiting = new Map<string, number>();
  for (const i of r.needs) {
    const o = ownerOf(i);
    if (o) waiting.set(o, (waiting.get(o) ?? 0) + 1);
  }
  const runningNow = new Set(r.working.filter((w) => w.status === "running").map((w) => w.agent));
  const rows = r.agents
    .filter((a) => !a.is_ghost && !a.retired_at)
    .map((a): RailAgent & { rank: number; last: string } => {
      const inSession = r.session === a.slug;
      const state: RailState = inSession ? (r.phase === "speaking" ? "speaking" : r.phase === "thinking" ? "thinking" : "listening") : waiting.get(a.slug) ? "waiting" : runningNow.has(a.slug) ? "working" : null;
      const n = waiting.get(a.slug) ?? 0;
      const words = [inSession ? (state === "speaking" ? "speaking" : state === "thinking" ? "working on your words" : "in the conversation") : null, runningNow.has(a.slug) ? "working" : null, n ? `waiting on you (${n})` : null]
        .filter(Boolean)
        .join(", ");
      return { slug: a.slug, name: a.name, state, waiting: n, stateWords: words, rank: inSession ? 0 : n ? 1 : runningNow.has(a.slug) ? 2 : 3, last: a.last_active_at ?? "" };
    });
  rows.sort((a, b) => a.rank - b.rank || b.last.localeCompare(a.last) || a.name.localeCompare(b.name));
  return rows.map(({ rank: _r, last: _l, ...a }) => a);
}
