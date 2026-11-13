/**
 * How much Ghost read and did, as Today shows it (ADR 0060): the day's totals from
 * `GET /reach`, with the calls that arrived since as `reach` events folded in, so the numbers
 * move while agents work. Counts and sizes only; there are no contents to show.
 */
import type { ReachCall, ReachView } from "@ghost/client-ts";

/** Ghost's own app: its calls count, but it is not one of the apps Ghost reached out to. */
export const OWN_APP = "ghost";

export interface Tally {
  calls: number;
  reads: number;
  changes: number;
  failed: number;
  items: number;
  bytes: number;
  ms: number;
  /** Apps called, by name, sorted; Ghost's own left out. */
  apps: string[];
  /** The newest call's time, if any. */
  last: string | null;
}

export const EMPTY_TALLY: Tally = { calls: 0, reads: 0, changes: 0, failed: 0, items: 0, bytes: 0, ms: 0, apps: [], last: null };

type TotalLike = { calls: number; reads: number; changes: number; failed: number; items: number; bytes: number; ms: number; apps: string[]; last?: string | null };

function add(t: Tally, x: TotalLike): Tally {
  const apps = new Set([...t.apps, ...x.apps.filter((a) => a !== OWN_APP)]);
  const last = !x.last ? t.last : !t.last || x.last > t.last ? x.last : t.last;
  return { calls: t.calls + x.calls, reads: t.reads + x.reads, changes: t.changes + x.changes, failed: t.failed + x.failed, items: t.items + x.items, bytes: t.bytes + x.bytes, ms: t.ms + x.ms, apps: [...apps].sort(), last };
}

function ofCall(c: ReachCall): TotalLike {
  return { calls: 1, reads: c.read ? 1 : 0, changes: c.read ? 0 : 1, failed: c.ok ? 0 : 1, items: c.items ?? 0, bytes: c.bytes, ms: c.ms, apps: [c.app], last: c.at };
}

/** The calls that came after `base` was read: the ones it can't hold yet. */
export function newer(base: ReachView | null, live: readonly ReachCall[]): ReachCall[] {
  if (!base) return [...live];
  const until = Date.parse(base.until);
  return live.filter((c) => Date.parse(c.at) >= until);
}

/** Everything: the day's totals and what came since. */
export function tallyOf(base: ReachView | null, live: readonly ReachCall[]): Tally {
  let t = base ? add(EMPTY_TALLY, base.total) : EMPTY_TALLY;
  for (const c of newer(base, live)) t = add(t, ofCall(c));
  return t;
}

/** One session or run (`run-<id>`): its totals and what came since. */
export function tallyFor(base: ReachView | null, live: readonly ReachCall[], session: string): Tally {
  const row = base?.by_session.find((s) => s.key === session);
  let t = row ? add(EMPTY_TALLY, row) : EMPTY_TALLY;
  for (const c of newer(base, live)) if (c.session_id === session) t = add(t, ofCall(c));
  return t;
}

/** The latest calls, newest first: the day's as read, and the live ones, each once. */
export function wireOf(live: readonly ReachCall[], n = 6, recent: readonly ReachCall[] = []): ReachCall[] {
  const seen = new Set<string>();
  const key = (c: ReachCall) => `${c.at}|${c.session_id}|${c.app}|${c.tool}`;
  return [...recent, ...live]
    .filter((c) => (seen.has(key(c)) ? false : (seen.add(key(c)), true)))
    .sort((a, b) => b.at.localeCompare(a.at))
    .slice(0, n);
}

/** Calls in the last minute: how busy Ghost is right now. */
export function perMinute(live: readonly ReachCall[], now: number = Date.now()): number {
  return live.filter((c) => now - Date.parse(c.at) < 60_000).length;
}

/** A size in decimal units: “820 B”, “71.2 kB”, “412 kB”, “3.7 MB”. */
export function bytes(n: number): string {
  if (n < 1000) return `${Math.round(n)} B`;
  const units = ["kB", "MB", "GB", "TB"];
  let v = n / 1000;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1).replace(/\.0$/, "")} ${units[i]}`;
}

/** A count with thousands separators: “1,204”. */
export function count(n: number): string {
  return Math.round(n).toLocaleString("en-US");
}

/** A call's time: “340 ms”, “1.2 s”. */
export function ms(n: number): string {
  return n < 1000 ? `${Math.round(n)} ms` : `${(n / 1000).toFixed(1)} s`;
}

/** “Read 412 items across 4 apps, 3.7 MB”, or what there is of it. */
export function reachSentence(t: Tally): string {
  if (!t.calls) return "Nothing read yet";
  const apps = t.apps.length;
  const where = apps ? ` across ${apps} ${apps === 1 ? "app" : "apps"}` : "";
  return `Read ${count(t.items)} ${t.items === 1 ? "item" : "items"}${where}, ${bytes(t.bytes)}`;
}
