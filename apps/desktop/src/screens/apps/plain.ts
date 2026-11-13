/**
 * Owner: apps. An app in plain words: what it lets Ghost do, its limits, the other connections
 * to the same service, and what Ghost used it for lately. Pure and tested.
 */
import type { AppAction, AppView, ReachCall, ReachTotalView } from "@ghost/client-ts";

import { count, toolLabel } from "./model";

const lower = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);

/** “search issues, list pull requests and 6 more”. */
export function toolList(actions: readonly AppAction[], shown = 4): string {
  const names = actions.map((a) => lower(toolLabel(a.name)));
  if (names.length <= shown) return names.length <= 1 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return `${names.slice(0, shown).join(", ")} and ${names.length - shown} more`;
}

export interface Ability {
  reads: AppAction[];
  changes: AppAction[];
  blocked: number;
  asks: number;
  /** One sentence each, or null when there are none. */
  readLine: string | null;
  changeLine: string | null;
  /** “1 is blocked, 5 ask you first”, or null. */
  ruleLine: string | null;
}

export function abilityOf(actions: readonly AppAction[]): Ability {
  const reads = actions.filter((a) => a.reads);
  const changes = actions.filter((a) => !a.reads);
  const blocked = actions.filter((a) => a.level === "block").length;
  const asks = actions.filter((a) => a.level === "ask").length;
  const rules = [blocked ? `${blocked} ${blocked === 1 ? "is" : "are"} blocked` : null, asks ? `${asks} ${asks === 1 ? "asks" : "ask"} you first` : null].filter(Boolean);
  return {
    reads,
    changes,
    blocked,
    asks,
    readLine: reads.length ? `Looks up: ${toolList(reads)}.` : null,
    changeLine: changes.length ? `Changes: ${toolList(changes)}.` : null,
    ruleLine: rules.length ? `${rules.join(", ")}.`.replace(/^./, (c) => c.toUpperCase()) : null,
  };
}

const secs = (n: number) => (n % 60 === 0 && n >= 60 ? `${n / 60} min` : `${n} s`);

/** The limits Ghost holds this app to, as one or two sentences. */
export function limitsWords(l: AppView["limits"]): string {
  const call = l.timeout ?? l.default_timeout;
  const conn = l.connect_timeout ?? l.default_connect_timeout;
  const chars = l.result_chars ?? l.default_result_chars;
  const pinned = l.pinned.length ? ` ${count(l.pinned.length, "tool")} always offered by name.` : "";
  return `A call may take ${secs(call)} and connecting ${secs(conn)}. Ghost reads up to ${chars.toLocaleString("en-US")} characters of each answer; past that it’s cut and Ghost is told how much.${pinned}`;
}

/** Other connections to the same service (the same catalog entry), by app name. Ghost reads through a twin when one can't find something (ADR 0062). */
export function twinsOf(apps: readonly AppView[]): Map<string, AppView[]> {
  const by = new Map<string, AppView[]>();
  for (const a of apps) if (a.catalog) by.set(a.catalog, [...(by.get(a.catalog) ?? []), a]);
  const out = new Map<string, AppView[]>();
  for (const group of by.values()) if (group.length > 1) for (const a of group) out.set(a.name, group.filter((x) => x.name !== a.name));
  return out;
}

/** “Used 42 times this week by Ghost and Legal: 2 changes, 1.2 MB read.” */
export function latelyWords(row: ReachTotalView | undefined, agents: string[], bytes: (n: number) => string): string | null {
  if (!row || row.calls === 0) return null;
  const who = agents.length ? ` by ${agents.length === 1 ? agents[0] : `${agents.slice(0, -1).join(", ")} and ${agents[agents.length - 1]}`}` : "";
  const parts = [row.changes ? count(row.changes, "change") : null, row.bytes ? `${bytes(row.bytes)} read` : null, row.failed ? `${row.failed} failed` : null].filter(Boolean);
  return `Used ${row.calls === 1 ? "once" : row.calls === 2 ? "twice" : `${row.calls} times`} this week${who}${parts.length ? `: ${parts.join(", ")}` : ""}.`;
}

/** This app's calls, newest first. */
export const callsOf = (recent: readonly ReachCall[], app: string, n = 8) => recent.filter((c) => c.app === app).slice(0, n);
