/**
 * Owner: today. Today, `#/`: the briefing, the desk's home (docs/design/overhaul-2026-11.md,
 * the combined direction). One sentence with the day's real counts, a problem said in it; the
 * instruments (what is working now, how much Ghost did, what it can reach, how fast it answers)
 * one remembered step away, closed for a new install (the simplification pass, 2026-11-14); then the timeline,
 * the spine of the app: Now at its head with what needs the operator (Decide, Answer, Know) and
 * the agents at work, then the day going back hour by hour. `[` and `]` move a day, `t` comes
 * back to today; the talk bar is the way to Ghost without leaving the page.
 *
 * What needs you (ADR 0029) lives here now: every action it had (Confirm and Cancel, an answer
 * or Decline, Got it, Later, Open) is on its item.
 */
import { useGhost } from "@ghost/ui";
import { AlertTriangle, ChevronLeft, ChevronRight, Settings2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { loadDay, useToday } from "../../app/today";
import { useDesign } from "../../design/context";
import { Button, IconButton, Notice, Page, Skeleton, errorText, useLive, useSetupPromptHere } from "../../ui";
import { appsDownOf, type DayCounts, type Moment, cluster, countsOf, dayLabel, headline, isSameDay, momentsOf, shiftDay, SLOW_FIRST_WORD_MS } from "./model";
import { liveNeeds } from "./live";
import { Now, WorkingNow } from "./Needs";
import { Instruments } from "./Signal";
import { TimingOverlay } from "../../app/TimingOverlay";
import { TalkBar, useLiveSession } from "./TalkBar";
import { Timeline } from "./Timeline";
import "./today.css";

/** How far back the timeline goes: the audit and the runs keep about this much worth showing. */
const MAX_BACK_DAYS = 30;

export default function TodayScreen(_: RouteProps) {
  const today = useToday();
  // A live session is seen here when Ghost doesn't float (the room view removed, 2026-11-14).
  const live = useLiveSession();
  const { client } = useGhost();
  const design = useDesign();
  const { online, setupNeeded, setupLocked, setupHref, loadingModels } = useLive();
  const [day, setDay] = useState(() => new Date());
  const isToday = isSameDay(day, new Date());
  const past = usePastDay(isToday ? null : day, client);
  const moments: Moment[] = isToday ? today.moments : past.moments;
  // Counted once: the session's own change is in the count even before the waiting list has it.
  const needs = liveNeeds(today.needs, live.pending);
  const counts: DayCounts = isToday ? { ...today.counts, decide: needs.decide.length, answer: needs.answer.length } : countsOf({ decide: [], answer: [], know: [] }, past.moments);
  const h = headline(counts, isToday, {
    failedChanges: today.counts.failedChanges,
    failedCalls: today.tally.failed,
    appsDown: appsDownOf(today.apps),
    slowModel: !!today.speed && today.speed.ms > SLOW_FIRST_WORD_MS,
  });
  const forceOpen = import.meta.env.DEV && design && new URLSearchParams(window.location.search).get("instruments") === "open" ? true : undefined;
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
    <Page title="Today" bare width="wide" className="today" dock={
        isToday ? (
          <>
            <TimingOverlay />
            <TalkBar live={live} />
          </>
        ) : undefined
      }
    >
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
        {/* What the instruments would show as a problem, in plain words, open or closed. */}
        {h.problems.length > 0 && !needsSetup && (
          <p className="today-problems" role="note">
            <AlertTriangle size={14} aria-hidden /> {h.problems.join(" · ")}
          </p>
        )}

        {/* What is working right now: never one step away (2026-11-14). */}
        {isToday && <WorkingNow />}

        {/* The instruments, one step away, once there is something true to say: a run, a change, a call or an app. */}
        {isToday && (working > 0 || counts.runs + counts.changes > 0 || today.reach.apps > 0 || today.tally.calls > 0) && <Instruments forceOpen={forceOpen} />}
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
        <Timeline key={day.toDateString()} moments={moments} agents={today.agents} isToday={isToday} head={isToday ? <Now pending={live.pending} question={live.question} /> : null} />
      )}
    </Page>
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

