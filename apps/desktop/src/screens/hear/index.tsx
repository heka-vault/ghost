/**
 * Owner: hear. The Hearing test (ADR 0042): the operator records takes of their own voice
 * reading the take list, checks each one at once, and scores how well Ghost hears them.
 *   #/hear              Record: hold Space (or the button) to record a take; its check
 *   #/hear/sessions     the sessions recorded, their takes, play, continue, delete
 *   #/hear/runs[/:id]   score a session; the runs, and a run's report against a baseline
 *   #/hear/plan         the take list, editable
 *   #/hear/wake         wake takes Ghost kept (ADR 0065): play, label, delete
 *
 * The recorder holds the mic while it is open, so Ghost isn't listening for its name then.
 * It is open while Record is the tab (and while a run it started is scoring), and closes when
 * the operator leaves, the window closes, or the screen goes away. What Ghost heard of a take
 * is shown here only: never kept, logged or put in an address.
 */
import type { HearPlanView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";

import type { RouteProps } from "../../app/routes";
import { navigate } from "../../app/router";
import { Page, StatusDot, Tabs, errorText, useLive } from "../../ui";
import { isStatus, refusal } from "./model";
import { PlanBar, PlanTab, usePlanEditor } from "./Plan";
import { RecordTab } from "./Record";
import { cancelClose, closeSoon, serial } from "./recorder";
import { RunsTab } from "./Runs";
import { SessionsTab } from "./Sessions";
import { WakeTakesTab } from "./WakeTakes";
import "./hear.css";

export type HearTab = "record" | "sessions" | "runs" | "plan" | "wake";
export const HEAR_TABS: HearTab[] = ["record", "sessions", "runs", "plan", "wake"];
const LABELS: Record<HearTab, string> = { record: "Record", sessions: "Sessions", runs: "Runs", plan: "Take list", wake: "Wake takes" };

/** The open recorder's lease, for the tabs: what it is doing and how to change it. */
export interface Lease {
  /** An open call is in flight. */
  opening: boolean;
  /** Why the last open failed, for the operator. */
  error: string | null;
  retry: () => void;
  /** Start a new session (Record's “New session”). */
  newSession: () => void;
}

export default function HearScreen({ params }: RouteProps) {
  const tab: HearTab = HEAR_TABS.includes(params.tab as HearTab) ? (params.tab as HearTab) : "record";
  const runId = tab === "runs" ? (params.id ?? null) : null;
  const { client, hear } = useGhost();
  const { online } = useLive();
  const view = useSyncExternalStore(hear.subscribe, hear.view);

  // ------------------------------------------------------------------ the take list
  const [plan, setPlan] = useState<HearPlanView | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [planAttempt, setPlanAttempt] = useState(0);
  useEffect(() => {
    let on = true;
    client
      .hearPlan()
      .then((p) => on && (setPlan(p), setPlanError(null)))
      .catch((e) => on && setPlanError(errorText(e)));
    return () => {
      on = false;
    };
  }, [client, planAttempt]);

  // The recorder's view before the first event: what it is doing now.
  useEffect(() => {
    let on = true;
    client
      .hear()
      .then((v) => on && hear.setView(v))
      .catch(() => {});
    return () => {
      on = false;
    };
  }, [client, hear]);

  // ------------------------------------------------------------------ the recorder's lease
  /** The session the operator chose (Continue, New session); `n` makes the same choice again. */
  const [chosen, setChosen] = useState<{ name: string | null; fresh?: boolean; n: number }>({
    name: null,
    n: 0,
  });
  /** The session the recorder last had open, so coming back to Record carries it on. */
  const last = useRef<string | null>(null);
  /** A run started from Runs holds the recorder until it is scored. */
  const [runHold, setRunHold] = useState(false);
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const want = (tab === "record" || runHold) && online;

  useEffect(() => {
    if (!want) return;
    // An open straight after a close (StrictMode's second mount, Continue on another session)
    // takes the recorder over instead: no close in between to drop the session.
    cancelClose();
    let on = true;
    const session = chosen.name ?? last.current;
    setOpening(true);
    setOpenError(null);
    serial(async () => {
      try {
        return await client.hearOpen(session, !session && !!chosen.fresh);
      } catch (e) {
        // A session closed with no takes is gone: start a new one instead.
        if (session && isStatus(e, 404)) return client.hearOpen(null);
        throw e;
      }
    })
      .then((v) => {
        // An open the screen has already let go of (StrictMode, a quick tab switch): its close follows.
        if (!on) return;
        hear.setView(v);
        last.current = v.session;
      })
      .catch((e) => on && setOpenError(refusal(e)))
      .finally(() => on && setOpening(false));
    return () => {
      on = false;
      closeSoon(() =>
        serial(() => client.hearClose())
          .then((v) => hear.setView(v))
          .catch(() => {}),
      );
    };
  }, [want, chosen, attempt, client, hear]);

  // Closing the window gives the mic back too.
  const wantRef = useRef(want);
  wantRef.current = want;
  useEffect(() => {
    const bye = () => {
      if (wantRef.current) void client.hearClose().catch(() => {});
    };
    window.addEventListener("pagehide", bye);
    window.addEventListener("beforeunload", bye);
    return () => {
      window.removeEventListener("pagehide", bye);
      window.removeEventListener("beforeunload", bye);
    };
  }, [client]);

  // A run Runs started holds the recorder until its scoring ends.
  const scoringNow = !!view?.scoring;
  const wasScoring = useRef(false);
  useEffect(() => {
    if (wasScoring.current && !scoringNow) setRunHold(false);
    wasScoring.current = scoringNow;
  }, [scoringNow]);

  const lease: Lease = {
    opening,
    error: openError,
    retry: () => setAttempt((n) => n + 1),
    newSession: () => {
      last.current = null;
      setChosen((c) => ({ name: null, fresh: true, n: c.n + 1 }));
    },
  };

  /** Continue a session in Record. */
  const continueSession = (name: string) => {
    last.current = name;
    setChosen((c) => ({ name, n: c.n + 1 }));
    navigate("/hear");
  };

  /** Score a session: the run needs the recorder open (it holds the mic), so open it first. */
  const score = useCallback(
    async (session: string, channel: "aec" | "raw") => {
      setRunHold(true);
      try {
        const v = await serial(() => client.hearOpen(view?.state && view.state !== "closed" ? null : session));
        hear.setView(v);
        last.current = v.session;
        const info = await client.startHearRun(session, channel);
        return info;
      } catch (e) {
        setRunHold(false);
        throw e;
      }
    },
    [client, hear, view?.state],
  );

  // ------------------------------------------------------------------ tabs
  /** The session Runs offers first: the one the operator asked to score. */
  const [runSession, setRunSession] = useState<string | null>(null);
  const toRuns = (session: string) => {
    setRunSession(session);
    navigate("/hear/runs");
  };
  // The take list's draft lives here, so moving between tabs keeps it.
  const editor = usePlanEditor(plan, setPlan, tab === "plan");
  const goTab = (next: HearTab) => navigate(next === "record" ? "/hear" : `/hear/${next}`);

  const open = !!view && view.state !== "closed" && view.state !== "opening";
  const status = open ? (
    <>
      <StatusDot phase="passive" size="sm" still />
      Recorder open
    </>
  ) : undefined;

  return (
    <Page
      width="wide"
      title="Hearing test"
      description="Read the take list aloud, one take at a time, and see what Ghost heard in each. Score a session to measure a change to how Ghost hears. Takes stay on this computer."
      status={status}
      dockFirst={tab === "plan" && editor.dirty}
      dock={tab === "plan" && editor.dirty ? <PlanBar editor={editor} /> : undefined}
    >
      <Tabs
        items={HEAR_TABS.map((t) => ({ value: t, label: t === "plan" && editor.dirty ? `${LABELS[t]} (unsaved)` : LABELS[t] }))}
        value={tab}
        onChange={goTab}
        aria-label="Hearing test"
        className="hear-tabs"
      >
        {tab === "record" && <RecordTab view={view} plan={plan?.plan ?? null} planError={planError} onRetryPlan={() => setPlanAttempt((n) => n + 1)} lease={lease} onScore={toRuns} />}
        {tab === "sessions" && <SessionsTab view={view} onContinue={continueSession} onScore={toRuns} />}
        {tab === "runs" && <RunsTab view={view} runId={runId} session={runSession} onScore={score} />}
        {tab === "wake" && <WakeTakesTab />}
        {tab === "plan" && <PlanTab editor={editor} error={planError} onRetry={() => setPlanAttempt((n) => n + 1)} />}
      </Tabs>
    </Page>
  );
}
