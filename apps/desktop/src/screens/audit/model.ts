/**
 * Owner: audit. Pure helpers: what a write's status means in words, what each gate event
 * means in words, how rows group by day, and what the search matches. No React here.
 */
import type { AuditEntry, Schemas } from "@ghost/client-ts";
import { autonomyLabel } from "@ghost/ui";

import { type Tone, statusLabel, statusTone } from "../../ui";

export type AuditEvent = Schemas["AuditEventView"];

/* ---------------------------------------------------------------------------- status */

/**
 * How long a confirm-level change can wait for a yes (ghost_core.models.PROPOSAL_TTL is 60
 * seconds), with a little room for the clock and the event stream. A stored "pending" older
 * than this, that isn't the change the daemon says is waiting now, lapsed without an expiry
 * event (a restart, a crash while it waited): nothing is waiting, and the row says so.
 */
export const CONFIRM_WINDOW_MS = 90_000;

/** What the screen knows beyond the row: the id the daemon reports as waiting now, and the time. */
export interface OutcomeContext {
  liveId?: string | null;
  now?: number;
}

/**
 * The outcome to show. The gate stores `executed` when a write finished, even if a step
 * failed; the `finished` event says how it went. So a write whose finished event reads
 * failed or partial shows as Failed or Partly done, whatever its stored status. And a stored
 * `pending` is only Waiting when it is the change the daemon is waiting on (status.pending),
 * or, with no daemon word, young enough to still be inside the confirmation window; anything
 * older lapsed and reads as such.
 */
export function outcomeOf(e: AuditEntry, ctx: OutcomeContext = {}): string {
  if (e.status === "executed" || e.status === "success") {
    const fin = [...e.events].reverse().find((ev) => ev.event === "finished");
    const s = fin?.detail?.status;
    if (s === "failed" || s === "partial") return s;
    return "executed";
  }
  if (e.status === "pending") {
    if (ctx.liveId !== undefined && ctx.liveId !== null) return ctx.liveId === e.id ? "pending" : "lapsed";
    const age = (ctx.now ?? Date.now()) - new Date(e.created_at).getTime();
    return age <= CONFIRM_WINDOW_MS ? "pending" : "lapsed";
  }
  return e.status;
}

const LOCAL: Record<string, { label: string; tone: Tone }> = {
  // Not in the shared map yet (see shared_requests): partial and the legacy "success".
  partial: { label: "Partly done", tone: "warn" },
  success: { label: "Done", tone: "ok" },
  // Not a gate status: a stored "pending" that lapsed without an expiry event (see outcomeOf).
  lapsed: { label: "Expired", tone: "neutral" },
};

export function outcomeLabel(outcome: string): string {
  return LOCAL[outcome]?.label ?? statusLabel(outcome);
}

export function outcomeTone(outcome: string): Tone {
  return LOCAL[outcome]?.tone ?? statusTone(outcome);
}

/** The status filter. "attention" is every outcome the operator may want to look at. */
export const ATTENTION = new Set(["failed", "partial", "stale", "pending", "claimed"]);
export const STATUS_ORDER = ["executed", "pending", "claimed", "failed", "partial", "stale", "expired", "lapsed", "cancelled"];

/* ---------------------------------------------------------------------------- words */

/** "github_heka.close": the source (a server's name, the agents or the departments) and the operation. */
export function opName(e: AuditEntry): string {
  return `${e.ability}.${e.operation}`;
}

/** Source names read as words in the filter: the built-in ones are "Agents" and "Departments", a server goes by its humanized name ("GitHub heka"; a department's own as "marketing/figma"). */
export function sourceLabel(name: string): string {
  if (name === "conversations") return "Agents";
  if (name === "departments") return "Departments";
  const s = name.replace(/[_-]+/g, " ");
  return (s[0].toUpperCase() + s.slice(1)).replace(/\bgithub\b/i, "GitHub");
}

/**
 * A row's summary in parts, target first: "ghost-mock/app#151", then what Ghost did,
 * "closed the issue, with a comment: …". The gate writes "On <target>: <action>"; the "On"
 * and the colon go, so the column reads as a list of things, not a repeated prefix.
 */
export interface SummaryParts {
  /** What it was done to, at medium weight: "ghost-mock/app#151", "Standup". Empty when the summary names none. */
  subject: string;
  /** What Ghost did, in --text-1. */
  action: string;
}

const upperFirst = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s);

/** "On ghost-mock/app#151: closed the issue." -> subject and action. "the Standup line" reads "Standup line". */
function splitOn(summary: string): { subject: string; action: string } | null {
  const m = /^On\s+(.+?):\s+([\s\S]*)$/.exec(summary);
  if (!m) return null;
  return { subject: m[1].replace(/^the\s+/, ""), action: m[2] };
}

/** A sentence's closing period goes: the row is a list item, not prose. Quotes keep theirs. */
const trimStop = (s: string) => s.replace(/\.$/, "");

/** "On ghost-mock/app#151: closed the issue." -> subject "ghost-mock/app#151", action "closed the issue". */
export function splitSummary(e: AuditEntry): SummaryParts {
  const on = splitOn(e.summary);
  return on ? { subject: on.subject, action: trimStop(on.action) } : { subject: "", action: trimStop(e.summary) };
}

/* ---------------------------------------------------------------------------- steps */

/**
 * One step as the gate wrote it (the source's describe()), in the operator's words.
 * `done` titles an applied step, `todo` follows "Couldn’t" for a failed one, `quote` is the
 * text the step carried (a comment's body, a pinned note). Unknown steps keep their words,
 * sentence-cased.
 */
export interface StepView {
  done: string;
  todo: string;
  quote?: string;
  known: boolean;
}

const unquote = (s: string) => s.trim().replace(/^(['"])([\s\S]*)\1$/, "$2");

export function stepView(raw: string): StepView {
  const s = raw.trim();
  let m: RegExpExecArray | null;
  // github_issues
  if ((m = /^comment:\s*([\s\S]*)$/.exec(s))) return { done: "Comment added", todo: "add the comment", quote: m[1] || undefined, known: true };
  if (s === "closed the issue") return { done: "Closed the issue", todo: "close the issue", known: true };
  if ((m = /^labels:\s*([\s\S]*)$/.exec(s))) return { done: `Labels: ${m[1]}`, todo: `change the labels (${m[1]})`, known: true };
  // A bare operation name, as the mock gate writes it and a source may: the same words.
  if (s === "close") return { done: "Closed the issue", todo: "close the issue", known: true };
  if (s === "reopen") return { done: "Reopened the issue", todo: "reopen the issue", known: true };
  if (s === "approve") return { done: "Approved", todo: "approve it", known: true };
  if (s === "request_changes") return { done: "Requested changes", todo: "request changes", known: true };
  if (s === "comment") return { done: "Comment added", todo: "add the comment", known: true };
  if (s === "set_labels") return { done: "Labels changed", todo: "change the labels", known: true };
  // agents (ADR 0024)
  if ((m = /^hire\s+(.+?)\s+in\s+(\S+)(.*)$/.exec(s))) return { done: `Hired ${unquote(m[1])} in ${m[2]}`, todo: `hire ${unquote(m[1])}`, known: true };
  if ((m = /^start conversation\s+(.+)$/.exec(s))) return { done: `Started ${unquote(m[1])}`, todo: `start ${unquote(m[1])}`, known: true };
  if (s === "retire the agent") return { done: "Retired", todo: "retire the agent", known: true };
  if (s === "bring the agent back") return { done: "Rehired", todo: "rehire the agent", known: true };
  if ((m = /^move to\s+(.+)$/.exec(s))) return { done: `Moved to ${m[1]}`, todo: `move it to ${m[1]}`, known: true };
  if ((m = /^add the rule:\s*([\s\S]*)$/.exec(s))) return { done: "Rule added", todo: "add the rule", quote: m[1] || undefined, known: true };
  if ((m = /^the department remembers:\s*([\s\S]*)$/.exec(s))) return { done: "The department remembers", todo: "add the shared fact", quote: m[1] || undefined, known: true };
  if ((m = /^rename to\s+(.+)$/.exec(s))) return { done: `Renamed to ${unquote(m[1])}`, todo: `rename it to ${unquote(m[1])}`, known: true };
  if ((m = /^voice\s+(\S+)\s+at\s+(\S+?)x$/.exec(s))) return { done: `Voice set to ${m[1]} at ${m[2]}×`, todo: `set the voice to ${m[1]}`, known: true };
  if ((m = /^persona:\s*([\s\S]*)$/.exec(s))) return { done: "Persona set", todo: "set the persona", quote: unquote(m[1]) || undefined, known: true };
  if (s === "clear persona") return { done: "Persona cleared", todo: "clear the persona", known: true };
  if ((m = /^remember:\s*([\s\S]*)$/.exec(s))) return { done: "Pinned a note", todo: "pin the note", quote: m[1] || undefined, known: true };
  if (s === "update notes") return { done: "Updated the notes", todo: "update the notes", known: true };
  if (s === "compact") return { done: "Compacted the memory", todo: "compact the memory", known: true };
  if (s === "edit memory") return { done: "Edited the memory", todo: "edit the memory", known: true };
  if (s === "reset: forget everything") return { done: "Reset · forgot everything", todo: "reset the agent", known: true };
  if (s === "delete the agent" || s === "delete the conversation") return { done: "Deleted the agent", todo: "delete the agent", known: true };
  if (s === "voice: the department's") return { done: "Voice set to the department's", todo: "set the voice to the department's", known: true };
  return { done: upperFirst(s), todo: s, known: false };
}

/** The operator's answer to a confirm-level write. Null when they never answered in words or with a click. */
export function answerOf(e: AuditEntry): { verdict: "yes" | "no" | "unclear"; ui: boolean; via: string } | null {
  const turn = e.events.find((ev) => ev.event === "user_turn");
  if (!turn) return null;
  const d = turn.detail ?? {};
  const verdict = d.verdict === "affirmative" ? "yes" : d.verdict === "negative" ? "no" : "unclear";
  return { verdict, ui: d.classifier === "ui", via: classifierLabel(String(d.classifier ?? "")) };
}

export function classifierLabel(c: string): string {
  if (c === "ui") return "the Confirm button";
  if (c === "jev") return "Jev";
  if (c === "regex") return "the word list";
  return c || "voice";
}

/**
 * Why the gate canceled a waiting write, in the operator's words. `short` is for the row's
 * meta line, `title` for the timeline. The reasons are the ones the gate and the voice
 * pipeline write: the classifier's verdict, a spoken "never mind" (operator cancelled),
 * sleep, switching lines, and a Cancel click in the app.
 */
export function cancelReason(reason: string): { short: string; title: string } | null {
  switch (reason) {
    case "negative":
      return { short: "you said no", title: "Canceled because you said no" };
    case "unclear":
      return { short: "no clear yes", title: "Canceled because the answer wasn’t a clear yes" };
    case "operator cancelled":
      return { short: "you said to drop it", title: "Canceled when you said to drop it" };
    case "went to sleep":
    case "session ended":
      return { short: "Ghost went to sleep", title: "Canceled when Ghost went to sleep" };
    case "switched conversation":
      return { short: "you switched agents", title: "Canceled when you switched agents" };
    case "ui":
    case "cancelled from the desktop app":
      return { short: "you clicked Cancel", title: "Canceled from the app" };
    default:
      return null;
  }
}

function cancelOf(e: AuditEntry): string | null {
  const ev = e.events.find((x) => x.event === "cancelled");
  return ev ? String(ev.detail?.reason ?? "") : null;
}

/** The meta line's level parts: ["Ask me first", "you said yes"]. */
export function levelParts(e: AuditEntry, outcome?: string): string[] {
  const level = autonomyLabel(e.autonomy);
  if (outcome === "lapsed") return [level, "no answer"];
  const a = answerOf(e);
  if (a) {
    if (a.ui) return [level, a.verdict === "yes" ? "you clicked Confirm" : "you clicked Cancel"];
    return [level, a.verdict === "yes" ? "you said yes" : a.verdict === "no" ? "you said no" : "no clear yes"];
  }
  const reason = cancelOf(e);
  if (reason != null) {
    const r = cancelReason(reason);
    return r ? [level, r.short] : [level, "canceled"];
  }
  return e.status === "expired" ? [level, "no answer"] : [level];
}

/** One gate event in plain words, with optional quoted speech, mono detail and tone. */
export interface EventView {
  title: string;
  quote?: string;
  detail?: string;
  tone?: Tone;
}

const STEP = (d: Record<string, unknown>) => (d.step != null ? stepView(String(d.step)) : null);

/** `entry` is the row the event belongs to: a target the row already shows isn't repeated. */
export function describeEvent(ev: AuditEvent, entry?: AuditEntry): EventView {
  const d = (ev.detail ?? {}) as Record<string, unknown>;
  switch (ev.event) {
    case "prepared": {
      const t = d.target ? String(d.target) : "";
      const shown = !t || (entry && (t === entry.target || t.endsWith(`:${entry.target}`)));
      return { title: "Ghost prepared it", detail: shown ? undefined : t };
    }
    case "user_turn": {
      const yes = d.verdict === "affirmative";
      const via = classifierLabel(String(d.classifier ?? ""));
      if (d.classifier === "ui") return { title: yes ? "You clicked Confirm" : "You clicked Cancel", tone: yes ? "ok" : undefined };
      return {
        title: yes ? "You said yes" : d.verdict === "negative" ? "You said no" : "Your answer wasn’t a clear yes",
        quote: d.text ? String(d.text) : undefined,
        detail: `heard by ${via}`,
        tone: yes ? "ok" : undefined,
      };
    }
    case "cancelled": {
      const r = String(d.reason ?? "");
      // The operator's answer is the line above: don't say it twice.
      if (entry?.events.some((x) => x.event === "user_turn" && x.at <= ev.at) && (r === "negative" || r === "unclear" || r === "cancelled" || r === "")) {
        return { title: "Canceled · Ghost left it alone" };
      }
      const known = cancelReason(r);
      if (known) return { title: known.title };
      // An unknown reason: say it was canceled; the gate's words go in mono, as written.
      return { title: "Canceled", detail: r && r !== "cancelled" ? r : undefined };
    }
    case "expired":
      return { title: "Expired · Ghost left it alone" };
    case "stale":
      return { title: "It changed after Ghost read it, so Ghost stopped", detail: d.was ? `${d.was} → ${d.now}` : undefined, tone: "warn" };
    case "claimed":
      return { title: "Started writing" };
    case "step_applied": {
      // The dot says it was applied; the title says what changed.
      const v = STEP(d);
      return v ? { title: v.done, quote: v.quote, tone: "ok" } : { title: `Step ${Number(d.index ?? 0) + 1} applied`, tone: "ok" };
    }
    case "step_failed": {
      const v = STEP(d);
      return {
        title: v ? `Couldn’t ${v.todo}` : `Step ${Number(d.index ?? 0) + 1} failed`,
        quote: v?.quote,
        detail: d.error ? String(d.error) : undefined,
        tone: "danger",
      };
    }
    case "readback_failed":
      return { title: "Couldn’t read it back to check", detail: d.error ? String(d.error) : undefined, tone: "warn" };
    case "finished": {
      const s = d.status;
      if (s === "failed") return { title: "Stopped · nothing was changed", tone: "danger" };
      if (s === "partial") return { title: "Stopped partway · some steps applied", tone: "warn" };
      return { title: "Finished and read back", tone: "ok" };
    }
    default: {
      const t = ev.event.replace(/_/g, " ");
      return { title: t[0].toUpperCase() + t.slice(1), detail: ev.detail ? JSON.stringify(ev.detail) : undefined };
    }
  }
}

/* ---------------------------------------------------------------------------- days */

export function dayKey(iso: string): string {
  const d = new Date(iso);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

/** "Today", "Yesterday", "Thursday", then "Thursday, Oct 9" (with the year when it isn't this one). */
export function dayLabel(iso: string, now = Date.now()): string {
  const d = new Date(iso);
  const today = new Date(now);
  const midnight = (x: Date) => Date.UTC(x.getFullYear(), x.getMonth(), x.getDate()) / 86400000;
  const days = midnight(today) - midnight(d);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days > 1 && days < 7) return d.toLocaleDateString(undefined, { weekday: "long" });
  const sameYear = d.getFullYear() === today.getFullYear();
  return d.toLocaleDateString(undefined, sameYear ? { weekday: "long", month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" });
}

export function groupByDay(items: AuditEntry[]): { key: string; label: string; items: AuditEntry[] }[] {
  const out: { key: string; label: string; items: AuditEntry[] }[] = [];
  for (const e of items) {
    const key = dayKey(e.created_at);
    const last = out[out.length - 1];
    if (last && last.key === key) last.items.push(e);
    else out.push({ key, label: dayLabel(e.created_at), items: [e] });
  }
  return out;
}

/* ---------------------------------------------------------------------------- search */

/** Everything a search can match on one row: words on screen plus the raw names. */
export function haystack(e: AuditEntry, ctx: OutcomeContext = {}): string {
  const o = outcomeOf(e, ctx);
  const sp = splitSummary(e);
  const parts = [
    e.summary,
    [sp.subject, sp.action].filter(Boolean).join(" "),
    e.target,
    opName(e),
    e.status,
    o,
    outcomeLabel(o),
    ...levelParts(e, o),
    sourceLabel(e.ability),
    e.department ?? "",
    e.id,
    ...e.events.flatMap((ev) => {
      const v = describeEvent(ev, e);
      return [ev.event, v.title, v.quote ?? "", v.detail ?? ""];
    }),
  ];
  return parts.join("\n").toLowerCase();
}

/** Every space-separated term must appear somewhere on the row. */
export function matches(hay: string, query: string): boolean {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  return terms.every((t) => hay.includes(t));
}

/* ---------------------------------------------------------------------------- url */

/**
 * Initial filters from the query string (before the hash), so every state has a URL:
 * `?mock=busy&status=attention&q=safari#/audit`. Read once on mount; never written back,
 * because the query survives navigation to other routes.
 */
export function initialParams(): { status: string; ability: string; q: string; open: string | null; fail: boolean; failMore: boolean } {
  const p = new URLSearchParams(window.location.search);
  const fail = p.get("fail");
  return { status: p.get("status") ?? "all", ability: p.get("ability") ?? "all", q: p.get("q") ?? "", open: p.get("open"), fail: fail === "audit", failMore: fail === "audit-more" };
}

