/**
 * The mock daemon's reach (ADR 0060, `ghost_core::reach`): one row per call an agent made,
 * counts and sizes only, made up from the day's runs and the session, and totalled as
 * `GET /reach` totals them. A run that is going makes a call every second or two, sent as a
 * `reach` event, so Today's numbers move while agents work.
 */
import type { ReachCall, ReachTotalView, ReachView, ScheduleRunInfo } from "@ghost/client-ts";

import { seeded } from "./scenarios";

/** Tools an agent might call on each app, with how much one call tends to bring back. */
const TOOLS: Record<string, [tool: string, items: number, bytesPerItem: number][]> = {
  github: [
    ["search_issues", 30, 2400],
    ["issue_read", 1, 5200],
    ["list_pull_requests", 18, 3100],
    ["get_comments", 12, 900],
    ["list_workflow_runs", 25, 1500],
  ],
  calendar: [
    ["list_events", 14, 700],
    ["get_event", 1, 1100],
  ],
  mobbin: [["search_screens", 24, 380]],
  mail: [
    ["list_unread", 40, 1800],
    ["read_message", 1, 7400],
  ],
  linear: [
    ["list_issues", 50, 1300],
    ["get_issue", 1, 3600],
  ],
  ghost: [
    ["recall", 8, 600],
    ["status", 1, 900],
    ["notify", 1, 120],
  ],
};

export function totalOf(key: string, rows: readonly ReachCall[], agent: string | null = null): ReachTotalView {
  const apps = new Set<string>();
  const t: ReachTotalView = { key, agent, calls: 0, reads: 0, changes: 0, failed: 0, items: 0, bytes: 0, ms: 0, apps: [], first: null, last: null };
  for (const r of rows) {
    t.calls++;
    if (r.read) t.reads++;
    else t.changes++;
    if (!r.ok) t.failed++;
    t.items += r.items ?? 0;
    t.bytes += r.bytes;
    t.ms += r.ms;
    apps.add(r.app);
    if (!t.first || r.at < t.first) t.first = r.at;
    if (!t.last || r.at > t.last) t.last = r.at;
  }
  t.apps = [...apps].sort();
  return t;
}

function grouped(rows: readonly ReachCall[], by: (r: ReachCall) => string, withAgent: boolean): ReachTotalView[] {
  const m = new Map<string, ReachCall[]>();
  for (const r of rows) m.set(by(r), [...(m.get(by(r)) ?? []), r]);
  return [...m.entries()].map(([k, list]) => totalOf(k, list, withAgent ? list[0].agent : null)).sort((a, b) => b.calls - a.calls || a.key.localeCompare(b.key));
}

/** What `GET /reach` answers over some rows. */
export function summarize(rows: readonly ReachCall[], since: string, until: string): ReachView {
  const bySession = grouped(rows, (r) => r.session_id, true).sort((a, b) => (b.last ?? "").localeCompare(a.last ?? ""));
  return { since, until, total: totalOf("all", rows), by_app: grouped(rows, (r) => r.app, false), by_agent: grouped(rows, (r) => r.agent, false), by_session: bySession };
}

export class MockReach {
  rows: ReachCall[] = [];
  private rand = seeded(4471);

  constructor(private apps: string[]) {
    if (!this.apps.length) this.apps = ["ghost"];
  }

  /** One call by `agent` in `session` at `at`. */
  call(session: string, agent: string, at: Date, read = true): ReachCall {
    const app = this.apps[Math.floor(this.rand() * this.apps.length)] ?? "ghost";
    const tools = TOOLS[app] ?? [["list", 10, 800] as [string, number, number]];
    const [tool, items, per] = tools[Math.floor(this.rand() * tools.length)];
    const n = Math.max(1, Math.round(items * (0.5 + this.rand())));
    const ok = this.rand() > 0.04;
    const row: ReachCall = {
      at: at.toISOString(),
      session_id: session,
      agent,
      app,
      tool: read ? tool : "issue_write",
      read,
      ok,
      items: ok ? n : null,
      bytes: ok ? Math.round(n * per * (0.7 + this.rand() * 0.6)) : 0,
      ms: Math.round(120 + this.rand() * 1400),
    };
    this.rows.push(row);
    return row;
  }

  /** A day of calls from the runs it had, and a session's. */
  seed(runs: readonly ScheduleRunInfo[], session: { id: string | null; agent: string } | null) {
    for (const r of runs) {
      const start = Date.parse(r.started_at ?? r.scheduled_for);
      const end = r.finished_at ? Date.parse(r.finished_at) : Date.now();
      const n = r.status === "running" ? Math.max(3, r.tool_calls) : r.tool_calls;
      for (let i = 0; i < n; i++) this.call(`run-${r.id}`, r.agent, new Date(start + ((end - start) * (i + 0.5)) / Math.max(1, n)), i % 7 !== 6);
    }
    if (session?.id) for (let i = 0; i < 4; i++) this.call(session.id, session.agent, new Date(Date.now() - (6 - i) * 60_000));
    this.rows.sort((a, b) => a.at.localeCompare(b.at));
  }

  view(since: string, until: string | null, sessions: string[]): ReachView {
    const end = until ?? new Date(Date.now() + 1000).toISOString();
    const rows = this.rows.filter((r) => r.at >= since && r.at < end && (!sessions.length || sessions.includes(r.session_id)));
    return summarize(rows, since, end);
  }
}
