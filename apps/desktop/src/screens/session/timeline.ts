/**
 * Owner: session. The live session as one timeline: the turns (`transcript.ts`) with what
 * happened between them woven in by time, from the session's activity (ADR 0055), the gate's
 * outcomes, the questions asked, and the phase:
 *
 *   work       the tool calls between two turns, as one row (and a handover from the fast
 *              model, ADR 0050), opened to see each call
 *   cut        the operator cut in while Ghost was speaking: marks the reply before it
 *   error      a turn that failed
 *   change     a change Ghost made at `quiet`/`allow` and announced (an outcome that never
 *              waited for the operator)
 *   question   a question an agent asked during the session, with its choices (ADR 0051)
 *   hold       the session went on hold (ADR 0021), and when it came back
 *   gap        two minutes or more with nothing said
 *
 * Pure: the screen gathers the inputs and renders the items.
 */
import type { QuestionView, Turn } from "@ghost/client-ts";
import { type Activity, type Call, callsOf } from "@ghost/ui";

import type { Item } from "./transcript";

export type ExtraItem =
  | { kind: "work"; key: string; at: string; calls: Call[]; handover: { secs: number; names: string[] } | null }
  | { kind: "cut"; key: string; at: string }
  | { kind: "error"; key: string; at: string }
  | { kind: "change"; key: string; at: string; summary: string; ok: boolean }
  | { kind: "question"; key: string; at: string; question: QuestionView }
  | { kind: "hold"; key: string; at: string; back: string | null };

/** A change the gate made and announced, from an `outcome` event that never waited. */
export interface Announced {
  id: string;
  at: string;
  summary: string;
  ok: boolean;
}

/** A stretch on hold, from the phase. */
export interface Held {
  at: string;
  back: string | null;
}

/** Gaps this long or longer get a “2 min later” row. */
export const GAP_SECS = 120;

/** When an item happened: its first moment. */
export function itemAt(it: Item): string | null {
  switch (it.kind) {
    case "ghost":
    case "you":
    case "run":
      return it.turns[0]?.at ?? null;
    case "decision":
      return it.decision.at;
    case "transfer":
      return it.at;
    default:
      return it.at;
  }
}

/** When an item ended: its last moment (a group of turns ends at its last turn). */
function itemEnd(it: Item): string | null {
  if (it.kind === "ghost" || it.kind === "you" || it.kind === "run") return it.turns[it.turns.length - 1]?.at ?? null;
  return itemAt(it);
}

/**
 * The session's activity as rows: calls and handovers between two turns become one `work` row,
 * a cut and an error are rows of their own. The operator's own turn opening and closing is
 * the stage's to show, not the record's.
 */
export function activityItems(activity: readonly Activity[], turns: readonly Turn[], apps: readonly string[]): ExtraItem[] {
  const out: ExtraItem[] = [];
  const turnTimes = turns.map((t) => t.at).sort();
  // Which turn gap a moment falls in: the number of turns at or before it.
  const slot = (at: string) => {
    let n = 0;
    for (const t of turnTimes) if (t <= at) n++;
    return n;
  };
  let work: { slot: number; acts: Activity[] } | null = null;
  const flush = () => {
    if (!work) return;
    const handover = work.acts.find((a) => a.kind === "handover");
    out.push({
      kind: "work",
      key: `w-${work.acts[0].at}-${work.acts[0].id ?? work.acts[0].kind}`,
      at: work.acts[0].at,
      calls: callsOf(work.acts, apps),
      handover: handover ? { secs: handover.secs ?? 0, names: handover.names ?? [] } : null,
    });
    work = null;
  };
  for (const a of activity) {
    if (a.kind === "call" || a.kind === "result" || a.kind === "handover") {
      const s = slot(a.at);
      // A result belongs with its call even when a reply was recorded in between.
      if (work && (work.slot === s || a.kind === "result")) work.acts.push(a);
      else {
        flush();
        work = { slot: s, acts: [a] };
      }
    } else if (a.kind === "cut" || a.kind === "error") {
      flush();
      out.push({ kind: a.kind, key: `${a.kind}-${a.at}`, at: a.at });
    }
  }
  flush();
  return out;
}

/**
 * The extras put among the items by time. An extra goes after every item that started at or
 * before it; a group of turns it falls inside is split there, so a call made between two of
 * Ghost's sentences sits between them. Items without a time keep their place.
 */
export function weave(items: Item[], extras: readonly ExtraItem[]): Item[] {
  if (!extras.length) return items;
  const sorted = [...extras].sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
  const out: Item[] = [];
  let e = 0;
  const emitUpTo = (at: string | null, inclusive: boolean) => {
    while (e < sorted.length && at != null && (inclusive ? sorted[e].at <= at : sorted[e].at < at)) out.push(sorted[e++]);
  };
  for (const it of items) {
    const start = itemAt(it);
    // Extras strictly before this item go first.
    emitUpTo(start, false);
    if ((it.kind === "ghost" || it.kind === "you") && it.turns.length > 1 && e < sorted.length) {
      // Split the group where an extra falls between its turns.
      let part: Turn[] = [];
      let n = 0;
      for (const t of it.turns) {
        if (part.length && e < sorted.length && sorted[e].at < t.at) {
          out.push({ ...it, key: n ? `${it.key}~${n}` : it.key, turns: part });
          n++;
          part = [];
          emitUpTo(t.at, false);
        }
        part.push(t);
      }
      out.push({ ...it, key: n ? `${it.key}~${n}` : it.key, turns: part });
    } else out.push(it);
  }
  while (e < sorted.length) out.push(sorted[e++]);
  return out;
}

/**
 * “2 min later” rows where nothing was said for {@link GAP_SECS} or more, between two items of
 * the same session (a new session has its own “Session started”).
 */
export function withGaps(items: Item[]): Item[] {
  const out: Item[] = [];
  let lastEnd: string | null = null;
  for (const it of items) {
    if (it.kind === "started") lastEnd = null;
    const start = itemAt(it);
    if (lastEnd && start && it.kind !== "ended") {
      const secs = (new Date(start).getTime() - new Date(lastEnd).getTime()) / 1000;
      if (secs >= GAP_SECS) out.push({ kind: "gap", key: `gap-${start}`, at: start, secs });
    }
    out.push(it);
    lastEnd = itemEnd(it) ?? lastEnd;
  }
  return out;
}

/** “2 min later”, “1 h 5 min later”, “3 h later”. */
export function gapText(secs: number): string {
  const m = Math.round(secs / 60);
  if (m < 60) return `${m} min later`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h} h ${r} min later` : `${h} h later`;
}

/** “4 min”, “1 h 2 min”, “under a minute”: how long a session ran. */
export function lengthText(secs: number): string {
  if (secs < 60) return "under a minute";
  const m = Math.round(secs / 60);
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h} h ${r} min` : `${h} h`;
}

/** Which ghost items were cut off: the reply right before each cut row. Keys of those items. */
export function cutReplies(items: readonly Item[]): Set<string> {
  const cut = new Set<string>();
  items.forEach((it, i) => {
    if (it.kind !== "cut") return;
    for (let j = i - 1; j >= 0; j--) {
      const p = items[j];
      if (p.kind === "ghost") {
        cut.add(p.key);
        return;
      }
      if (p.kind === "you") return;
    }
  });
  return cut;
}
