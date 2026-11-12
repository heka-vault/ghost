/**
 * Owner: session. Turns the flat turn list into what the transcript shows: groups of
 * consecutive turns by one speaker, transfer markers between agents, and system notes.
 *
 * Who spoke an assistant turn is worked out by walking the session: it starts with Ghost
 * (every wake lands there, ADR 0014), and each transfer sentence the old agent speaks
 * ("Transferring you to Accounting.", "Back to Ghost.", and after a move "Moved to
 * Infra.") hands the voice to the new one. A switch seen only as a status change (no
 * spoken sentence yet) is placed where it happened, so a transfer never goes unmarked.
 */
import type { Turn } from "@ghost/client-ts";

import type { ExtraItem } from "./timeline";

export interface LineInfo {
  slug: string;
  name: string;
  aliases: string[];
  /** When the agent was last used, from GET /conversations. */
  lastActive?: string | null;
  /** The agent's voice and speed, for hearing a reply again. */
  voice?: string;
  speed?: number;
}

/** A switch observed on the status stream: after the turn at `afterAt` (null: before any). */
export interface ObservedSwitch {
  afterAt: string | null;
  to: string;
}

/**
 * How a change that waited for the operator's okay was settled, kept in the transcript so the
 * most consequential control in the app leaves a record where it happened.
 */
export interface Decision {
  /** The proposal id. */
  key: string;
  /** Placed after the turn at this time (null: before any). */
  afterAt: string | null;
  outcome: "confirmed" | "canceled" | "expired" | "failed";
  /** True when the operator answered by voice rather than with the buttons. */
  byVoice: boolean;
  /** Plain words for what it was, from the gate's summary: “close the issue on ghost-mock/app#163”. */
  what: string;
  at: string;
}

export type Item =
  | { kind: "started"; key: string; at: string; dated?: boolean }
  /** A session in the history ended: how long it ran (ADR 0014; the store keeps no end, so its last turn). */
  | { kind: "ended"; key: string; at: string; secs: number }
  /** Time passed with nothing said (`timeline.ts`). */
  | { kind: "gap"; key: string; at: string; secs: number }
  /** A scheduled run's or an asked turn's entry in the transcript (ADR 0047, ADR 0048, ADR 0052). */
  | { kind: "run"; key: string; at: string; label: string; turns: Turn[] }
  | ExtraItem
  | { kind: "transfer"; key: string; to: string; text: string; at: string | null }
  | { kind: "note"; key: string; text: string; at: string }
  | { kind: "decision"; key: string; decision: Decision }
  | { kind: "ghost"; key: string; slug: string; turns: Turn[] }
  | { kind: "you"; key: string; typed: boolean; turns: Turn[] };

const PATCH = /^(?:transferring you to|patching you through to) (.+?)[.!]?$/i;
const BACK = /^back to ghost[.!]?$/i;

/** The agent a transfer sentence hands over to, or null if the turn isn't one. */
export function transferTarget(text: string, lines: LineInfo[]): string | null {
  const t = text.trim();
  if (BACK.test(t)) return "ghost";
  const m = PATCH.exec(t);
  if (!m) return null;
  return resolveLine(m[1], lines);
}

export function resolveLine(spoken: string, lines: LineInfo[]): string {
  const want = spoken.trim().replace(/^the\s+/i, "").toLowerCase();
  const hit = lines.find((l) => l.slug === want || l.name.toLowerCase() === want || l.aliases.some((a) => a.toLowerCase() === want));
  return hit?.slug ?? want.replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}

export function buildTranscript(turns: Turn[], current: string, lines: LineInfo[], observed: ObservedSwitch[], decisions: Decision[] = []): Item[] {
  const spokenAt = turns.map((t) => (t.role === "assistant" ? transferTarget(t.text, lines) : null));
  // An observed switch is dropped when the agent's own transfer sentence sits within two
  // turns of it (the status event and the spoken line arrive in either order).
  const extra = observed.filter((s) => {
    const i = s.afterAt == null ? -1 : turns.findIndex((t) => t.at === s.afterAt);
    if (s.afterAt != null && i < 0) return false;
    for (let j = Math.max(0, i - 2); j <= Math.min(turns.length - 1, i + 3); j++) if (spokenAt[j] === s.to) return false;
    return true;
  });
  const anyTransfer = extra.length > 0 || spokenAt.some((s) => s != null);
  // Without a transfer in view, every reply is the current line's (a session that began there).
  let speaker = anyTransfer ? "ghost" : current;

  const items: Item[] = [];
  if (turns.length) items.push({ kind: "started", key: "started", at: turns[0].at });

  // A decision whose turn has scrolled out of the store's window goes first.
  const known = new Set(turns.map((t) => t.at));
  // Two turns can share a timestamp; each decision is placed once.
  const placed = new Set<string>();
  const switchAfter = (at: string | null) => {
    // A switch with no spoken line takes the time of the next turn, the new line's first words.
    const next = at == null ? turns[0] : turns[turns.findIndex((t) => t.at === at) + 1];
    for (const s of extra.filter((x) => x.afterAt === at)) {
      items.push({ kind: "transfer", key: `sw-${at}-${s.to}`, to: s.to, text: "", at: next?.at ?? null });
      speaker = s.to;
    }
    for (const d of decisions.filter((x) => (at == null ? x.afterAt == null || !known.has(x.afterAt) : x.afterAt === at))) {
      if (placed.has(d.key)) continue;
      placed.add(d.key);
      items.push({ kind: "decision", key: `dc-${d.key}`, decision: d });
    }
  };
  switchAfter(null);

  turns.forEach((t, i) => {
    const key = `${i}-${t.at}`;
    const to = spokenAt[i];
    const last = items[items.length - 1];
    if (to != null) {
      items.push({ kind: "transfer", key, to, text: t.text.trim().replace(/[.!]$/, ""), at: t.at });
      speaker = to;
    } else if (t.role === "system") {
      items.push({ kind: "note", key, text: t.text, at: t.at });
    } else if (t.role === "assistant") {
      if (last?.kind === "ghost" && last.slug === speaker) last.turns.push(t);
      else items.push({ kind: "ghost", key, slug: speaker, turns: [t] });
    } else {
      const typed = t.via === "text";
      if (last?.kind === "you" && last.typed === typed) last.turns.push(t);
      else items.push({ kind: "you", key, typed, turns: [t] });
    }
    switchAfter(t.at);
  });
  return items;
}

/**
 * Ghost's earlier sessions, from its stored transcript (GET /conversations/ghost/turns), as
 * the same items the live transcript shows: one “Session started” caption per session, then
 * its turns. These are Ghost's own turns only (an agent's replies after a transfer live in
 * that agent's transcript), so a transfer sentence is a caption and every reply stays Ghost's.
 * Each caption carries the day, since the history spans days. Keys are prefixed so they
 * never meet the live session's.
 */
export function buildHistory(turns: Turn[], lines: LineInfo[]): Item[] {
  const items: Item[] = [];
  let session: string | null = null;
  let first: string | null = null;
  let prev: Turn | null = null;
  const end = () => {
    if (prev && first) items.push({ kind: "ended", key: `h-e-${prev.session_id}-${prev.at}`, at: prev.at, secs: secondsApart(first, prev.at) });
  };
  turns.forEach((t, i) => {
    const key = `h${i}-${t.at}`;
    if (t.session_id !== session) {
      end();
      session = t.session_id;
      first = t.at;
      items.push({ kind: "started", key: `${key}-s`, at: t.at, dated: true });
    }
    prev = t;
    const last = items[items.length - 1];
    const mark = runMark(t.text);
    if (mark) {
      const turn = { ...t, text: mark.rest };
      if (last?.kind === "run" && last.label === mark.label) last.turns.push(turn);
      else items.push({ kind: "run", key, at: t.at, label: mark.label, turns: [turn] });
      return;
    }
    const to = t.role === "assistant" ? transferTarget(t.text, lines) : null;
    if (to != null) items.push({ kind: "transfer", key, to, text: t.text.trim().replace(/[.!]$/, ""), at: t.at });
    else if (t.role === "system") items.push({ kind: "note", key, text: t.text, at: t.at });
    else if (t.role === "assistant") {
      if (last?.kind === "ghost") last.turns.push(t);
      else items.push({ kind: "ghost", key, slug: "ghost", turns: [t] });
    } else {
      const typed = t.via === "text";
      if (last?.kind === "you" && last.typed === typed) last.turns.push(t);
      else items.push({ kind: "you", key, typed, turns: [t] });
    }
  });
  end();
  return items;
}

const MARK = /^\[(?:scheduled run: ([^\]]+)|asked by ([^,\]]+), (an outside tool|another agent))\]\s*/;

/**
 * A turn a run with nobody there wrote (ADR 0047: `[scheduled run: <name>]`) or one an outside
 * tool or another agent asked for (ADR 0048, ADR 0052: `[asked by <who>, …]`): its label in
 * plain words and the words after the mark. Null for an ordinary turn.
 */
export function runMark(text: string): { label: string; rest: string } | null {
  const m = MARK.exec(text);
  if (!m) return null;
  const label = m[1] ? `Scheduled run · ${m[1].trim()}` : `Asked by ${m[2].trim()}`;
  return { label, rest: text.slice(m[0].length) };
}

function secondsApart(from: string, to: string): number {
  return Math.max(0, (new Date(to).getTime() - new Date(from).getTime()) / 1000);
}
