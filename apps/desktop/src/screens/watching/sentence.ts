/**
 * Owner: watching. Everything Ghost does on its own, as one list of plain sentences (the owner's
 * simplification, 2026-11-14): a schedule, a watch, a watch that acts and a webhook are rows of
 * the same list, told apart by their sentence, not by tabs or four vocabularies.
 *
 * Every word here is built from typed fields. The one piece of someone's own words is a
 * schedule's instruction (the operator's, or an agent's through the gate), bounded to its first
 * sentence and 120 characters and returned apart (`quote`) so the screen marks it
 * `data-user-content`; it is redacted by the daemon as before. An app's text never reaches a
 * sentence: an app is named by its catalog title, a watch's read and a webhook's path stay in
 * its details. Pure, so it is tested (test/on-its-own.test.ts).
 */
import type { HookInfo, ScheduleInfo, ScheduleRunInfo, WatchInfo } from "@ghost/client-ts";
import { relativeTime } from "@ghost/ui";

import { clockWords } from "../settings/plain";
import type { Problem } from "./problems";
import { OUTCOME, URGENCY, count, untilWords, whenWords } from "./model";

/** One thing Ghost does on its own. The code keeps its names; the operator sees a sentence. */
export type Doing = { kind: "schedule"; key: string; s: ScheduleInfo } | { kind: "watch"; key: string; w: WatchInfo } | { kind: "hook"; key: string; h: HookInfo };

export interface Sentence {
  /** Which group of the list: at a time, or when something happens. */
  group: Group;
  /** When, or on what: the row's title (“Weekdays at 9 AM”, “New pull requests in GitHub”). */
  trigger: string;
  /** Who does it, before the quote (“Ghost”); with no quote, the whole of what it does. */
  what: string;
  /** Someone's own words, shown apart and marked `data-user-content`; none for a watch or webhook. */
  quote: string | null;
  /** The agent the row leads with. */
  agent: string;
  /** Its state now, in a few words: “Tells you with a notification · next in 3 h · last ran 2 h ago, done”. */
  state: string;
  /** Paused, broken or off: the chip in the row. */
  badge: { label: string; tone: "ok" | "warn" | "danger" | "info" | "neutral" } | null;
  /** Its switch: on or off, and why it can't be flipped here when it can't. */
  pause: { on: boolean; why: string | null };
}

/** The list's two groups (the flow review, 2026-11-14). */
export type Group = "time" | "event";
export const GROUP_TITLE: Record<Group, string> = { time: "At a time", event: "When something happens" };

/** Why a row of the operator's own file has a switch that this window can't flip. */
export const FILE_OWNS = "This one is yours, in Ghost’s folder: change it there.";
export const HOOK_OWNS = "A webhook is on while Ghost’s folder declares it: change it there.";

export interface SentenceContext {
  /** This computer's time zone, for a schedule's “when”. */
  zone: string;
  /** The person's locale, for times in their own clock (tests pin it). */
  locale?: string;
  /** An agent's name by its slug. */
  agentName: (slug: string) => string;
  /** An app's catalog title by its name (“GitHub”, not github_heka). */
  appTitle: (name: string) => string;
  now?: number;
}

/** A schedule's instruction, as the row quotes it: its first sentence, 120 characters at most. */
export const QUOTE_MAX = 120;
export function quoteOf(instruction: string): string {
  const first = instruction.trim().split(/(?<=[.!?])\s/)[0].trim().replace(/[.!?]+$/, "");
  const one = first.replace(/\s+/g, " ");
  const bounded = one.length > QUOTE_MAX ? `${one.slice(0, QUOTE_MAX - 1).trimEnd()}…` : one;
  // “Check the build” reads on as “…: check the build”; “GitHub …” and “PR …” keep their capital.
  return lowerFirst(bounded);
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
/** “Check the build” as “check the build”; a word with a capital inside (GitHub, PRs) as written. */
const ONE_CAPITAL = /^[A-Z][a-z’']+(?=[\s,.:;!?]|$)/;
const lowerFirst = (s: string) => (ONE_CAPITAL.test(s) ? s.charAt(0).toLowerCase() + s.slice(1) : s);

/** A webhook's source, by how it is checked: GitHub signs its own; otherwise the hook's name. */
function sourceOf(h: HookInfo): string {
  return h.verify === "github" ? "GitHub" : cap(h.name.replace(/[-_]+/g, " "));
}

/** “a push or a release” from a hook's events (“workflow_run” reads “workflow run”). */
function eventsWords(events: string[]): string {
  const words = events.map((e) => e.replace(/[_.]+/g, " "));
  if (words.length === 0) return "anything";
  if (words.length === 1) return words[0];
  return `${words.slice(0, -1).join(", ")} or ${words[words.length - 1]}`;
}

/**
 * What a watch looks for, from its read's name (a typed field of the operator's own file, not an
 * app's text): `list_workflow_runs` is “workflow runs”, `search_pull_requests` “pull requests”.
 * Only letters and digits, 40 characters at most; nothing left says “something”.
 */
export const READ_MAX = 40;
export function readWords(read: string): string {
  const words = read
    .toLowerCase()
    .replace(/^(list|search|get|fetch|find|read|query|poll|watch)[_.-]+/, "")
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
  if (!words) return "something";
  return words.length > READ_MAX ? words.slice(0, READ_MAX).replace(/\s+\S*$/, "") : words;
}

/** A webhook that is off, in words: never the password's name. */
function offWords(reason: string | null | undefined): string {
  if (reason && /password \S+ (is not|isn’t|isn't) set/.test(reason)) return "Off until its password is set";
  return "Off until it is set up";
}

export function sentenceOf(d: Doing, given: SentenceContext): Sentence {
  // An app with no catalog title is named by its own name, first letter up (“Stripe”, “Mail”).
  const c = { ...given, appTitle: (n: string) => (given.appTitle(n) === n ? cap(n) : given.appTitle(n)) };
  const now = c.now ?? Date.now();
  if (d.kind === "schedule") {
    const s = d.s;
    const when = whenWords(s, c.zone);
    const whenPhrase = when.mono ? `on the schedule ${when.mono}` : lowerFirst(when.text);
    const hours = s.active_hours ? `, between ${clockWords(s.active_hours.start, c.locale)} and ${clockWords(s.active_hours.end, c.locale)}` : "";
    const check = s.check ? `, only when ${c.appTitle(s.check.app)} has something new` : "";
    const last = s.last_outcome ? OUTCOME[s.last_outcome] : null;
    const state = s.running
      ? "running now"
      : !s.enabled
        ? "paused"
        : [s.next_run ? `next ${untilWords(s.next_run, now)}` : "no next run", s.last_run ? `last ran ${relativeTime(s.last_run, now)}${last ? `, ${last.label.toLowerCase()}` : ""}` : "not run yet"].join(" · ");
    return {
      group: "time",
      trigger: cap(`${whenPhrase}${hours}`),
      what: c.agentName(s.agent),
      quote: quoteOf(s.instruction || s.name),
      agent: s.agent,
      state: `Tells you ${URGENCY[s.urgency]}${check} · ${state}`,
      badge: s.running ? { label: "Running", tone: "info" } : !s.enabled ? { label: "Paused", tone: "neutral" } : last && (last.tone === "danger" || last.tone === "warn") ? { label: last.label, tone: last.tone } : null,
      pause: { on: s.enabled, why: null },
    };
  }
  if (d.kind === "watch") {
    const w = d.w;
    const app = c.appTitle(w.app);
    const actor = w.acts_through ? c.agentName(w.acts_through) : null;
    const does = w.response === "tell" ? `Ghost tells you ${URGENCY[w.urgency]}` : "Ghost keeps a record";
    const what = readWords(w.read);
    const state = w.paused
      ? "Paused"
      : w.error
        ? w.last_ok
          ? `Last worked ${relativeTime(w.last_ok, now)}`
          : w.last_run
            ? "Hasn’t worked yet"
            : "Not looked yet"
        : [w.last_run ? `Looked ${relativeTime(w.last_run, now)}` : "Not looked yet", w.open ? `${w.open} open` : null].filter(Boolean).join(" · ");
    const maker = w.made_by?.startsWith("agent:") ? c.agentName(w.made_by.slice(6)) : null;
    return {
      group: "event",
      trigger: what === "something" ? `Something new in ${app}` : `New ${what} in ${app}`,
      what: actor ? `${actor} looks into each, then ${does.replace(/^Ghost keeps/, "Ghost keeps")}` : does,
      quote: null,
      agent: w.acts_through ?? "ghost",
      state: maker ? `${state} · set up by ${maker}` : state,
      badge: w.paused ? { label: "Paused", tone: "neutral" } : w.error ? { label: "Can’t read", tone: "danger" } : null,
      pause: { on: !w.paused, why: w.made_by != null ? null : FILE_OWNS },
    };
  }
  const h = d.h;
  return {
    group: "event",
    trigger: `When ${sourceOf(h)} sends ${eventsWords(h.events)}`,
    what: "Ghost tells you",
    quote: null,
    agent: "ghost",
    state: h.on ? (h.last ? `Last heard from ${relativeTime(h.last, now)}` : "Nothing yet") : offWords(h.off_reason),
    badge: h.on ? null : { label: "Off", tone: "neutral" },
    pause: { on: h.on, why: HOOK_OWNS },
  };
}

/** The whole sentence as one string: the row's accessible name, and what the tests read. */
export function sentenceText(s: Sentence): string {
  return `${s.trigger}: ${s.quote ? `${s.what}, ${s.quote}` : s.what}`;
}

/**
 * Everything on the list, in one order: what needs a look first (broken, off), then by kind as
 * the operator meets them (schedules, watches, webhooks), each in its file's or the daemon's order.
 * Stable: nothing moves while the page is open unless its state changes.
 */
export function doingsOf(schedules: readonly ScheduleInfo[], watches: readonly WatchInfo[], hooks: readonly HookInfo[]): Doing[] {
  return [
    ...schedules.map((s): Doing => ({ kind: "schedule", key: `schedule:${s.id}`, s })),
    ...watches.map((w): Doing => ({ kind: "watch", key: `watch:${w.name}`, w })),
    ...hooks.map((h): Doing => ({ kind: "hook", key: `hook:${h.name}`, h })),
  ];
}

/** Whose a row is, for the page's agent filter: a schedule's agent, a watch's maker and actor. */
export function ownsDoing(d: Doing, slug: string): boolean {
  if (d.kind === "schedule") return d.s.agent === slug;
  if (d.kind === "watch") return d.w.made_by === `agent:${slug}` || d.w.acts_through === slug || (slug === "ghost" && !d.w.acts_through);
  return slug === "ghost";
}

/** Every agent a row belongs to, for the page's Whose filter: Ghost first, then by name. */
export function ownersOf(doings: readonly Doing[]): string[] {
  const set = new Set<string>();
  for (const d of doings) {
    if (d.kind === "schedule") set.add(d.s.agent);
    else if (d.kind === "watch") {
      set.add(d.w.acts_through ?? "ghost");
      if (d.w.made_by?.startsWith("agent:")) set.add(d.w.made_by.slice(6));
    } else set.add("ghost");
  }
  return [...set].sort((a, b) => (a === "ghost" ? -1 : b === "ghost" ? 1 : a.localeCompare(b)));
}

/** The runs going now, oldest first: the top of the page, each with Stop. */
export function goingNow(runs: readonly ScheduleRunInfo[]): ScheduleRunInfo[] {
  return runs.filter((r) => r.status === "running" || r.status === "queued").sort((a, b) => (a.started_at ?? a.scheduled_for).localeCompare(b.started_at ?? b.scheduled_for));
}

/** “Ghost does 3 things on its own”: the page's own count, in words. */
export function countWords(n: number): string {
  return n === 0 ? "Nothing yet" : `Ghost does ${count(n, "thing")} on its own`;
}

/** A run going now, as one line: “Ledger is running vendor inbox”. */
export function goingWords(r: ScheduleRunInfo, name: (slug: string) => string): string {
  const who = name(r.agent);
  const why = r.source.startsWith("watch:") ? `looking into something new for ${r.source.slice(6)}` : r.status === "queued" ? `about to run ${r.name}` : `running ${r.name}`;
  return `${who} is ${why}`;
}

/* ------------------------------------------------------------------ what can't run, in its row */

/** A problem the daemon reports for something that isn't on the list (it doesn't read), as a row of its own. */
export interface ProblemRow {
  key: string;
  group: Group;
  problem: Problem;
}

/**
 * Each problem goes to its row (the flow review, 2026-11-14: a red box above the list said three
 * things and disagreed with the rows): a row it names gets its chip and its sentence; one that
 * names nothing on the list becomes a row of its own in its group.
 */
export function placeProblems(doings: readonly Doing[], problems: readonly Problem[]): { byKey: Map<string, Problem>; rows: ProblemRow[] } {
  const byKey = new Map<string, Problem>();
  const rows: ProblemRow[] = [];
  problems.forEach((p, i) => {
    const subject = p.subject;
    const hit = subject
      ? doings.find((d) => (d.kind === "schedule" ? subject.kind === "schedule" && d.s.name === subject.name : d.kind === subject.kind && (d.kind === "watch" ? d.w.name : d.h.name) === subject.name))
      : undefined;
    if (hit && !byKey.has(hit.key)) byKey.set(hit.key, p);
    else rows.push({ key: `problem:${i}:${subject ? `${subject.kind}:${subject.name}` : "file"}`, group: subject?.kind === "schedule" ? "time" : "event", problem: p });
  });
  return { byKey, rows };
}
