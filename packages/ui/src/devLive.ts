/**
 * Settings › Developer's live views (ADR 0073, amended 2026-11-14), as the daemon reports
 * them: the timing overlay's `timing` events (one per turn of a live session, at its end:
 * stage times, the model and provider, fast or main) and the microphone meter's `meter`
 * events (about fifteen a second while the meter is open). Numbers and fixed kinds only:
 * nothing here ever holds a word that was said.
 *
 * Kept out of React state, like the voice level and the recorder's level: a view reads what
 * it needs and subscribes. The last ten turns are kept in memory for the overlay and its Copy
 * button; nothing is written anywhere. Plain TypeScript, safe for React Native.
 */
import type { MeterView, TimingView } from "@ghost/client-ts";

import type { Activity } from "./activity";

/** Turns the overlay keeps, and copies. */
export const TIMINGS_KEPT = 10;

export interface DevLive {
  /** The last turns, oldest first. */
  timings(): readonly TimingView[];
  /** A `timing` event. */
  pushTiming(t: TimingView): void;
  /** The last meter reading, or null when the meter is closed. */
  meter(): MeterView | null;
  /** A `meter` event. */
  pushMeter(m: MeterView): void;
  /** Called when a turn is added. */
  subscribe(fn: () => void): () => void;
  /** Called on every meter reading. */
  subscribeMeter(fn: (m: MeterView) => void): () => void;
}

export function createDevLive(): DevLive {
  let list: TimingView[] = [];
  let last: MeterView | null = null;
  const subs = new Set<() => void>();
  const meters = new Set<(m: MeterView) => void>();
  return {
    timings: () => list,
    pushTiming(t) {
      if (list.some((x) => x.session_id === t.session_id && x.turn === t.turn && x.at === t.at)) return;
      list = [...list, t].slice(-TIMINGS_KEPT);
      for (const fn of subs) fn();
    },
    meter: () => last,
    pushMeter(m) {
      last = m.open ? m : null;
      for (const fn of meters) fn(m);
    },
    subscribe(fn) {
      subs.add(fn);
      return () => subs.delete(fn);
    },
    subscribeMeter(fn) {
      meters.add(fn);
      return () => meters.delete(fn);
    },
  };
}

// --- the timing overlay's rows ---------------------------------------------------------

/** The stages a row shows, in order, with their short labels. */
export const STAGES = [
  ["closed", "closed"],
  ["stt", "stt"],
  ["first_word", "word"],
  ["first_tool", "tool"],
  ["tool_result", "result"],
  ["first_sound", "sound"],
  ["reply_end", "end"],
] as const;

export type StageKey = (typeof STAGES)[number][0];

/** Seconds as the overlay shows them: two decimals, or a dash for a stage the turn didn't have. */
export function secs(x: number | null | undefined): string {
  return typeof x === "number" && Number.isFinite(x) ? x.toFixed(2) : "–";
}

/** A model's name as the overlay shows it: the provider's prefix and a date suffix left out. */
export function shortModel(m: string | null | undefined): string {
  if (!m) return "no model";
  const last = m.split("/").pop() ?? m;
  return last.replace(/-\d{8}$/, "");
}

/** The model and where it ran: "haiku-5-5 · openrouter · fast". */
export function modelLine(t: Pick<TimingView, "model" | "provider" | "fast" | "handover">): string {
  const parts = [shortModel(t.model).replace(/^claude-/, "")];
  if (t.provider) parts.push(t.provider);
  parts.push(t.fast ? (t.handover ? "fast→main" : "fast") : "main");
  return parts.join(" · ");
}

export interface TimingRow {
  turn: number;
  kind: string;
  /** Each stage's seconds, as shown. */
  cells: Record<StageKey, string>;
  model: string;
  /** The person talked over the reply. */
  cut: boolean;
  /** Still under way: from the session's activity, before the turn's record. */
  live: boolean;
}

export function rowOf(t: TimingView): TimingRow {
  const cells = {} as Record<StageKey, string>;
  for (const [k] of STAGES) cells[k] = secs(t[k]);
  return { turn: t.turn, kind: t.kind, cells, model: modelLine(t), cut: t.barge_in != null, live: false };
}

/**
 * The turn under way, from the session's activity (ADR 0055): the operator's turn closed, the
 * first call asked for and back, before the turn's record arrives at its end. Null when no turn
 * is under way, or its record is in already (`lastAt`, the newest record's time).
 */
export function liveRow(activity: readonly Activity[], lastAt: string | null): TimingRow | null {
  let i = activity.length - 1;
  while (i >= 0 && activity[i].kind !== "you_stopped") i--;
  if (i < 0) return null;
  const t0 = new Date(activity[i].at).getTime();
  if (lastAt && new Date(lastAt).getTime() >= t0) return null;
  const after = activity.slice(i + 1);
  const at = (k: Activity["kind"]) => {
    const a = after.find((x) => x.kind === k);
    return a ? (new Date(a.at).getTime() - t0) / 1000 : null;
  };
  const cells = {} as Record<StageKey, string>;
  for (const [k] of STAGES) cells[k] = "…";
  cells.closed = "0.00";
  cells.first_tool = after.some((x) => x.kind === "call") ? secs(at("call")) : "…";
  cells.tool_result = after.some((x) => x.kind === "result") ? secs(at("result")) : "…";
  return { turn: 0, kind: "now", cells, model: "", cut: after.some((x) => x.kind === "cut"), live: true };
}

/** The last turns as a plain-text table, for pasting into an issue. Numbers only. */
export function timingTable(list: readonly TimingView[]): string {
  const head = ["turn", "kind", ...STAGES.map(([, l]) => l), "model"];
  const rows = list.map((t) => {
    const r = rowOf(t);
    return [String(r.turn), r.kind, ...STAGES.map(([k]) => r.cells[k]), r.model + (r.cut ? " · cut" : "")];
  });
  const all = [head, ...rows];
  const widths = head.map((_, c) => Math.max(...all.map((r) => r[c].length)));
  const line = (r: string[]) =>
    r
      .map((x, c) => (c === r.length - 1 ? x : c < 2 ? x.padEnd(widths[c]) : x.padStart(widths[c])))
      .join("  ")
      .trimEnd();
  return [
    "Ghost turn timings, seconds from when you stopped talking (a typed turn: from when it was sent)",
    line(head),
    ...rows.map(line),
  ].join("\n");
}

// --- the meter's decisions -------------------------------------------------------------

/** A decision as the meter's list says it: by kind, with its numbers, never words. */
export function decisionWords(d: MeterView["decisions"][number]): string {
  const db = (x: number | null | undefined) => (typeof x === "number" ? `${x.toFixed(0)} dBFS` : "");
  switch (d.kind) {
    case "taken":
      return `Taken as your turn: ${d.words ?? 0} word${d.words === 1 ? "" : "s"} at ${db(d.dbfs)}`;
    case "no_words":
      return `Read, no words in it (${(d.secs ?? 0).toFixed(1)} s at ${db(d.dbfs)})`;
    case "held_room":
      return d.person != null ? `Held as the room: ${db(d.dbfs)}, you at ${db(d.person)}` : `Held as the room: ${db(d.dbfs)}, your level not heard yet`;
    case "let_go":
      return `Let go as the room (${(d.secs ?? 0).toFixed(1)} s at ${db(d.dbfs)})`;
    case "taken_by_name":
      return "Held, then taken: it began with Ghost’s name";
    case "echo":
      return "Dropped as Ghost’s own voice coming back";
    case "too_short":
      return "Too short to interrupt Ghost";
    case "barge_in":
      return "Barge-in: you talked over Ghost and it stopped";
    case "wake":
      return `Wake attempt, score ${(d.score ?? 0).toFixed(2)}: ${d.woke == null ? "being checked" : d.woke ? "woke Ghost" : "refused by the check"}`;
    case "near_miss":
      return `Near miss, best score ${(d.score ?? 0).toFixed(2)}`;
  }
}

/** Which tone a decision wears in the list. */
export function decisionTone(d: MeterView["decisions"][number]): "ok" | "held" | "drop" | "wake" {
  switch (d.kind) {
    case "taken":
    case "taken_by_name":
    case "barge_in":
      return "ok";
    case "held_room":
    case "let_go":
      return "held";
    case "wake":
    case "near_miss":
      return "wake";
    default:
      return "drop";
  }
}
