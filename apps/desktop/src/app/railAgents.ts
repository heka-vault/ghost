/**
 * The agents the rail lists, capped (the owner's simplification, 2026-11-14; ADR 0061 as amended).
 *
 * Which: every agent that matters now is shown whatever the cap (in the session, waiting on the
 * operator for a decision or an answer, at work), then the most recently used fill the rest of
 * the room, up to `RAIL_MAX`. The rest are one entry, All agents. Ghost is the rail's presence
 * at the top, never a row; retired agents are off the rail (the roster has them).
 *
 * In what order: the order they were hired, which never changes, so an agent keeps its place on
 * the rail and its state shows on its mark, not by moving. An agent that starts working or needs
 * the operator appears at once; while the pointer is on the list (`holdStill`) a newcomer goes at
 * the end and nothing leaves or moves until the pointer does, so a row never jumps under it.
 * Pure, so it is tested (test/rail-agents.test.ts).
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
  /** When it was hired: its place on the rail. */
  hired: string;
  /** When it last worked, for which of the quiet ones fill the room. */
  last: string;
}

/**
 * The most agents the rail lists, whatever room it has. Measured (docs/design-system.md, the
 * rail): at the window's default 1040×720 the rail has room for four agent rows besides All
 * agents (40px each, names hidden under 760px tall), at 1440×900 for five (60px with names); six
 * keeps a tall window calm, and the owner works with six or more. A short window shows as many as
 * fit (`fitOf`), and at the smallest window (480px tall) that can be none: every agent that
 * matters now is shown anyway, and the list scrolls, as it did before the cap.
 */
export const RAIL_MAX = 6;

/** Whose an item is: `agent:<slug>` as its source, or the asker of its question. */
function ownerOf(i: WaitingItemView): string | null {
  const src = i.question?.source ?? i.source;
  return src.startsWith("agent:") ? src.slice("agent:".length).split(":")[0] : null;
}

/** Every hired agent with its live state, in the order they were hired. */
export function railAgents(r: { agents: ConversationView[]; working: ScheduleRunInfo[]; needs: WaitingItemView[]; session: string | null; phase: string }): RailAgent[] {
  const waiting = new Map<string, number>();
  for (const i of r.needs) {
    const o = ownerOf(i);
    if (o) waiting.set(o, (waiting.get(o) ?? 0) + 1);
  }
  const runningNow = new Set(r.working.filter((w) => w.status === "running").map((w) => w.agent));
  const rows = r.agents
    .filter((a) => !a.is_ghost && !a.retired_at)
    .map((a): RailAgent => {
      const inSession = r.session === a.slug;
      const state: RailState = inSession ? (r.phase === "speaking" ? "speaking" : r.phase === "thinking" ? "thinking" : "listening") : waiting.get(a.slug) ? "waiting" : runningNow.has(a.slug) ? "working" : null;
      const n = waiting.get(a.slug) ?? 0;
      const words = [inSession ? (state === "speaking" ? "speaking" : state === "thinking" ? "working on your words" : "in the conversation") : null, runningNow.has(a.slug) ? "working" : null, n ? `waiting on you (${n})` : null]
        .filter(Boolean)
        .join(", ");
      return { slug: a.slug, name: a.name, state, waiting: n, stateWords: words, hired: a.created_at ?? "", last: a.last_active_at ?? "" };
    });
  return rows.sort(byHire);
}

const byHire = (a: RailAgent, b: RailAgent) => a.hired.localeCompare(b.hired) || a.slug.localeCompare(b.slug);

/**
 * Which agents the rail shows with room for `fit` rows: every one that matters now, then the most
 * recently used, `RAIL_MAX` at most; in the order they were hired. `hidden` is how many are left
 * for All agents.
 */
export function railShown(all: readonly RailAgent[], fit: number): { shown: RailAgent[]; hidden: number } {
  const room = Math.max(0, Math.min(RAIL_MAX, Math.floor(fit)));
  const live = all.filter((a) => a.state != null);
  const quiet = all
    // Never used yet sorts after every used one, in the order they were hired.
    .filter((a) => a.state == null)
    .sort((a, b) => b.last.localeCompare(a.last) || byHire(a, b))
    .slice(0, Math.max(0, room - live.length));
  const keep = new Set([...live, ...quiet].map((a) => a.slug));
  const shown = all.filter((a) => keep.has(a.slug));
  return { shown, hidden: all.length - shown.length };
}

/**
 * The rail's order while the pointer may be on it: with `hold`, what was shown keeps its place
 * (even one that would leave), and a newcomer goes at the end; without, the order as chosen.
 */
export function holdStill(before: readonly string[], now: readonly string[], hold: boolean): string[] {
  if (!hold) return [...now];
  return [...before, ...now.filter((s) => !before.includes(s))];
}

/**
 * How many agent rows fit in the rail's nav without scrolling: its height, less what the rest of
 * the nav takes (the places, All agents, the gaps), over one row's pitch.
 */
export function fitOf(m: { navHeight: number; restHeight: number; rowPitch: number }): number {
  if (m.rowPitch <= 0) return RAIL_MAX;
  return Math.max(0, Math.min(RAIL_MAX, Math.floor((m.navHeight - m.restHeight) / m.rowPitch)));
}
