/**
 * Where Ghost is looking (the floating room's working state, ADR 0059 as amended): every app
 * and agent a turn reaches for is a named place with a beam, lit while its call runs, and a
 * quiet trace after, so the operator sees where it looked this turn.
 *
 * Plain TypeScript, true to the daemon's activity (ADR 0055: names and ids, never words). A
 * place's name is the app's, or for Ghost's own app (ADR 0045) the part of Ghost the tool
 * reads (Memory, Docs, Agents...); its word is at most two words from the tool's own name
 * (`github_issue_list` → GitHub, "issues"). Nothing here reads arguments or results: the size
 * of what came back is a number the page measures (`GET /calls/{id}`, its length only).
 */
import { appOf, type Activity } from "@ghost/ui";

/** How many places the field draws (`field.ts`'s `uNodes`). */
export const MAX_PLACES = 6;
/** A call quicker than this is still lit this long, so a 90 ms lookup is seen (about 22 frames). */
export const MIN_LIT_MS = 750;
/** The light coming back from a place with its result. */
export const RETURN_MS = 650;

export interface Place {
  /** One place per app or per part of Ghost: `github`, `ghost:memory`. */
  key: string;
  /** Short and spoken: "GitHub", "Memory". */
  name: string;
  /** At most two words for what was asked of it last ("issues", "today"), or null. */
  word: string | null;
  calls: number;
  /** Calls still running. */
  open: number;
  /** When its latest call started and, once every call is back, when the last came back (ms epoch). */
  startedAt: number;
  endedAt: number | null;
  /** The ids of its calls, for the page to measure what came back. */
  ids: string[];
}

/** The part of Ghost a tool of Ghost's own app reads: somewhere it is looking too. */
const GHOST_PLACES: [RegExp, string, string][] = [
  [/^(recall|transcript_read|memory_read)$/, "memory", "Memory"],
  [/^docs_/, "docs", "Docs"],
  [/^ask_agent$|^agents?_/, "agents", "Agents"],
  [/^schedules?_/, "schedules", "Schedules"],
  [/^watch(es)?_/, "watches", "Watches"],
  [/^audit_/, "audit", "Audit"],
  [/^(apps_list|app_connect|tools_search)$/, "apps", "Apps"],
  [/^(notify|resolve|waiting_ack|waiting_snooze|routes_read)$/, "waiting", "Waiting list"],
  [/^skill/, "skills", "Skills"],
  [/^outputs?_/, "outputs", "Outputs"],
];

/** Display names where the app's name isn't the word people say. */
const APP_NAMES: Record<string, string> = { github: "GitHub", gitlab: "GitLab", gmail: "Gmail", ci: "CI", posthog: "PostHog" };

/** Verbs in a tool's name: what is asked is the rest. */
const VERBS = new Set(["get", "list", "read", "search", "find", "fetch", "query", "create", "write", "update", "delete", "close", "add", "set", "manage", "run", "show", "open", "send", "post", "comment", "describe", "check"]);

export function appName(app: string): string {
  const base = app.split("-")[0];
  const named = APP_NAMES[base] ?? base.charAt(0).toUpperCase() + base.slice(1);
  return app.includes("-") ? `${named} (${app.slice(base.length + 1)})` : named;
}

/** "issues", "events today", "runs": the nouns of a tool's name, a list made plural. */
export function wordOf(tool: string): string | null {
  const parts = tool.toLowerCase().split(/[_\-.]+/).filter(Boolean);
  const listy = parts.some((p) => p === "list" || p === "search" || p === "find" || p === "query");
  const nouns = parts.filter((p) => !VERBS.has(p) && !/^\d+$/.test(p));
  if (nouns.length === 0) return null;
  const kept = nouns.slice(0, 2);
  if (listy && kept.length === 1 && !kept[0].endsWith("s")) kept[0] = `${kept[0]}s`;
  const w = kept.join(" ");
  return w.length > 18 ? kept[0].slice(0, 18) : w;
}

/** The place a tool's call reaches, from the connected apps' names. */
export function placeOf(name: string, apps: readonly string[]): { key: string; name: string; word: string | null } {
  const app = appOf(name, apps);
  if (app === "ghost") {
    const tool = name.slice("ghost_".length);
    // The part of Ghost is the word already: "Memory", "Docs", "Agents".
    for (const [re, key, label] of GHOST_PLACES) if (re.test(tool)) return { key: `ghost:${key}`, name: label, word: null };
    return { key: "ghost", name: "Ghost", word: wordOf(tool) };
  }
  if (app) return { key: app, name: appName(app), word: wordOf(name.slice(app.length + 1)) };
  // A tool no connected app claims (the app list not read yet, an in-tree source): its first
  // part is the app's name as tools are offered (`<app>_<tool>`, ADR 0034), the rest the word.
  const cut = name.indexOf("_");
  if (cut > 0) return { key: `tool:${name.slice(0, cut)}`, name: appName(name.slice(0, cut)), word: wordOf(name.slice(cut + 1)) };
  return { key: `tool:${name}`, name: appName(name), word: null };
}

/** The activity of the current turn: everything after the operator's last closed turn. */
export function thisTurn(list: readonly Activity[]): Activity[] {
  for (let i = list.length - 1; i >= 0; i--) if (list[i].kind === "you_stopped") return list.slice(i + 1);
  return [...list];
}

/**
 * The places this turn reached, in the order first reached, at most `max` (the newest kept
 * when more: the field has six beams; the rest are counted in `more`).
 */
export function reachOf(list: readonly Activity[], apps: readonly string[], max = MAX_PLACES): { places: Place[]; more: number } {
  const byKey = new Map<string, Place>();
  const callPlace = new Map<string, Place>();
  for (const a of thisTurn(list)) {
    const t = new Date(a.at).getTime();
    if (a.kind === "call" && a.id && a.name) {
      const p0 = placeOf(a.name, apps);
      let p = byKey.get(p0.key);
      if (!p) {
        p = { ...p0, calls: 0, open: 0, startedAt: t, endedAt: null, ids: [] };
        byKey.set(p0.key, p);
      }
      p.calls++;
      p.open++;
      p.word = p0.word ?? p.word;
      p.startedAt = t;
      p.endedAt = null;
      p.ids.push(a.id);
      callPlace.set(a.id, p);
    } else if (a.kind === "result" && a.id) {
      const p = callPlace.get(a.id);
      if (p && p.open > 0) {
        p.open--;
        callPlace.delete(a.id);
        if (p.open === 0) p.endedAt = t;
      }
    }
  }
  const all = [...byKey.values()];
  return { places: all.slice(-max), more: Math.max(0, all.length - max) };
}

/**
 * How lit a place's beam is at `now`, 0..1: full while a call runs and for `MIN_LIT_MS` after
 * it started, a moment's flash as its answer comes back, then the quiet trace.
 */
export const TRACE = 0.2;
export function litOf(p: Place, now: number): number {
  if (p.open > 0 || now - p.startedAt < MIN_LIT_MS) return 1;
  const since = now - Math.max(p.endedAt ?? now, p.startedAt + MIN_LIT_MS);
  if (since < RETURN_MS) return TRACE + (1 - TRACE) * (1 - since / RETURN_MS);
  return TRACE;
}

/** Where the light coming back is, 0 at the place to 1 at Ghost; -1 when none is travelling. */
export function returnOf(p: Place, now: number): number {
  if (p.open > 0 || p.endedAt == null) return -1;
  const since = now - Math.max(p.endedAt, p.startedAt + MIN_LIT_MS);
  return since >= 0 && since < RETURN_MS ? since / RETURN_MS : -1;
}

/** What came back as the light's size, 0.15..1: √chars against a full result (24,000, ADR 0046). */
export function sizeOf(chars: number | null | undefined): number {
  if (!chars || chars <= 0) return 0.15;
  return Math.min(1, 0.15 + 0.85 * Math.sqrt(chars / 24_000));
}

/** Where Ghost is looking, for the label: "GitHub", or "3 places" (the places name themselves). */
export function namesText(names: readonly string[]): string | null {
  if (names.length === 0) return null;
  if (names.length === 1) return names[0];
  return `${names.length} places`;
}
