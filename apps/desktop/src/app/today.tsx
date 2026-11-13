/**
 * Today's records, loaded once by the shell for everything that shows them: Today (the
 * briefing), the rail's counts and the room. A day other than today is Today's own load
 * (`loadDay`), so moving through time never disturbs the shell.
 *
 * Reloads when the daemon says the waiting list, the agents or the apps changed, when a
 * session starts or ends, when a change is proposed or settled, and when a run starts or ends
 * (ADR 0060's `run` event). Each call an agent makes arrives as a `reach` event and is folded
 * into the day's totals as it comes, so the numbers move while agents work.
 */
import type { AppView, AuditEntry, ConversationView, GhostClient, ModelCheckView, QuestionView, ReachCall, ReachView, ScheduleInfo, ScheduleRunInfo, WaitingItemView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";

import { EMPTY_TALLY, type Tally, tallyOf } from "../screens/today/telemetry";
import { type DayRecords, type Moment, type Needs, cluster, countsOf, dayBounds, type DayCounts, momentsOf, needsOf, reachOf, type Reach, replySpeed } from "../screens/today/model";

export interface Today {
  /** False until the first load has answered (or failed). */
  loaded: boolean;
  error: string | null;
  records: DayRecords;
  needs: Needs;
  /** The day's moments, newest first, clustered. */
  moments: Moment[];
  counts: DayCounts;
  /** Runs going or about to start, oldest first: agents at work right now. */
  working: ScheduleRunInfo[];
  agents: ConversationView[];
  schedules: ScheduleInfo[];
  apps: AppView[];
  reach: Reach;
  speed: ReturnType<typeof replySpeed>;
  /** How much Ghost read and did today, as `GET /reach` totalled it when the day was read (ADR 0060). */
  reachDay: ReachView | null;
  /** The calls since this window opened, oldest first (the `reach` events). */
  calls: readonly ReachCall[];
  /** Everything today: the day's totals with the calls since folded in. */
  tally: Tally;
  reload: () => void;
}

const EMPTY: DayRecords = { audit: [], runs: [], waiting: [], questions: [] };
const AUDIT_PAGE = 500;
/** Enough for a long day of changes; a day past it shows its newest 2,000 and says so. */
const AUDIT_MAX = 2000;

/**
 * One day's records, read by the day's range (ADR 0060): the audit and the runs of that day
 * only, and how much Ghost read in it. A daemon from before ADR 0060 ignores the range; the
 * timeline filters by time anyway, and reach is then null.
 */
export async function loadDay(client: GhostClient, day: Date): Promise<DayRecords & { auditCut: boolean; reach: ReachView | null }> {
  const b = dayBounds(day);
  const since = new Date(b.start).toISOString();
  const until = new Date(b.end).toISOString();
  const audit: AuditEntry[] = [];
  let cut = false;
  for (let offset = 0; ; offset += AUDIT_PAGE) {
    const page = await client.audit({ limit: AUDIT_PAGE, offset, since, until });
    audit.push(...page.items);
    if (page.items.length < AUDIT_PAGE || offset + AUDIT_PAGE >= page.total) break;
    if (offset + AUDIT_PAGE >= AUDIT_MAX) {
      cut = true;
      break;
    }
  }
  const [runs, waiting, questions, reach] = await Promise.all([
    client.runs(200, { since, until }).then((r) => r.runs as ScheduleRunInfo[]).catch(() => [] as ScheduleRunInfo[]),
    client.waiting().then((w) => w.items as WaitingItemView[]),
    client.questions().then((q) => q.questions as QuestionView[]).catch(() => [] as QuestionView[]),
    client.reach(since, until < new Date().toISOString() ? until : undefined).catch(() => null),
  ]);
  return { audit, runs, waiting, questions, auditCut: cut, reach };
}

const TodayContext = createContext<Today | null>(null);

export function TodayProvider({ children }: { children: ReactNode }) {
  const { client, connected, status, pending, waitingVersion, conversationsVersion, appsVersion, runsVersion, reach: reachLive } = useGhost();
  const [records, setRecords] = useState<DayRecords>(EMPTY);
  const [reachDay, setReachDay] = useState<ReachView | null>(null);
  const [calls, setCalls] = useState<readonly ReachCall[]>(() => reachLive.recent());
  const [agents, setAgents] = useState<ConversationView[]>([]);
  const [schedules, setSchedules] = useState<ScheduleInfo[]>([]);
  const [apps, setApps] = useState<AppView[]>([]);
  const [check, setCheck] = useState<ModelCheckView | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  const seq = useRef(0);

  useEffect(() => {
    if (!connected) return;
    const n = ++seq.current;
    loadDay(client, new Date())
      .then((r) => {
        if (n !== seq.current) return;
        setRecords(r);
        setReachDay(r.reach);
        setError(null);
      })
      .catch((e) => n === seq.current && setError(String(e?.message ?? e)))
      .finally(() => n === seq.current && setLoaded(true));
    client.schedules().then((s) => n === seq.current && setSchedules(s.schedules)).catch(() => {});
  }, [client, connected, waitingVersion, status?.session_id, pending?.id, runsVersion, tick]);

  // Each call as it comes (ADR 0060), drawn at most four times a second.
  useEffect(() => {
    let timer = 0;
    const off = reachLive.subscribe(() => {
      if (timer) return;
      timer = window.setTimeout(() => {
        timer = 0;
        setCalls(reachLive.recent());
      }, 250);
    });
    return () => {
      off();
      if (timer) window.clearTimeout(timer);
    };
  }, [reachLive]);

  useEffect(() => {
    if (!connected) return;
    client.conversations().then(setAgents).catch(() => {});
  }, [client, connected, conversationsVersion]);

  useEffect(() => {
    if (!connected) return;
    client.apps().then(setApps).catch(() => {});
    client.modelCheckLatest().then((c) => setCheck(c.check ?? null)).catch(() => {});
  }, [client, connected, appsVersion]);

  const working = useMemo(
    () => records.runs.filter((r) => r.status === "running" || r.status === "queued").sort((a, b) => (a.started_at ?? a.scheduled_for).localeCompare(b.started_at ?? b.scheduled_for)),
    [records.runs],
  );
  // A run starting or ending is an event of its own now (ADR 0060): no polling. A daemon from
  // before it sends none, so while anything runs look again once a minute as a backstop.
  useEffect(() => {
    if (!working.length || reachDay) return;
    const t = window.setInterval(reload, 60_000);
    return () => window.clearInterval(t);
  }, [working.length, reload, reachDay]);
  // And at midnight, today becomes a new day.
  useEffect(() => {
    const b = dayBounds(new Date());
    const t = window.setTimeout(reload, b.end - Date.now() + 1000);
    return () => window.clearTimeout(t);
  }, [reload, tick]);

  const value = useMemo<Today>(() => {
    const now = new Date();
    const needs = needsOf(records.waiting);
    const moments = cluster(momentsOf(records, now));
    return {
      loaded,
      error,
      records,
      needs,
      moments,
      counts: countsOf(needs, moments),
      working,
      agents,
      schedules,
      apps,
      reach: reachOf(apps),
      speed: replySpeed(check),
      reachDay,
      calls,
      tally: tallyOf(reachDay, calls),
      reload,
    };
  }, [loaded, error, records, working, agents, schedules, apps, check, reload, reachDay, calls]);

  return <TodayContext.Provider value={value}>{children}</TodayContext.Provider>;
}

/** Today's records. Outside the shell (a bare route) it is empty and never loads. */
export function useToday(): Today {
  const t = useContext(TodayContext);
  if (t) return t;
  const needs = { decide: [], answer: [], know: [] };
  return { loaded: false, error: null, records: EMPTY, needs, moments: [], counts: countsOf(needs, []), working: [], agents: [], schedules: [], apps: [], reach: { apps: 0, tools: 0, down: 0 }, speed: null, reachDay: null, calls: [], tally: EMPTY_TALLY, reload: () => {} };
}

/** An agent's name from its slug, with Ghost for Ghost. */
export function nameOf(agents: ConversationView[], slug: string | null | undefined): string {
  if (!slug || slug === "ghost") return "Ghost";
  return agents.find((a) => a.slug === slug)?.name ?? slug.charAt(0).toUpperCase() + slug.slice(1);
}
