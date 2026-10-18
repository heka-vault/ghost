/**
 * Owner: session. Turns the flat turn list into what the transcript shows: groups of
 * consecutive turns by one speaker, transfer markers between lines, and system notes.
 *
 * Who spoke an assistant turn is worked out by walking the session: it starts on Ghost
 * (every wake lands there, ADR 0014), and each transfer line the old line speaks
 * ("Patching you through to Accounting.", "Back to Ghost.") hands the voice to the new
 * one. A switch seen only as a status change (no spoken line yet) is placed where it
 * happened, so a transfer never goes unmarked.
 */
import type { Turn } from "@ghost/client-ts";

export interface LineInfo {
  slug: string;
  name: string;
  aliases: string[];
  /** When the line was last used, from GET /conversations. */
  lastActive?: string | null;
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
  /** `undone`: the operator undid a change that has no caption of its own here. */
  outcome: "confirmed" | "canceled" | "expired" | "failed" | "undone";
  /** True when the operator answered by voice rather than with the buttons. */
  byVoice: boolean;
  /** Plain words for what it was, from the gate's summary: “close the issue on ghost-mock/app#163”. */
  what: string;
  /** The gate's summary, to match it against what Undo would reverse. */
  summary: string;
  at: string;
  undone?: boolean;
}

export type Item =
  | { kind: "started"; key: string; at: string }
  | { kind: "transfer"; key: string; to: string; text: string; at: string | null }
  | { kind: "note"; key: string; text: string; at: string }
  | { kind: "decision"; key: string; decision: Decision }
  | { kind: "ghost"; key: string; slug: string; turns: Turn[] }
  | { kind: "you"; key: string; typed: boolean; turns: Turn[] };

const PATCH = /^patching you through to (.+?)[.!]?$/i;
const BACK = /^back to ghost[.!]?$/i;

/** The line a transfer sentence hands over to, or null if the turn isn't one. */
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
  // An observed switch is dropped when the line's own transfer sentence sits within two
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
