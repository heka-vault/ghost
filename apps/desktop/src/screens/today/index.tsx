/**
 * Owner: today. Today, `#/`: the briefing, the desk's home (docs/design/overhaul-2026-11.md,
 * the combined direction). One sentence with the day's real counts; the instruments (what is
 * working now, how much Ghost did, what it can reach, how fast it answers); then the timeline,
 * the spine of the app: Now at its head with what needs the operator (Decide, Answer, Know) and
 * the agents at work, then the day going back hour by hour. `[` and `]` move a day, `t` comes
 * back to today; the talk bar is the way to Ghost without leaving the page.
 *
 * What needs you (ADR 0029) lives here now: every action it had (Confirm and Cancel, an answer
 * or Decline, Got it, Later, Open) is on its item.
 */
import { useGhost } from "@ghost/ui";
import { ChevronLeft, ChevronRight, Settings2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { hrefFor } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { loadDay, nameOf, useToday } from "../../app/today";
import { Button, IconButton, Notice, Page, Skeleton, cx, errorText, useLive, useSetupPromptHere } from "../../ui";
import { type DayCounts, type Moment, cluster, countsOf, dayLabel, headline, isSameDay, momentsOf, shiftDay } from "./model";
import { Now } from "./Needs";
import { TalkBar } from "./TalkBar";
import { Timeline } from "./Timeline";
import "./today.css";

/** How far back the timeline goes: the audit and the runs keep about this much worth showing. */
const MAX_BACK_DAYS = 30;

export default function TodayScreen(_: RouteProps) {
  const today = useToday();
  const { client } = useGhost();
  const { online, setupNeeded, setupLocked, setupHref, loadingModels } = useLive();
  const [day, setDay] = useState(() => new Date());
  const isToday = isSameDay(day, new Date());
  const past = usePastDay(isToday ? null : day, client);
  const moments: Moment[] = isToday ? today.moments : past.moments;
  const counts: DayCounts = isToday ? today.counts : countsOf({ decide: [], answer: [], know: [] }, past.moments);
  const h = headline(counts, isToday);
  const needsSetup = online && !loadingModels && setupNeeded;
  useSetupPromptHere(needsSetup && isToday);

  const go = (n: number) => {
    const next = shiftDay(day, n);
    if (next.getTime() > Date.now()) return setDay(new Date());
    if (Date.now() - next.getTime() > MAX_BACK_DAYS * 86_400_000) return;
    setDay(next);
  };
  // `[` and `]` a day back and forward, `t` today, wherever focus isn't in a field.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.closest("input, textarea, select, [contenteditable=true]") || t.closest("dialog"))) return;
      if (e.key === "[") go(-1);
      else if (e.key === "]") go(1);
      else if (e.key === "t") setDay(new Date());
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const dateLine = day.toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" });
  const working = today.working.length;

  return (
    <Page title="Today" bare width="wide" className="today" dock={isToday ? <TalkBar /> : undefined}>
      <header className="today-head">
        <div className="today-day">
          <IconButton size="sm" variant="ghost" label={`The day before (${"["})`} icon={<ChevronLeft />} onClick={() => go(-1)} />
          <p className="today-day__label" aria-live="polite">
            <span className="today-day__name">{dayLabel(day)}</span>
            {dayLabel(day) !== dateLine && <span className="today-day__date">{dateLine}</span>}
          </p>
          <IconButton size="sm" variant="ghost" label={`The day after (${"]"})`} icon={<ChevronRight />} onClick={() => go(1)} disabled={isToday} />
          {!isToday && (
            <Button size="sm" variant="ghost" onClick={() => setDay(new Date())}>
              Back to today
            </Button>
          )}
        </div>

        {needsSetup && isToday ? (
          <div className="today-setup">
            <p className="today-headline">
              <span className="today-headline__lead">{setupLocked ? "Your keyring is locked." : "Ghost needs a model before it can answer."}</span>{" "}
              <span className="today-headline__rest">{setupLocked ? "It can’t read its passwords until it’s unlocked." : "Add the Anthropic password or a model endpoint, and the rest of this page fills in as it works."}</span>
            </p>
            <Button variant="primary" leadingIcon={<Settings2 />} href={setupHref}>
              {setupLocked ? "Unlock…" : "Finish setup"}
            </Button>
          </div>
        ) : (
          <p className="today-headline">
            <span className="today-headline__lead">{h.lead}</span> {h.rest && <span className="today-headline__rest">{h.rest}</span>}
          </p>
        )}

        {/* The instruments appear once there is something true to say: a run, a change or an app. */}
        {isToday && (working > 0 || counts.runs + counts.changes > 0 || today.reach.apps > 0) && (
          <ul className="today-gauges" role="list" aria-label="Ghost right now">
            <Gauge label="Working now" value={working ? `${working} ${working === 1 ? "agent" : "agents"}` : "Nobody"} live={working > 0} hint={working ? today.working.map((r) => nameOf(today.agents, r.agent)).join(", ") : "No run is going"} />
            <Gauge label="Today" value={`${counts.toolCalls} tool ${counts.toolCalls === 1 ? "call" : "calls"}`} hint={`${counts.modelCalls} model ${counts.modelCalls === 1 ? "call" : "calls"} in ${counts.runs} ${counts.runs === 1 ? "run" : "runs"}`} />
            <Gauge
              label="In reach"
              value={`${today.reach.apps} ${today.reach.apps === 1 ? "app" : "apps"}`}
              hint={`${today.reach.tools} tools`}
              trouble={today.reach.down ? `${today.reach.down} not connected` : undefined}
              href="/settings/apps"
            />
            <Gauge
              label="Replies start in"
              value={today.speed ? today.speed.text : "Not checked"}
              hint={today.speed ? today.speed.model : "Test it in Settings › Model"}
              href="/settings/model"
            />
          </ul>
        )}
      </header>

      {today.error && isToday && (
        <Notice tone="danger" title="Couldn’t load today" className="today-notice" action={<Button size="sm" onClick={today.reload}>Try again</Button>}>
          {today.error}
        </Notice>
      )}
      {past.error && !isToday && (
        <Notice tone="danger" title="Couldn’t load that day" className="today-notice">
          {past.error}
        </Notice>
      )}
      {past.cut && !isToday && (
        <Notice tone="info" title="A long day" className="today-notice">
          This shows its newest 1,000 changes; the audit log has the rest.
        </Notice>
      )}

      {(isToday && !today.loaded) || (!isToday && past.loading) ? (
        <TimelineSkeleton />
      ) : (
        <Timeline key={day.toDateString()} moments={moments} agents={today.agents} isToday={isToday} head={isToday ? <Now /> : null} />
      )}
    </Page>
  );
}

/** One instrument: a true number about Ghost right now, linked to where it is set when it can be. */
function Gauge({ label, value, hint, live, trouble, href }: { label: string; value: string; hint: string; live?: boolean; trouble?: string; href?: string }) {
  const body = (
    <>
      <span className="today-gauge__label">
        {live && <span className="today-gauge__live" aria-hidden />}
        {label}
      </span>
      <span className="today-gauge__value t-num">{value}</span>
      <span className="today-gauge__hint">
        {hint}
        {trouble && <span className="today-gauge__trouble"> · {trouble}</span>}
      </span>
    </>
  );
  return (
    <li>
      {href ? (
        <a className="today-gauge is-link" href={hrefFor(href)} aria-label={`${label}: ${value}, ${hint}${trouble ? `, ${trouble}` : ""}`}>
          {body}
        </a>
      ) : (
        <div className="today-gauge" aria-label={`${label}: ${value}, ${hint}`} role="group">
          {body}
        </div>
      )}
    </li>
  );
}

function usePastDay(day: Date | null, client: ReturnType<typeof useGhost>["client"]) {
  const [state, setState] = useState<{ moments: Moment[]; loading: boolean; error: string | null; cut: boolean }>({ moments: [], loading: false, error: null, cut: false });
  const key = day?.toDateString() ?? null;
  useEffect(() => {
    if (!day) return;
    let live = true;
    setState((s) => ({ ...s, loading: true, error: null }));
    loadDay(client, day)
      .then((r) => live && setState({ moments: cluster(momentsOf(r, day)), loading: false, error: null, cut: r.auditCut }))
      .catch((e) => live && setState({ moments: [], loading: false, error: errorText(e), cut: false }));
    return () => {
      live = false;
    };
    // The day's date is the key; a new Date object for the same day loads nothing new.
  }, [key, client]);
  return useMemo(() => state, [state]);
}

function TimelineSkeleton() {
  return (
    <div className="today-skel" aria-busy="true" aria-label="Loading today">
      {[0, 1, 2, 3, 4].map((i) => (
        <div key={i} className="today-skel__row">
          <Skeleton width={48} height={12} />
          <Skeleton width={10} height={10} radius={999} />
          <Skeleton width={`${60 - i * 6}%`} height={14} />
        </div>
      ))}
    </div>
  );
}

