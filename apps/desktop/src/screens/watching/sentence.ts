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

import { OUTCOME, URGENCY, count, everyWords, sumOf, untilWords, whenWords } from "./model";

/** One thing Ghost does on its own. The code keeps its names; the operator sees a sentence. */
export type Doing = { kind: "schedule"; key: string; s: ScheduleInfo } | { kind: "watch"; key: string; w: WatchInfo } | { kind: "hook"; key: string; h: HookInfo };

export interface Sentence {
  /** Ghost's own words: who, and when or on what. */
  lead: string;
  /** Someone's own words, shown apart and marked `data-user-content`; none for a watch or webhook. */
  quote: string | null;
  /** The agent the row leads with. */
  agent: string;
  /** How it tells the operator, and anything that narrows it. */
  how: string;
  /** Its state now, in a few words: “Next in 3 h · last ran 2 h ago, done”. */
  state: string;
  /** Paused, broken or off: the row says so in its badge. */
  badge: { label: string; tone: "ok" | "warn" | "danger" | "info" | "neutral" } | null;
}

export interface SentenceContext {
  /** This computer's time zone, for a schedule's “when”. */
  zone: string;
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

export function sentenceOf(d: Doing, c: SentenceContext): Sentence {
  const now = c.now ?? Date.now();
  if (d.kind === "schedule") {
    const s = d.s;
    const when = whenWords(s, c.zone);
    const whenPhrase = when.mono ? `on the schedule ${when.mono}` : lowerFirst(when.text);
    const hours = s.active_hours ? `, between ${s.active_hours.start} and ${s.active_hours.end}` : "";
    const check = s.check ? `, only when ${c.appTitle(s.check.app)} has something new` : "";
    const last = s.last_outcome ? OUTCOME[s.last_outcome] : null;
    const state = s.running
      ? "Running now"
      : !s.enabled
        ? "Paused"
        : [s.next_run ? `next ${untilWords(s.next_run, now)}` : "no next run", s.last_run ? `last ran ${relativeTime(s.last_run, now)}${last ? `, ${last.label.toLowerCase()}` : ""}` : "not run yet"].join(" · ");
    return {
      lead: `${c.agentName(s.agent)}, ${whenPhrase}${hours}:`,
      quote: quoteOf(s.instruction || s.name),
      agent: s.agent,
      how: `Tells you ${URGENCY[s.urgency]}${check}`,
      state: cap(state),
      badge: s.running ? { label: "Running", tone: "info" } : !s.enabled ? { label: "Paused", tone: "neutral" } : last && (last.tone === "danger" || last.tone === "warn") ? { label: last.label, tone: last.tone } : null,
    };
  }
  if (d.kind === "watch") {
    const w = d.w;
    const app = c.appTitle(w.app);
    const actor = w.acts_through ? c.agentName(w.acts_through) : null;
    const does = w.response === "tell" ? `Ghost tells you ${URGENCY[w.urgency]}` : "Ghost keeps a record";
    const lead = actor ? `When something new turns up in ${app}, ${actor} looks into it and ${does}` : `When something new turns up in ${app}, ${does}`;
    const state = w.paused
      ? "Paused"
      : w.error
        ? `Can’t read ${app}${w.last_ok ? ` · last worked ${relativeTime(w.last_ok, now)}` : w.last_run ? " · never worked" : ""}`
        : [`looks ${everyWords(w.every_secs)}`, w.last_run ? `looked ${relativeTime(w.last_run, now)}` : "no look yet", w.open ? `${w.open} open` : null].filter(Boolean).join(" · ");
    return {
      lead,
      quote: null,
      agent: w.acts_through ?? "ghost",
      how: w.made_by?.startsWith("agent:") ? `Set up by ${c.agentName(w.made_by.slice(6))}` : "Yours, in Ghost’s folder",
      state: cap(state),
      badge: w.paused ? { label: "Paused", tone: "neutral" } : w.error ? { label: "Can’t read", tone: "danger" } : null,
    };
  }
  const h = d.h;
  const taken = sumOf(h.accepted);
  return {
    lead: `When ${sourceOf(h)} sends ${eventsWords(h.events)}, Ghost tells you`,
    quote: null,
    agent: "ghost",
    how: h.on ? "Checked for its signature before Ghost reads it" : "Off until it is set up",
    state: cap(h.on ? (h.last ? `last ${relativeTime(h.last, now)} · ${count(taken, "delivery", "deliveries")}` : "nothing yet") : `off: ${h.off_reason ?? "the listener is off"}`),
    badge: h.on ? null : { label: "Off", tone: "neutral" },
  };
}

/** The whole sentence as one string: the row's accessible name, and what the tests read. */
export function sentenceText(s: Sentence): string {
  return s.quote ? `${s.lead} ${s.quote}` : s.lead;
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
