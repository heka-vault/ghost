/**
 * Today's model: what needs the operator, what happened, and what is happening, from the
 * daemon's own records (the waiting list, the audit, the runs, the questions). Pure functions,
 * so the screen, the room and the tests read the same day the same way.
 *
 * Every line here is text from the operator's own Ghost, but titles, answers and summaries can
 * carry an outside system's words: they are shown as text, never as markup.
 */
import type { AppView, AuditEntry, ModelCheckView, QuestionView, ScheduleRunInfo, WaitingItemView } from "@ghost/client-ts";

// ---------------------------------------------------------------- what needs you

/** What the operator does about an open item: decide a change, answer a question, or know it. */
export type NeedKind = "decide" | "answer" | "know";

/** The kind of an open item, or null when it is not open (heard, or put off until later). */
export function needKind(i: WaitingItemView, now: number = Date.now()): NeedKind | null {
  if (i.state === "acknowledged") return null;
  if (i.state === "snoozed" && i.snoozed_until && Date.parse(i.snoozed_until) > now) return null;
  if (i.change) return "decide";
  if (i.question) return i.question.state === "open" ? "answer" : null;
  return "know";
}

export interface Needs {
  decide: WaitingItemView[];
  answer: WaitingItemView[];
  know: WaitingItemView[];
}

/** The open items by what the operator does about them, oldest decision first, newest news first. */
export function needsOf(items: WaitingItemView[], now: number = Date.now()): Needs {
  const out: Needs = { decide: [], answer: [], know: [] };
  for (const i of items) {
    const k = needKind(i, now);
    if (k) out[k].push(i);
  }
  // A decision and a question age toward their expiry: the one that runs out first leads.
  const expiry = (i: WaitingItemView) => Date.parse(i.change?.expires_at ?? i.question?.expires_at ?? "") || Infinity;
  out.decide.sort((a, b) => expiry(a) - expiry(b));
  out.answer.sort((a, b) => expiry(a) - expiry(b));
  out.know.sort((a, b) => b.first_seen.localeCompare(a.first_seen));
  return out;
}

/** The change's words without the waiting list's prefix: “On ghost#17: close the issue.” */
export function needLine(i: WaitingItemView): string {
  if (i.change) return i.change.summary || i.title.replace(/^Waiting for your yes: /, "");
  if (i.question) return i.question.question;
  return i.title;
}

// ---------------------------------------------------------------- the day

/** Midnight to midnight, local time, for the day that holds `day`. */
export function dayBounds(day: Date): { start: number; end: number } {
  const s = new Date(day);
  s.setHours(0, 0, 0, 0);
  const e = new Date(s);
  e.setDate(e.getDate() + 1);
  return { start: s.getTime(), end: e.getTime() };
}

/** The same day `n` days away (negative goes back). */
export function shiftDay(day: Date, n: number): Date {
  const d = new Date(day);
  d.setHours(12, 0, 0, 0);
  d.setDate(d.getDate() + n);
  return d;
}

export function isSameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

/** “Today”, “Yesterday”, else “Tuesday, November 10”. */
export function dayLabel(day: Date, now: Date = new Date()): string {
  if (isSameDay(day, now)) return "Today";
  if (isSameDay(day, shiftDay(now, -1))) return "Yesterday";
  return day.toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" });
}

// ---------------------------------------------------------------- the timeline

/** One moment on the timeline. Kinds map to one mark each (docs/design-system.md, The timeline). */
export type MomentKind = "change" | "run" | "watch" | "question" | "answer" | "note";
export type MomentTone = "ok" | "failed" | "running" | "quiet" | "neutral";

export interface Moment {
  id: string;
  kind: MomentKind;
  /** ISO time the moment happened (a run: when it started). */
  at: string;
  /** A run's end, when it has ended. */
  ends?: string | null;
  /** The agent it belongs to (slug), when one. */
  agent: string | null;
  title: string;
  /** A second line: what a run answered, the comment on a change, a question's detail. */
  detail?: string | null;
  tone: MomentTone;
  /** A run's work: model calls and tool calls. */
  calls?: { model: number; tool: number };
  link?: string | null;
  /** A cluster's members, newest first. */
  children?: Moment[];
  /** Mono detail: the operation, the watch's name, the run's source. */
  mono?: string | null;
}

export interface DayRecords {
  audit: AuditEntry[];
  runs: ScheduleRunInfo[];
  waiting: WaitingItemView[];
  questions: QuestionView[];
}

const SILENT = "[SILENT]";
const DONE_RUN = new Set(["ok", "unchanged"]);
const FAILED_RUN = new Set(["failed", "timed_out", "interrupted", "missed"]);

/** A run's tone: going, ended well, ended with nothing to say, or ended badly. */
export function runTone(status: string): MomentTone {
  if (status === "running" || status === "queued") return "running";
  if (status === "unchanged" || status === "cancelled") return "quiet";
  if (FAILED_RUN.has(status)) return "failed";
  return DONE_RUN.has(status) ? "ok" : "neutral";
}

/** What a run answered, in words: “[SILENT]” and an unchanged check say nothing new. */
export function runAnswer(r: ScheduleRunInfo): string | null {
  if (r.error) return r.error;
  if (r.status === "unchanged") return "Nothing had changed since the last look, so it didn’t need the model.";
  if (!r.answer || r.answer.trim() === SILENT) return r.status === "ok" ? "Nothing to report." : null;
  return r.answer;
}

/** Why a run started, in a few words: its schedule, a watch, or the operator. */
export function runReason(r: ScheduleRunInfo): string {
  if (r.source.startsWith("watch:")) return `for the ${r.source.slice(6)} watch`;
  if (r.source === "run_now") return "when asked";
  return "on its schedule";
}

/** The agent behind an audit entry: the conversation it was made in, Ghost when none. */
function auditAgent(a: AuditEntry): string {
  return a.conversation || "ghost";
}

const inDay = (iso: string | null | undefined, b: { start: number; end: number }) => {
  if (!iso) return false;
  const t = Date.parse(iso);
  return t >= b.start && t < b.end;
};

/**
 * The day's moments, newest first: every change Ghost or an agent made or tried (not the one
 * still waiting, which is in Decide), every run, everything a watch or webhook found, the
 * questions asked and answered, and what outside tools and apps said.
 */
export function momentsOf(r: DayRecords, day: Date): Moment[] {
  const b = dayBounds(day);
  const out: Moment[] = [];
  for (const a of r.audit) {
    if (a.status === "pending" || !inDay(a.created_at, b)) continue;
    const tone: MomentTone = a.status === "executed" ? "ok" : a.status === "failed" ? "failed" : "quiet";
    out.push({ id: `c-${a.id}`, kind: "change", at: a.created_at, agent: auditAgent(a), title: a.summary, tone, mono: `${a.source}.${a.operation}`, detail: tone === "quiet" ? changeAside(a.status) : null });
  }
  for (const run of r.runs) {
    const at = run.started_at ?? run.scheduled_for;
    if (!inDay(at, b)) continue;
    out.push({
      id: `r-${run.id}`,
      kind: "run",
      at,
      ends: run.finished_at ?? null,
      agent: run.agent,
      title: run.name,
      detail: runAnswer(run),
      tone: runTone(run.status),
      calls: { model: run.model_calls, tool: run.tool_calls },
      mono: runReason(run),
    });
  }
  for (const i of r.waiting) {
    if (!inDay(i.first_seen, b) || i.change || i.question) continue;
    const [kind, rest] = i.source.includes(":") ? i.source.split(/:(.*)/s, 2) : [i.source, ""];
    // A run's answer is the run itself on the timeline.
    if (kind === "agent") continue;
    const watch = kind === "watch" || kind === "hook";
    out.push({ id: `w-${i.key}`, kind: watch ? "watch" : "note", at: i.first_seen, agent: null, title: i.title, tone: "neutral", link: i.link, mono: watch ? `${kind === "hook" ? "webhook" : "watch"} ${rest}` : kind === "client" ? `outside tool ${rest}` : kind === "app" ? `app ${rest}` : null });
  }
  for (const q of r.questions) {
    if (q.kind !== "question") continue;
    if (inDay(q.asked_at, b)) out.push({ id: `q-${q.key}`, kind: "question", at: q.asked_at, agent: q.source.startsWith("agent:") ? q.source.slice(6).split(":")[0] : null, title: q.question, mono: q.asked_by, tone: q.state === "open" ? "running" : "neutral" });
    if (q.answered_at && inDay(q.answered_at, b)) {
      const said = q.answer ?? q.choice;
      out.push({ id: `a-${q.key}`, kind: "answer", at: q.answered_at, agent: null, title: said ? `You answered ${q.asked_by}: ${said}` : `You declined ${q.asked_by}’s question`, tone: "ok", mono: q.answered_via ? `by ${q.answered_via}` : null });
    }
  }
  return out.sort((x, y) => y.at.localeCompare(x.at) || x.id.localeCompare(y.id));
}

function changeAside(status: string): string {
  if (status === "cancelled" || status === "canceled") return "You said no; nothing changed.";
  if (status === "expired") return "Nobody answered in time; Ghost left it alone.";
  if (status === "stale") return "It had changed first; Ghost left it alone.";
  return "";
}

/**
 * Folds a run of changes by one agent, each within `gapMin` minutes of the next, into one
 * moment (“Legal made 3 changes”), so a busy agent reads as one thing it did, opened to each.
 */
export function cluster(moments: Moment[], gapMin = 20): Moment[] {
  const out: Moment[] = [];
  let group: Moment[] = [];
  const flush = () => {
    if (group.length >= 3) {
      const failed = group.filter((m) => m.tone === "failed").length;
      out.push({
        id: `g-${group[0].id}`,
        kind: "change",
        at: group[0].at,
        ends: group[group.length - 1].at,
        agent: group[0].agent,
        title: `${group.length} changes`,
        detail: failed ? `${failed} didn’t go through` : null,
        tone: failed ? "failed" : "ok",
        children: group,
      });
    } else out.push(...group);
    group = [];
  };
  for (const m of moments) {
    const last = group[group.length - 1];
    const joins = m.kind === "change" && last && last.agent === m.agent && Date.parse(last.at) - Date.parse(m.at) <= gapMin * 60_000;
    if (!joins) flush();
    if (m.kind === "change") group.push(m);
    else out.push(m);
  }
  flush();
  return out;
}

export interface HourBlock {
  /** 0 to 23. */
  hour: number;
  moments: Moment[];
  /** Hours with nothing since the block before (newer) it, for a compressed gap. */
  quietBefore: number;
}

/** Moments (newest first) grouped by their local hour, with the empty hours between counted. */
export function byHour(moments: Moment[]): HourBlock[] {
  const blocks: HourBlock[] = [];
  for (const m of moments) {
    const h = new Date(m.at).getHours();
    const last = blocks[blocks.length - 1];
    if (last && last.hour === h) last.moments.push(m);
    else blocks.push({ hour: h, moments: [m], quietBefore: last ? Math.max(0, last.hour - h - 1) : 0 });
  }
  return blocks;
}

/** “9 AM”, “12 PM”. */
export function hourLabel(h: number): string {
  const n = h % 12 === 0 ? 12 : h % 12;
  return `${n} ${h < 12 ? "AM" : "PM"}`;
}

/** How many moments of each kind in each hour of the day: the scrubber's bars. */
export function density(moments: Moment[]): { hour: number; count: number; failed: number }[] {
  const hours = Array.from({ length: 24 }, (_, hour) => ({ hour, count: 0, failed: 0 }));
  for (const m of moments) {
    const h = hours[new Date(m.at).getHours()];
    const n = m.children?.length ?? 1;
    h.count += n;
    if (m.tone === "failed") h.failed += 1;
  }
  return hours;
}

export type MomentFilter = "all" | "changes" | "runs" | "watches" | "questions";

export function filterMoments(moments: Moment[], f: MomentFilter, agent: string | null = null): Moment[] {
  const kinds: Record<MomentFilter, MomentKind[] | null> = {
    all: null,
    changes: ["change"],
    runs: ["run"],
    watches: ["watch", "note"],
    questions: ["question", "answer"],
  };
  const k = kinds[f];
  return moments.filter((m) => (!k || k.includes(m.kind)) && (!agent || m.agent === agent));
}

// ---------------------------------------------------------------- the day in numbers

export interface DayCounts {
  decide: number;
  answer: number;
  know: number;
  changes: number;
  failedChanges: number;
  runs: number;
  modelCalls: number;
  toolCalls: number;
  found: number;
  answered: number;
}

export function countsOf(needs: Needs, moments: Moment[]): DayCounts {
  const flat = moments.flatMap((m) => m.children ?? [m]);
  const runs = flat.filter((m) => m.kind === "run");
  return {
    decide: needs.decide.length,
    answer: needs.answer.length,
    know: needs.know.length,
    changes: flat.filter((m) => m.kind === "change" && m.tone === "ok").length,
    failedChanges: flat.filter((m) => m.kind === "change" && m.tone === "failed").length,
    runs: runs.length,
    modelCalls: runs.reduce((n, m) => n + (m.calls?.model ?? 0), 0),
    toolCalls: runs.reduce((n, m) => n + (m.calls?.tool ?? 0), 0),
    found: flat.filter((m) => m.kind === "watch").length,
    answered: flat.filter((m) => m.kind === "answer").length,
  };
}

const plural = (n: number, one: string, many: string) => `${n === 0 ? "no" : n} ${n === 1 ? one : many}`;

/**
 * Today in a sentence, with real counts: the lead is what the operator has to do, the rest is
 * what Ghost did. “2 things need you.” / “Ghost made 8 changes and ran 5 jobs today.”
 */
export function headline(c: DayCounts, today = true): { lead: string; rest: string } {
  const needs = c.decide + c.answer;
  const lead = !today
    ? c.changes + c.runs === 0
      ? "A quiet day."
      : `${cap(plural(c.changes, "change", "changes"))}, ${plural(c.runs, "run", "runs")}.`
    : needs > 0
      ? `${needs === 1 ? "One thing needs" : `${needs} things need`} you.`
      : c.know > 0
        ? `Nothing to decide. ${c.know === 1 ? "One thing" : `${c.know} things`} to know.`
        : "Nothing needs you.";
  const did: string[] = [];
  if (c.changes) did.push(`made ${plural(c.changes, "change", "changes")}`);
  if (c.runs) did.push(`ran ${plural(c.runs, "job", "jobs")}`);
  if (c.found) did.push(`caught ${plural(c.found, "thing", "things")} on watch`);
  const when = today ? " today" : "";
  const rest = !today ? "" : did.length ? `Ghost ${list(did)}${when}.` : "Ghost hasn’t had to do anything yet today.";
  return { lead, rest };
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function list(parts: string[]): string {
  if (parts.length <= 1) return parts.join("");
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

// ---------------------------------------------------------------- what Ghost can do

export interface Reach {
  apps: number;
  tools: number;
  /** Apps that are not connected right now (signed out, failed). */
  down: number;
}

/** The apps Ghost can reach right now, and their tools. */
export function reachOf(apps: AppView[]): Reach {
  const up = apps.filter((a) => a.state === "connected");
  return { apps: up.length, tools: up.reduce((n, a) => n + a.actions.length, 0), down: apps.filter((a) => a.state === "failed" || a.state === "needs_sign_in").length };
}

/**
 * How fast a spoken reply starts, from the last model check: the quickest reachable model's
 * median to its first word (the fast model speaks first when there is one), or null.
 */
export function replySpeed(check: ModelCheckView | null | undefined): { text: string; ms: number; when: string; model: string } | null {
  const timed = (check?.models ?? []).filter((m) => m.reachable && m.first_word?.median_ms != null);
  if (!check || !timed.length) return null;
  const best = timed.reduce((a, b) => (b.first_word.median_ms! < a.first_word.median_ms! ? b : a));
  const ms = best.first_word.median_ms!;
  return { text: `${(ms / 1000).toFixed(1)} s`, ms, when: check.checked_at, model: best.model };
}

/** Elapsed since an ISO time, as a clock: “1:12”, “14:03”, “1:02:07”. */
export function clock(fromIso: string, now: number = Date.now()): string {
  const s = Math.max(0, Math.floor((now - Date.parse(fromIso)) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** A finished run's length: “40 s”, “3 min”. */
export function took(startIso: string, endIso: string): string {
  const s = Math.max(0, Math.round((Date.parse(endIso) - Date.parse(startIso)) / 1000));
  if (s < 60) return `${s} s`;
  const m = Math.round(s / 60);
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`;
}
