/**
 * What the live session did, as the daemon reports it (`{"type": "activity"}`, ADR 0055): the
 * operator's turn opening and closing, a tool call starting and coming back, a handover from
 * the fast model, Ghost cut off, a failed turn. Names, ids and a duration only: never the
 * words said, a reply, a tool's arguments or result, or audio.
 *
 * Plain TypeScript, safe for React Native. The provider keeps the session's list; the screens
 * read facts from it with the functions here.
 */
export type ActivityKind = "you_started" | "you_stopped" | "call" | "result" | "handover" | "cut" | "error";

export interface Activity {
  kind: ActivityKind;
  session_id: string;
  at: string;
  name?: string | null;
  id?: string | null;
  names?: string[];
  secs?: number | null;
}

/** How many activities a session keeps: plenty for a long session's calls. */
const KEPT = 400;

/**
 * One more activity; a new session's first one drops the last session's. One already in the
 * list (the same kind, call and moment, sent again over a new connection) is not added twice.
 */
export function addActivity<T extends Activity>(list: readonly T[], a: T): T[] {
  const same = list.length === 0 || list[list.length - 1].session_id === a.session_id;
  if (!same) return [a];
  if (list.some((x) => x.kind === a.kind && x.at === a.at && (x.id ?? null) === (a.id ?? null))) return list as T[];
  return [...list, a].slice(-KEPT);
}

/** The operator is speaking now: their turn opened and hasn't closed. */
export function youSpeaking(list: readonly Activity[]): boolean {
  for (let i = list.length - 1; i >= 0; i--) {
    const k = list[i].kind;
    if (k === "you_started") return true;
    if (k === "you_stopped" || k === "cut") return false;
  }
  return false;
}

/** One tool call: when it started, and when it came back (null while it runs). */
export interface Call {
  id: string;
  name: string;
  /** The app the tool belongs to, from the connected apps' names; null when none matches. */
  app: string | null;
  /** The tool's own name without the app's prefix, in words: `issue read`. */
  tool: string;
  startedAt: string;
  endedAt: string | null;
}

/**
 * The app a tool is offered under (`github_issue_read` → `github`): the longest connected app
 * name the tool name starts with followed by `_`. Ghost's own app is `ghost`.
 */
export function appOf(name: string, apps: readonly string[]): string | null {
  let best: string | null = null;
  for (const a of [...apps, "ghost"]) {
    if (name.startsWith(`${a}_`) && (!best || a.length > best.length)) best = a;
  }
  return best;
}

/** The calls in `list`, in the order they started, each with its result's time once back. */
export function callsOf(list: readonly Activity[], apps: readonly string[]): Call[] {
  const out: Call[] = [];
  const byId = new Map<string, Call>();
  for (const a of list) {
    if (a.kind === "call" && a.id && a.name) {
      const app = appOf(a.name, apps);
      const c: Call = { id: a.id, name: a.name, app, tool: (app ? a.name.slice(app.length + 1) : a.name).replace(/_/g, " "), startedAt: a.at, endedAt: null };
      byId.set(a.id, c);
      out.push(c);
    } else if (a.kind === "result" && a.id) {
      const c = byId.get(a.id);
      if (c && !c.endedAt) c.endedAt = a.at;
    }
  }
  return out;
}

/** Seconds between two ISO times, never negative. */
export function secondsBetween(from: string, to: string): number {
  return Math.max(0, (new Date(to).getTime() - new Date(from).getTime()) / 1000);
}
