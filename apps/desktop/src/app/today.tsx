/**
 * Today's records, loaded once by the shell for everything that shows them: Today (the
 * briefing), the rail's counts and the room. A day other than today is Today's own load
 * (`loadDay`), so moving through time never disturbs the shell.
 *
 * Reloads when the daemon says the waiting list, the agents or the apps changed, when a
 * session starts or ends, when a change is proposed or settled, and every 20 seconds while
 * anything is running (runs have no event of their own yet).
 */
import type { AppView, ConversationView, GhostClient, ModelCheckView, QuestionView, ScheduleInfo, ScheduleRunInfo, WaitingItemView, AuditEntry } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";

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
  reload: () => void;
}

const EMPTY: DayRecords = { audit: [], runs: [], waiting: [], questions: [] };
const AUDIT_PAGE = 200;
/** Enough for a long day of changes; a day past it shows its newest 1,000 and says so. */
const AUDIT_MAX = 1000;

/** One day's records. The audit is read back page by page until it is older than the day. */
export async function loadDay(client: GhostClient, day: Date): Promise<DayRecords & { auditCut: boolean }> {
  const b = dayBounds(day);
  const audit: AuditEntry[] = [];
  let cut = false;
  for (let offset = 0; ; offset += AUDIT_PAGE) {
    const page = await client.audit({ limit: AUDIT_PAGE, offset });
    audit.push(...page.items);
    const oldest = page.items[page.items.length - 1];
    if (!oldest || page.items.length < AUDIT_PAGE || Date.parse(oldest.created_at) < b.start) break;
    if (offset + AUDIT_PAGE >= AUDIT_MAX) {
      cut = true;
      break;
    }
  }
  const [runs, waiting, questions] = await Promise.all([
    client.runs(200).then((r) => r.runs as ScheduleRunInfo[]).catch(() => [] as ScheduleRunInfo[]),
    client.waiting().then((w) => w.items as WaitingItemView[]),
    client.questions().then((q) => q.questions as QuestionView[]).catch(() => [] as QuestionView[]),
  ]);
  return { audit, runs, waiting, questions, auditCut: cut };
}

const TodayContext = createContext<Today | null>(null);

export function TodayProvider({ children }: { children: ReactNode }) {
  const { client, connected, status, pending, waitingVersion, conversationsVersion, appsVersion } = useGhost();
  const [records, setRecords] = useState<DayRecords>(EMPTY);
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
        setError(null);
      })
      .catch((e) => n === seq.current && setError(String(e?.message ?? e)))
      .finally(() => n === seq.current && setLoaded(true));
    client.schedules().then((s) => n === seq.current && setSchedules(s.schedules)).catch(() => {});
  }, [client, connected, waitingVersion, status?.session_id, pending?.id, tick]);

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
  // While anything runs, look again every 20 s: a run ends with no event of its own.
  useEffect(() => {
    if (!working.length) return;
    const t = window.setInterval(reload, 20_000);
    return () => window.clearInterval(t);
  }, [working.length, reload]);
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
      reload,
    };
  }, [loaded, error, records, working, agents, schedules, apps, check, reload]);

  return <TodayContext.Provider value={value}>{children}</TodayContext.Provider>;
}

/** Today's records. Outside the shell (a bare route) it is empty and never loads. */
export function useToday(): Today {
  const t = useContext(TodayContext);
  if (t) return t;
  const needs = { decide: [], answer: [], know: [] };
  return { loaded: false, error: null, records: EMPTY, needs, moments: [], counts: countsOf(needs, []), working: [], agents: [], schedules: [], apps: [], reach: { apps: 0, tools: 0, down: 0 }, speed: null, reload: () => {} };
}

/** An agent's name from its slug, with Ghost for Ghost. */
export function nameOf(agents: ConversationView[], slug: string | null | undefined): string {
  if (!slug || slug === "ghost") return "Ghost";
  return agents.find((a) => a.slug === slug)?.name ?? slug.charAt(0).toUpperCase() + slug.slice(1);
}
