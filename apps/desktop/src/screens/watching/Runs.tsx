/**
 * Owner: watching. A run read as what it did: who ran it and why, how it ended, how long it took,
 * what it reached (ADR 0060: the apps, items and bytes of its `run-<id>` session, never contents)
 * and what it said. Used by a schedule's history and by Recent runs on the page.
 */
import type { ReachTotalView, ScheduleRunInfo } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ChevronDown } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { Badge, cx, LineGlyph, agentName, relativeTime } from "../../ui";
import { DELIVERY, OUTCOME, count, formatBytes, runErrorWords } from "./model";
import { tookWords } from "./when";

/** Reach per run, keyed by run id, read once for a list of runs. */
export function useRunReach(runs: readonly ScheduleRunInfo[] | null): Map<string, ReachTotalView> {
  const { client } = useGhost();
  const [map, setMap] = useState<Map<string, ReachTotalView>>(new Map());
  const key = runs?.map((r) => `${r.id}:${r.status}`).join(",") ?? "";
  useEffect(() => {
    if (!runs?.length) return;
    let on = true;
    const since = runs.reduce((min, r) => Math.min(min, Date.parse(r.started_at ?? r.scheduled_for)), Date.now()) - 60_000;
    client
      .reach(new Date(since), new Date(), runs.map((r) => `run-${r.id}`))
      .then((v) => on && setMap(new Map(v.by_session.map((s) => [s.key.replace(/^run-/, ""), s]))))
      .catch(() => {});
    return () => {
      on = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, key]);
  return map;
}

/** “Read 12 items, 34 kB from GitHub and Calendar; made 1 change”, from a run's reach. */
export function reachWords(r: ReachTotalView | undefined, appTitle: (name: string) => string): string | null {
  if (!r || r.calls === 0) return null;
  const apps = r.apps.map(appTitle);
  const where = apps.length === 0 ? "" : apps.length === 1 ? ` from ${apps[0]}` : ` from ${apps.slice(0, -1).join(", ")} and ${apps[apps.length - 1]}`;
  const read = r.reads ? `Read ${r.items ? count(r.items, "item") : count(r.reads, "time")}${r.bytes ? `, ${formatBytes(r.bytes)}` : ""}${where}` : null;
  const changed = r.changes ? `made ${count(r.changes, "change")}` : null;
  const failed = r.failed ? `${r.failed} failed` : null;
  const parts = [read, changed, failed].filter(Boolean) as string[];
  if (!parts.length) return null;
  const s = parts.join("; ");
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function RunCard({ r, reach, showAgent, hideName, appTitle, open: openInit = false }: { r: ScheduleRunInfo; reach?: ReachTotalView; showAgent?: boolean; hideName?: boolean; appTitle: (n: string) => string; open?: boolean }) {
  const [open, setOpen] = useState(openInit);
  const quiet = r.delivery === "silent" || r.delivery === "empty";
  const took = tookWords(r.started_at, r.finished_at);
  const reached = reachWords(reach, appTitle);
  const why = r.source.startsWith("watch:") ? `for the watch ${r.source.slice(6)}` : r.source === "schedule" || r.schedule ? null : r.source.startsWith("client:") ? `asked by ${r.source.slice(7)}` : null;
  const meta = [r.status === "running" ? "running now" : relativeTime(r.finished_at ?? r.started_at ?? r.scheduled_for), took, r.status !== "unchanged" ? count(r.tool_calls, "tool call") : "no model needed", why].filter(Boolean).join(" · ");
  const said = r.error ? runErrorWords(r.error) : r.answer && !quiet ? r.answer : quiet ? "Nothing to report." : null;
  const long = !!said && said.length > 180;
  return (
    <article className={cx("wat-runcard", `is-${r.status}`)} aria-label={`${agentName(r.agent)}, ${r.name}, ${OUTCOME[r.status].label}`}>
      <header className="wat-runcard__head">
        {showAgent && <LineGlyph slug={r.agent} name={agentName(r.agent)} size="md" />}
        <span className="wat-runcard__title">
          {showAgent && <span className="wat-runcard__agent">{agentName(r.agent)}</span>} {hideName ? null : r.name}
        </span>
        <Badge tone={OUTCOME[r.status].tone} dot size="sm">
          {OUTCOME[r.status].label}
        </Badge>
      </header>
      <p className="wat-runcard__meta t-num">{meta}</p>
      {reached && <p className="wat-runcard__reach">{reached}</p>}
      {said && (
        <div className={cx("wat-runcard__said", r.error && "is-error", long && !open && "is-clamped")} data-user-content title={r.error ?? undefined}>
          {said}
        </div>
      )}
      {long && (
        <button type="button" className="wat-runcard__more" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          {open ? "Less" : "All of it"} <ChevronDown size={14} aria-hidden className={cx(open && "is-up")} />
        </button>
      )}
      {r.delivery && DELIVERY[r.delivery] && !quiet && <p className="wat-runcard__delivery">{DELIVERY[r.delivery]}</p>}
    </article>
  );
}

/** Apps' titles by name, for words: “GitHub”, not github_heka. */
export function useAppTitles(): (name: string) => string {
  const { client, status } = useGhost();
  const [titles, setTitles] = useState<Map<string, string>>(new Map());
  useEffect(() => {
    client
      .apps()
      .then((a) => setTitles(new Map(a.map((x) => [x.name, x.title]))))
      .catch(() => {});
  }, [client, status?.apps.length]);
  return useMemo(() => (n: string) => (n === "ghost" ? "Ghost" : titles.get(n) ?? n), [titles]);
}
