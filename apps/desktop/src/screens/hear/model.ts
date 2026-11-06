/**
 * Owner: hear. The Hearing test's plain logic (ADR 0042), no React: the take list in order,
 * which take comes next, what state a take is in, and the words the screen shows. Tested in
 * test/hear.test.ts.
 */
import type { HearBlock, HearPlan, HearPlanTake, HearSession, HearTake } from "@ghost/client-ts";

/** A take in the take list, with its block and its place in the whole list. */
export interface PlanEntry {
  take: HearPlanTake;
  block: HearBlock;
  /** 0-based, across every block. */
  index: number;
  /** The first take of its block. */
  first: boolean;
}

/** Every take in the take list, in order. */
export function entriesOf(plan: HearPlan | null | undefined): PlanEntry[] {
  const out: PlanEntry[] = [];
  for (const block of plan?.blocks ?? []) {
    block.takes.forEach((take, i) => out.push({ take, block, index: out.length, first: i === 0 }));
  }
  return out;
}

/** A take's state in a session: what the take strip shows. */
export type TakeStatus = "todo" | "recorded" | "flagged" | "skipped";

/** The check's warnings that make a take wrong, not just worth a look: the ones that ask for a redo. */
export const FLAG_KINDS = ["no_speech", "started_early", "cut_off", "clipping", "speech_in_silence"] as const;

export function takeStatus(t: HearTake | undefined): TakeStatus {
  if (!t) return "todo";
  if (t.state === "skipped") return "skipped";
  return (t.warnings ?? []).some((w) => (FLAG_KINDS as readonly string[]).includes(w)) ? "flagged" : "recorded";
}

export const STATUS_WORDS: Record<TakeStatus, string> = {
  todo: "To do",
  recorded: "Recorded",
  flagged: "Worth a redo",
  skipped: "Skipped",
};

/** The session's takes by id (a take's last line wins, as the daemon already resolves them). */
export function takesById(session: HearSession | null | undefined): Map<string, HearTake> {
  return new Map((session?.takes ?? []).map((t) => [t.id, t]));
}

const done = (t: HearTake | undefined) => !!t && (t.state === "recorded" || t.state === "skipped");

/**
 * The take to do next: the first after `current` that is neither recorded nor skipped,
 * then from the top; null when every take is done. With no current take, the first not done.
 */
export function nextTake(plan: HearPlan | null | undefined, session: HearSession | null | undefined, current: string | null): string | null {
  const all = entriesOf(plan);
  const byId = takesById(session);
  const at = current ? all.findIndex((e) => e.take.id === current) : -1;
  for (let k = 1; k <= all.length; k++) {
    const e = all[(at + k + all.length) % all.length];
    if (!e) break;
    if (e.take.id !== current && !done(byId.get(e.take.id))) return e.take.id;
  }
  return null;
}

/** Counts for a session against the take list. */
export function progressOf(plan: HearPlan | null | undefined, session: HearSession | null | undefined) {
  const all = entriesOf(plan);
  const byId = takesById(session);
  let recorded = 0, skipped = 0, flagged = 0;
  for (const e of all) {
    const s = takeStatus(byId.get(e.take.id));
    if (s === "recorded" || s === "flagged") recorded++;
    if (s === "flagged") flagged++;
    if (s === "skipped") skipped++;
  }
  return { total: all.length, recorded, skipped, flagged, left: all.length - recorded - skipped };
}

/** A line split at its pauses (" ... " is about a second): the words, with a pause between each piece. */
export function lineParts(text: string): string[] {
  return text.split(/\s*\.\.\.\s*/).map((p) => p.trim()).filter(Boolean);
}

/** Text with `code` in backticks, as pieces: a report's titles and sentences. */
export function splitCode(text: string): { code: boolean; text: string }[] {
  const out: { code: boolean; text: string }[] = [];
  const re = /`([^`]+)`/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push({ code: false, text: text.slice(last, m.index) });
    out.push({ code: true, text: m[1] });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ code: false, text: text.slice(last) });
  return out;
}

/** A word error rate (a fraction) as the report writes it: 0.072 is "7.2%"; none is "–". */
export function pct(x: number | null | undefined): string {
  return x == null || !Number.isFinite(x) ? "–" : `${(x * 100).toFixed(1)}%`;
}

/** dBFS, rounded: "−12 dBFS". The minus is a real one. */
export function dbfs(x: number | null | undefined): string {
  if (x == null || !Number.isFinite(x)) return "–";
  const n = Math.round(x);
  return `${n < 0 ? "−" : ""}${Math.abs(n)} dBFS`;
}

/** Seconds, short: "4.2 s"; a minute and more as "12 min". */
export function secs(x: number | null | undefined): string {
  if (x == null || !Number.isFinite(x)) return "–";
  if (x >= 90) return `${Math.round(x / 60)} min`;
  return `${x.toFixed(1)} s`;
}

/** Where speech sits in a take, as fractions of its length, for the bar under a check. */
export function speechSpan(start: number | null | undefined, end: number | null | undefined, seconds: number): { from: number; to: number } | null {
  if (start == null || end == null || !(seconds > 0)) return null;
  const clamp = (v: number) => Math.max(0, Math.min(1, v / seconds));
  return { from: clamp(start), to: Math.max(clamp(start), clamp(end)) };
}

/** A run report's take key, `<id>|<source>`: the take id. */
export function takeOfKey(key: string | null | undefined): string | null {
  if (!key) return null;
  const id = key.split("|")[0]?.trim();
  return id || null;
}

/** The daemon's refusal in words: a sentence, or a validation error's messages. */
export function refusal(e: unknown): string {
  const detail = (e as { detail?: unknown } | null)?.detail;
  if (typeof detail === "string") return detail;
  if (Array.isArray(detail)) {
    const msgs = detail.map((d) => {
      const x = d as { msg?: unknown; loc?: unknown };
      const where = Array.isArray(x.loc) ? x.loc.filter((p) => p !== "body").join(".") : "";
      return typeof x.msg === "string" ? (where ? `${where}: ${x.msg}` : x.msg) : null;
    });
    const text = msgs.filter(Boolean).join("; ");
    if (text) return text;
  }
  return e instanceof Error ? e.message : String(e);
}

/** True for an ApiError with this status. */
export function isStatus(e: unknown, status: number): boolean {
  return (e as { status?: unknown } | null)?.status === status;
}

// ------------------------------------------------------------------ editing the take list

/** Move an item in a list by `by` places; the same list when it can't move. */
export function moved<T>(list: T[], i: number, by: number): T[] {
  const j = i + by;
  if (i < 0 || i >= list.length || j < 0 || j >= list.length) return list;
  const out = list.slice();
  const [x] = out.splice(i, 1);
  out.splice(j, 0, x);
  return out;
}

/** A take id not yet in the plan: the block's letter and the next free two-digit number. */
export function freeTakeId(plan: HearPlan, block: string): string {
  const used = new Set(entriesOf(plan).map((e) => e.take.id));
  const letter = block || "X";
  for (let n = 1; n < 1000; n++) {
    const id = `${letter}${String(n).padStart(2, "0")}`;
    if (!used.has(id)) return id;
  }
  return `${letter}${Date.now() % 1000}`;
}

/** A block letter not yet used: the next after the last one. */
export function freeBlockName(plan: HearPlan): string {
  const used = new Set(plan.blocks.map((b) => b.block));
  for (let c = 65; c < 91; c++) {
    const l = String.fromCharCode(c);
    if (!used.has(l)) return l;
  }
  return `B${plan.blocks.length + 1}`;
}

/** Tags or terms as the operator types them: comma-separated, trimmed, empties dropped. */
export function splitList(text: string): string[] {
  return text.split(",").map((t) => t.trim()).filter(Boolean);
}
