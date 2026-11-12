/**
 * The waiting list as the phone shows it (issue #21, ADR 0051), and the live connection's
 * state as the app reads it. Pure functions over the stream's own views, so the mapping is
 * tested without a phone or a desk (`waiting.test.ts`).
 */
import type { QuestionView, WaitingItemView } from "@ghost/client-ts";
import { relativeTime } from "@ghost/ui";

import type { Via } from "./types";

/** What the live connection is doing. `unavailable`: no native side (the web, the mock desk). */
export type LiveState = "off" | "unpaired" | "connecting" | "connected" | "reconnecting" | "offline" | "refused" | "unavailable";

export interface Ringing {
  ringId: string;
  agent: string;
  key: string;
}

export interface LiveLink {
  state: LiveState;
  /** The desk's address the stream is open on; null when it isn't. */
  address: string | null;
  via: Via | null;
  /** When the phone last heard from the desk on the stream (ISO), or null. */
  heardAt: string | null;
  /** The operator's switch: keep the connection with the app closed. */
  enabled: boolean;
  /** Start it after the phone restarts. */
  bootStart: boolean;
  ringing: Ringing[];
  /** Keys with an action from this phone on its way. */
  sending: string[];
  /** Keys the desk refused an action for, with why. */
  errors: Record<string, string>;
}

export const NO_LINK: LiveLink = { state: "unavailable", address: null, via: null, heardAt: null, enabled: false, bootStart: false, ringing: [], sending: [], errors: {} };

export interface LinkSnapshot {
  live: LiveLink;
  items: WaitingItemView[];
  questions: QuestionView[];
}

const STATES: LiveState[] = ["off", "unpaired", "connecting", "connected", "reconnecting", "offline", "refused"];

/** The native connection's state (`GhostLink.getState()`), read defensively. */
export function parseLink(json: string | null): LinkSnapshot | null {
  if (!json) return null;
  let o: Record<string, unknown>;
  try {
    o = JSON.parse(json) as Record<string, unknown>;
  } catch {
    return null;
  }
  const arr = <T>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
  const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
  const state = STATES.includes(o.link as LiveState) ? (o.link as LiveState) : "off";
  const via = o.via === "home" ? "Wi-Fi" : o.via === "tunnel" ? "tunnel" : null;
  const heard = typeof o.heardAt === "number" ? new Date(o.heardAt).toISOString() : null;
  const errors: Record<string, string> = {};
  if (o.errors && typeof o.errors === "object") {
    for (const [k, v] of Object.entries(o.errors as Record<string, unknown>)) if (typeof v === "string") errors[k] = v;
  }
  return {
    live: {
      state,
      address: str(o.address),
      via,
      heardAt: heard,
      enabled: o.enabled !== false,
      bootStart: o.bootStart === true,
      ringing: arr<Ringing>(o.ringing).filter((r) => r && typeof r.ringId === "string"),
      sending: arr<unknown>(o.sending).filter((k): k is string => typeof k === "string"),
      errors,
    },
    items: arr<WaitingItemView>(o.items).filter((i) => i && typeof i.key === "string"),
    questions: arr<QuestionView>(o.questions).filter((q) => q && typeof q.key === "string"),
  };
}

/** Where an item came from, in plain words (the notifications say the same). */
export function sourceWords(source: string): string {
  const i = source.indexOf(":");
  const kind = i < 0 ? source : source.slice(0, i);
  const rest = i < 0 ? "" : source.slice(i + 1);
  if (kind === "watch") return `Watch ${rest}`;
  if (kind === "hook") return `Webhook ${rest}`;
  if (kind === "agent") return rest.replace(/-/g, " ").replace(/\b\w/g, (c) => c.toUpperCase()) || "An agent";
  if (kind === "client" || kind === "app") return rest || "An app";
  if (kind === "gate") return "A change";
  return "Ghost";
}

export type RowKind = "question" | "change" | "item";

/** One line of the list on Home. */
export interface WaitingRow {
  key: string;
  kind: RowKind;
  /** Who it is from, always shown: "Ledger", "Watch ci". */
  from: string;
  /** The asker's words or the item's title: shown as text, never markup. */
  text: string;
  detail: string | null;
  /** When it was asked or first seen (ISO). */
  at: string;
  choices: string[];
  /** Only the choices will do; no answer in the operator's own words. */
  choicesOnly: boolean;
  /** A change's proposal id, for Yes and No; null for a change only a spoken yes can settle. */
  changeId: string | null;
  acknowledged: boolean;
  sending: boolean;
  error: string | null;
}

/**
 * The list: every item that isn't snoozed, a question or a change as the question it is
 * (only while open), and any open question the list doesn't carry yet; oldest first.
 */
export function waitingRows(items: WaitingItemView[], questions: QuestionView[], sending: string[] = [], errors: Record<string, string> = {}): WaitingRow[] {
  const byKey = new Map(questions.map((q) => [q.key, q]));
  const rows: WaitingRow[] = [];
  const seen = new Set<string>();
  const base = (key: string) => ({ sending: sending.includes(key), error: errors[key] ?? null });
  const fromQuestion = (q: QuestionView, acknowledged: boolean): WaitingRow => ({
    key: q.key,
    kind: q.kind === "change" ? "change" : "question",
    from: q.asked_by,
    text: q.question,
    detail: q.detail ?? null,
    at: q.asked_at,
    choices: q.choices,
    choicesOnly: q.kind === "change" ? true : q.choices_only,
    changeId: q.kind === "change" ? (q.change_id ?? null) : null,
    acknowledged,
    ...base(q.key),
  });
  for (const item of items) {
    seen.add(item.key);
    if (item.state === "snoozed") continue;
    const q = byKey.get(item.key) ?? item.question ?? null;
    if (q) {
      if (q.state === "open") rows.push(fromQuestion(q, item.state === "acknowledged"));
      continue;
    }
    const gate = item.source === "gate";
    rows.push({
      key: item.key,
      kind: gate ? "change" : "item",
      from: sourceWords(item.source),
      text: item.title,
      detail: null,
      at: item.first_seen,
      choices: [],
      choicesOnly: gate,
      changeId: gate ? (item.change?.id ?? null) : null,
      acknowledged: item.state === "acknowledged",
      ...base(item.key),
    });
  }
  for (const q of questions) if (!seen.has(q.key) && q.state === "open") rows.push(fromQuestion(q, false));
  return rows.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
}

/**
 * The line under the list when it may be out of date: null while the stream is live, else
 * when the phone last heard and why it can't now.
 */
export function staleLine(live: LiveLink, now = Date.now()): string | null {
  if (live.state === "connected" || live.state === "unavailable") return null;
  const rel = relativeTime(live.heardAt, now);
  const as = !rel ? "" : rel === "now" ? "As of a moment ago. " : `As of ${rel}. `;
  switch (live.state) {
    case "connecting":
    case "reconnecting":
      return `${as}Reconnecting to your desk…`;
    case "refused":
      return `${as}Your desk doesn’t recognise this phone any more. Pair it again in Settings.`;
    case "off":
      return `${as}The connection is off, so this list is only as new as the last time the app was open.`;
    default:
      return `${as}Can’t reach your desk right now; the phone keeps trying.`;
  }
}

/** Settings' one line on the connection: its state and the address in use. */
export function liveLine(live: LiveLink): string {
  const where = live.via === "Wi-Fi" ? "over your home network" : live.via === "tunnel" ? "through your tunnel" : "";
  switch (live.state) {
    case "connected":
      return ["Connected", where, live.address ? `at ${live.address}` : ""].filter(Boolean).join(" ");
    case "connecting":
      return "Connecting…";
    case "reconnecting":
      return "Reconnecting…";
    case "offline":
      return "Can’t reach your desk. Trying again.";
    case "refused":
      return "Your desk refused this phone. Pair it again, or check the phone’s clock.";
    case "unpaired":
      return "Not paired";
    case "off":
      return "Off while the app is closed";
    default:
      return "Not available in this build";
  }
}

/** A ring the app was opened to answer: `ghost://answer?ring=…&agent=…&item=…`. */
export interface RingAnswer {
  ringId: string;
  agent: string;
  item: string | null;
}

export function parseRingAnswer(params: Record<string, string | string[] | undefined>): RingAnswer | null {
  const one = (v: string | string[] | undefined) => (Array.isArray(v) ? v[0] : v) ?? "";
  const ringId = one(params.ring);
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(ringId)) return null;
  const agent = one(params.agent);
  const item = one(params.item);
  return {
    ringId,
    agent: /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(agent) ? agent : "ghost",
    item: item && item.length <= 300 ? item : null,
  };
}
