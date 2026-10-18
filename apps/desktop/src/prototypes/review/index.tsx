/**
 * Owner: review. PROTOTYPE: the issue review queue (PRD sections 4 and 6; FR-007, FR-017,
 * FR-021, FR-022). Issues plausibly waiting on the operator's sign-off, ranked, each with
 * why it's here; the selected issue's decision, facts, dependencies, other people's comments
 * and decisions; what Ghost is on right now or where the last session stopped; a weekly
 * summary. Voice first: every action shows the phrase to say.
 *
 * Data: useFixture<ReviewFixture>("review") (no API yet). Live pieces are real: the waiting
 * change (PendingChange, client.confirm/cancel), the session state, and asking Ghost about
 * an issue (client.say in a session, client.wake otherwise).
 *
 * States by URL, before the hash: ?mock=<scenario>&view=week&issue=infra-77&filter=billing
 * &state=loading|error #/review. See the review owner's list in the design handoff.
 */
import { useGhost } from "@ghost/ui";
import { Check, Copy, History, Inbox } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { inTauri, startDaemon } from "../../transport";

import { navigate } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { useFixture } from "../../design/context";
import type { ReviewDecision, ReviewFixture, ReviewIssue, ReviewLink } from "../../design/mock/fixtures/review";
import {
  Badge,
  Button,
  Card,
  cx,
  EmptyState,
  IconButton,
  LineGlyph,
  LiveStatus,
  Notice,
  Page,
  PendingChange,
  SegmentedControl,
  Skeleton,
  SkeletonText,
  Tooltip,
  errorText,
  formatTime,
  lineName,
  relativeTime,
  quoted,
  useLive,
  useSetupPromptHere,
  useToast,
} from "../../ui";
import { Detail } from "./Detail";
import { keyOf, matches, QUEUE_PHRASE, rank, readQuery, RESUME_PHRASE, type ReviewQuery, sameRef, spoken, spokenRef, talkPhrase, writeQuery } from "./model";
import { Queue, QueueSkeleton } from "./Queue";
import { Week } from "./Week";
import "./review.css";

type Busy = false | "ask" | "resume" | "confirm" | "cancel";

/**
 * What happened to a waiting change, by issue key. `id` is the proposal's audit id, so a new
 * proposal on the same issue starts clean; `here` is true when this page's Confirm or Cancel
 * settled it. Statuses come from the daemon's answer or its audit entry, never from a guess.
 */
interface Outcome {
  id: string;
  status: "executed" | "cancelled" | "expired";
  at: string;
  via: ReviewDecision["via"];
  here: boolean;
  reason?: string;
}

/** The one line the detail shows for a minute after a waiting change settled without a yes. */
function outcomeNote(o: Outcome | undefined): string | null {
  if (!o || o.status === "executed") return null;
  if (o.status === "expired") return "Your okay ran out, so Ghost left it alone.";
  if (o.here) return "You canceled it, so Ghost left it alone.";
  if (o.reason === "session ended") return "The session ended before you said yes, so Ghost left it alone.";
  return "You said no, so Ghost left it alone.";
}

/** "ghost-mock/app#151" -> { repo, number } */
function parseTarget(t: string | null | undefined): { repo: string; number: number } | null {
  const m = /^([\w.-]+\/[\w.-]+)#(\d+)$/.exec(t ?? "");
  return m ? { repo: m[1], number: Number(m[2]) } : null;
}

export default function ReviewPrototype(_: RouteProps) {
  const data = useFixture<ReviewFixture>("review");
  const { client, status, pending } = useGhost();
  const live = useLive();
  const toast = useToast();
  const [q, setQState] = useState<ReviewQuery>(readQuery);
  const setQ = useCallback((patch: Partial<ReviewQuery>) => {
    setQState((prev) => {
      const next = { ...prev, ...patch };
      writeQuery(next);
      return next;
    });
  }, []);
  const [busy, setBusy] = useState<Busy>(false);
  const [error, setError] = useState<{ title: string; text: string } | null>(null);
  /** An issue the operator brought up from this page; Ghost is on it until the session ends. */
  const [asked, setAsked] = useState<{ repo: string; number: number } | null>(null);
  /** What happened to each waiting change seen on this page, by issue key. */
  const [outcomes, setOutcomes] = useState<Record<string, Outcome>>({});
  const root = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);
  const [dockH, setDockH] = useState(0);
  /** The issue whose waiting change settled in the last minute: the detail says how. */
  const [recent, setRecent] = useState<string | null>(null);
  /** Issues whose waiting change this page is confirming or canceling right now. */
  const settling = useRef(new Set<string>());
  const [starting, setStarting] = useState(false);

  const inSession = !!status?.session_id;
  // Dots beside stale data: the live phase, or a still amber while Ghost isn't answering
  // (the same tone LiveStatus uses for "Reconnecting"), never the red of "Not running".
  const dotPhase = live.online ? live.phase : "starting";
  const setupNeeded = live.setupNeeded;
  useSetupPromptHere(setupNeeded && live.online);
  useEffect(() => {
    if (!inSession) setAsked(null);
  }, [inSession]);

  // ------------------------------------------------------------------ derived data
  // The fixture's "pending" decisions follow the live proposal: still waiting while it is,
  // then whatever the daemon says happened to it.
  const livePending = parseTarget(pending?.target);
  const pendingKey = livePending ? keyOf(livePending) : null;
  const pendingId = pending?.id ?? null;
  const pendingIdRef = useRef(pendingId);
  pendingIdRef.current = pendingId;
  const outcomesRef = useRef(outcomes);
  outcomesRef.current = outcomes;

  const record = useCallback((key: string, o: Outcome) => {
    setOutcomes((prev) => ({ ...prev, [key]: o }));
    setRecent(key);
  }, []);
  useEffect(() => {
    if (!recent) return;
    const t = window.setTimeout(() => setRecent(null), 60_000);
    return () => window.clearTimeout(t);
  }, [recent, outcomes]);

  /** Ask the audit log what became of a proposal this page didn't settle: a spoken yes or no,
   * the session ending, or the okay running out. If it can't say, the page says nothing. */
  const lookUp = useCallback(
    async (key: string, id: string) => {
      try {
        const page = await client.audit({ limit: 50 });
        const e = page.items.find((a) => a.id === id);
        if (!e || (e.status !== "executed" && e.status !== "cancelled" && e.status !== "expired")) return;
        const status = e.status;
        const last = [...e.events].reverse().find((ev) => ev.event === (status === "executed" ? "finished" : status));
        const turn = e.events.find((ev) => ev.event === "user_turn");
        const reason = typeof last?.detail?.reason === "string" ? last.detail.reason : undefined;
        const via: ReviewDecision["via"] = turn?.detail?.classifier === "ui" ? "ui" : turn?.detail?.classifier === "text" ? "text" : "voice";
        record(key, { id, status, at: last?.at ?? new Date().toISOString(), via, here: false, reason });
      } catch {
        /* unknown: no line rather than a guessed one */
      }
    },
    [client, record],
  );

  // The proposal went away. If this page is settling it, settle() records the answer; if
  // not, the audit log says what happened. Leaving is never read as "it ran out".
  const lastPending = useRef<{ key: string; id: string } | null>(pendingKey && pendingId ? { key: pendingKey, id: pendingId } : null);
  useEffect(() => {
    const was = lastPending.current;
    lastPending.current = pendingKey && pendingId ? { key: pendingKey, id: pendingId } : null;
    if (!was || was.id === pendingId) return;
    if (settling.current.has(was.key) || outcomesRef.current[was.key]?.id === was.id) return;
    void lookUp(was.key, was.id);
  }, [pendingKey, pendingId, lookUp]);
  const issues = useMemo(
    () =>
      (data?.issues ?? []).map((i) => {
        const k = keyOf(i);
        const outcome = outcomes[k];
        if (!i.decisions.some((d) => d.status === "pending")) return i;
        if (livePending && sameRef(livePending, i)) return i;
        // No live proposal and no answer from this page or the audit log: the fixture's own
        // reading, that it lapsed before this page opened.
        const status = outcome?.status ?? "expired";
        return {
          ...i,
          state: outcome?.status === "executed" ? ("decided" as const) : i.state,
          decisions: i.decisions.map((d) => (d.status === "pending" ? { ...d, status, ...(outcome ? { via: outcome.via, at: outcome.at } : {}) } : d)),
        };
      }),
    [data?.issues, outcomes, livePending?.repo, livePending?.number],
  );
  const weights = data?.weights ?? { impact: 30, readiness: 25, urgency: 20, markers: 15, age: 10 };
  const filters = data?.filters ?? [];
  const filter = filters.find((f) => f.id === q.filter) ?? filters[0];
  const waitingAll = useMemo(() => rank(issues.filter((i) => i.state === "waiting"), weights), [issues, weights]);
  const waiting = useMemo(() => waitingAll.filter((i) => matches(i, filter)), [waitingAll, filter]);
  const decided = useMemo(() => issues.filter((i) => i.state === "decided"), [issues]);
  const counts = useMemo(() => Object.fromEntries(filters.map((f) => [f.id, waitingAll.filter((i) => matches(i, f)).length])), [filters, waitingAll]);
  const visible = useMemo(() => [...waiting, ...decided], [waiting, decided]);

  // What Ghost is on right now: the waiting change's target, else what was asked from here,
  // else what the session was last talking about.
  const pendingRef = livePending;
  const fixtureNow = inSession ? data?.discussing ?? null : null;
  const nowRef = pendingRef ?? asked ?? fixtureNow;
  const nowIssue = nowRef ? issues.find((i) => sameRef(i, nowRef)) ?? null : null;
  const line = status?.conversation ?? fixtureNow?.line ?? "ghost";
  const lineLabel = lineName(line, fixtureNow?.line === line ? fixtureNow.lineName : undefined);

  const selectedKey = (q.issue && visible.some((i) => keyOf(i) === q.issue) ? q.issue : null) ?? (nowIssue && visible.includes(nowIssue) ? keyOf(nowIssue) : null) ?? (visible[0] ? keyOf(visible[0]) : null);
  const selected = visible.find((i) => keyOf(i) === selectedKey) ?? null;
  const at = selected ? visible.indexOf(selected) : -1;
  const position = selected && selected.state === "waiting" ? waiting.indexOf(selected) + 1 : null;

  const select = useCallback(
    (key: string) => {
      setQ({ issue: key });
      // Bring the detail's top into view if the reader had scrolled past it. main.scrollBy,
      // not scrollIntoView, so Tab order stays where the reader is.
      requestAnimationFrame(() => {
        const main = document.getElementById("main");
        const el = detailRef.current;
        if (!main || !el) return;
        const top = el.getBoundingClientRect().top - main.getBoundingClientRect().top;
        if (top < 0) main.scrollBy({ top: top - 64, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
      });
    },
    [setQ],
  );
  const step = useCallback(
    (d: 1 | -1) => {
      const n = visible[at + d];
      if (n) select(keyOf(n));
    },
    [visible, at, select],
  );

  // J and K move through the queue, as in a mail client. Never while typing or in a dialog.
  useEffect(() => {
    if (q.view !== "queue") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.closest("input, textarea, select, [contenteditable='true'], dialog") || t.isContentEditable)) return;
      if (e.key === "j") step(1);
      else if (e.key === "k") step(-1);
      else return;
      e.preventDefault();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [q.view, step]);

  // The dock covers the panel's bottom while a change waits; the queue bounds itself by it.
  useLayoutEffect(() => {
    const dock = root.current?.closest(".g-page")?.querySelector<HTMLElement>(".g-page__dock");
    if (!dock) {
      setDockH(0);
      return;
    }
    const measure = () => setDockH(dock.offsetHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(dock);
    return () => ro.disconnect();
  }, [!!pending]);

  // ---------------------------------------------------------------------- actions
  const canTalk = live.online && !live.loadingModels && !setupNeeded;
  const talkWhy =
    live.connection === "offline"
      ? "Ghost isn’t running."
      : live.connection === "reconnecting"
        ? "Reconnecting to Ghost…"
        : live.connection === "connecting"
          ? "Connecting to Ghost…"
          : live.loadingModels
            ? "Ghost is loading its voice models."
            : setupNeeded
              ? "Finish setup first."
              : null;
  // The shell's banner already says why while models load or the connection is back to
  // reconnecting: then the page's own spots show no second copy of the reason, and the
  // disabled buttons carry it for hover and screen readers only.
  const bannerSaysWhy = live.connection === "reconnecting" || (live.online && live.loadingModels);
  const talk = { can: canTalk, why: talkWhy, shown: bannerSaysWhy ? null : talkWhy };

  // "undo that" reverses Ghost's last write only: the newest executed decision this session,
  // or whatever was just confirmed from this page.
  const lastWriteKey = useMemo(() => {
    let best: { k: string; t: number } | null = null;
    for (const i of issues)
      for (const d of i.decisions)
        if (d.thisSession && d.status === "executed" && (!best || Date.parse(d.at) > best.t)) best = { k: keyOf(i), t: Date.parse(d.at) };
    return best?.k ?? null;
  }, [issues]);

  const start = async () => {
    setStarting(true);
    try {
      await startDaemon();
    } catch (e) {
      setError({ title: "Ghost didn’t start", text: errorText(e) });
    } finally {
      setStarting(false);
    }
  };

  const run = async (kind: Exclude<Busy, false>, title: string, f: () => Promise<unknown>) => {
    setBusy(kind);
    setError(null);
    try {
      await f();
      return true;
    } catch (e) {
      setError({ title, text: errorText(e) });
      return false;
    } finally {
      setBusy(false);
    }
  };

  // The button sends the same phrase the screen shows beside it: "open app 212" to wake Ghost
  // from passive, "tell me about app 212" into a live session. One fact, one wording.
  const ask = async (i: ReviewIssue) => {
    const text = talkPhrase(i, inSession);
    const ok = await run("ask", "Couldn’t reach Ghost", () => (inSession ? client.say(text) : client.wake(text)));
    if (!ok) return;
    setAsked({ repo: i.repo, number: i.number });
    toast({ title: inSession ? "Brought up with Ghost" : "Ghost is awake and on it", description: quoted(spoken(text, inSession)), action: { label: "Open session", onClick: () => navigate("/") } });
  };

  const resume = async () => {
    const r = data?.resume;
    if (!r) return;
    const ok = await run("resume", "Couldn’t wake Ghost", () => client.wake("Continue where I left off."));
    if (!ok) return;
    setAsked({ repo: r.repo, number: r.number });
    select(keyOf(r));
    toast({ title: "Picking up where you left off", description: `Ghost checks ${spokenRef(r)} on GitHub again before anything else.` });
  };

  // Confirm calls client.confirm() and nothing else (ADR 0013); Cancel calls client.cancel().
  // The intent is marked before the call, because the event stream clears the proposal
  // before the call returns; the outcome is recorded only once the daemon answers. If the
  // call fails after the proposal left (it ran out at the same moment), the audit log says
  // what happened.
  const settle = async (kind: "confirm" | "cancel") => {
    const target = livePending;
    const id = pendingId;
    const key = target ? keyOf(target) : null;
    if (key) settling.current.add(key);
    const ok = await run(kind, kind === "confirm" ? "Couldn’t confirm" : "Couldn’t cancel", () => (kind === "confirm" ? client.confirm() : client.cancel()));
    if (!key || !id) return;
    settling.current.delete(key);
    if (ok) record(key, { id, status: kind === "confirm" ? "executed" : "cancelled", at: new Date().toISOString(), via: "ui", here: true });
    else if (pendingIdRef.current !== id) void lookUp(key, id);
  };

  const openLink = (l: ReviewLink) => {
    const k = keyOf(l);
    return visible.some((i) => keyOf(i) === k) ? () => select(k) : undefined;
  };

  // ------------------------------------------------------------------------ views
  // The week stays reachable after the queue is cleared, which is when it's most worth a look.
  const viewSwitch =
    data && !setupNeeded && (issues.length > 0 || data.week.decided > 0 || q.view === "week") ? (
      <SegmentedControl
        aria-label="View"
        value={q.view}
        onChange={(v) => setQ({ view: v })}
        options={[
          { value: "queue", label: q.state === "loading" ? "Queue" : <>Queue <span className="rev-seg-count t-num">{waitingAll.length}</span></> },
          { value: "week", label: "This week" },
        ]}
      />
    ) : undefined;

  // Waiting for okay is a state of every view: the dock shows on the queue and the week alike.
  const dock =
    pending ? (
      <PendingChange
        summary={pending.summary}
        rationale={pending.rationale}
        operation={`${pending.ability}.${pending.operation}`}
        expiresAt={pending.expires_at}
        busy={busy === "confirm" ? "confirm" : busy === "cancel" ? "cancel" : busy !== false}
        disabled={!live.online}
        onConfirm={() => settle("confirm")}
        onCancel={() => settle("cancel")}
      />
    ) : undefined;

  let body: ReactNode;
  if (!data) {
    body = <EmptyState size="page" icon={<Inbox />} title="The review queue is a prototype" description="It has no API behind it yet. Open it with a mock scenario, like ?mock=passive, to see it." />;
  } else if (setupNeeded) {
    body = (
      <EmptyState
        size="page"
        icon={<Inbox />}
        title="Ghost builds this queue once it’s set up"
        description="Ghost needs a model key before it can read your repositories and rank what’s waiting on you."
        actions={<Button variant="primary" href={live.setupHref}>Finish setup</Button>}
      />
    );
  } else if (q.state === "loading") {
    body = (
      <div className="rev-layout" aria-busy="true" aria-label="Loading the queue">
        <QueueSkeleton />
        <Card padding="lg" className="rev-skel-detail" aria-hidden>
          <Skeleton width={160} height={12} />
          <Skeleton width="56%" height={16} />
          <Skeleton width="40%" height={12} />
          <span className="rev-skel-detail__gap" />
          <Skeleton width={110} height={10} />
          <Skeleton width="80%" height={20} />
          <SkeletonText lines={3} />
        </Card>
      </div>
    );
  } else if (issues.length === 0 && q.view !== "week") {
    body = (
      <EmptyState
        size="page"
        icon={<Inbox />}
        title="Nothing waiting on you"
        description={<>When an issue needs your yes or no, it shows up here with the reason.{canTalk && <> You can also ask: {quoted(spoken(QUEUE_PHRASE, inSession))}.</>}</>}
        actions={data.week.decided > 0 ? <Button variant="secondary" onClick={() => setQ({ view: "week" })}>See this week</Button> : undefined}
      />
    );
  } else if (q.view === "week") {
    const oldest = waitingAll.reduce<string | null>((o, i) => (!o || Date.parse(i.waitingSince) < Date.parse(o) ? i.waitingSince : o), null);
    body = <Week week={data.week} waiting={waitingAll.length} oldest={oldest} issues={visible} inSession={inSession} canTalk={canTalk} onOpen={(k) => { setQ({ view: "queue" }); select(k); }} />;
  } else {
    // The context line stays while a change waits (it's the same issue the dock names), so the
    // list never jumps when the dock clears, and on an empty filter: what Ghost is on doesn't
    // depend on which filter is open.
    const filterEmpty = visible.length === 0;
    const resumeShown = !inSession && live.connection !== "offline" ? data.resume : null;
    const contextLine = (
      <Context
        now={nowIssue && inSession ? { issue: nowIssue, lineSlug: line, lineLabel, lastSaid: fixtureNow && sameRef(fixtureNow, nowIssue) ? fixtureNow.lastSaid : null } : null}
        resume={resumeShown}
        selectedKey={selectedKey}
        onShow={select}
        onResume={resume}
        resumeBusy={busy === "resume"}
        talk={talk}
        brief={!!pending}
      />
    );
    const n = waitingAll.length;
    const rest = filter.empty?.rest ?? "outside this filter";
    body = (
      <div className="rev-layout">
        <Queue
          waiting={waiting}
          decided={decided}
          filters={filters}
          filter={filter}
          counts={counts}
          onFilter={(id) => setQ({ filter: id, issue: null })}
          selected={selectedKey}
          onSelect={select}
          discussing={nowIssue && inSession ? { repo: nowIssue.repo, number: nowIssue.number, phase: dotPhase } : null}
          context={contextLine}
          dockHeight={dockH}
          canTalk={canTalk}
          inSession={inSession}
          empty={filterEmpty}
        />
        {filterEmpty && (
          <Card padding="lg" className="rev-filter-empty">
            <EmptyState
              icon={<Inbox />}
              title={filter.empty?.title ?? `Nothing in “${filter.label}”`}
              description={
                <>
                  {n === 0 ? "Nothing is waiting on you right now." : n === 1 ? `The one issue waiting on you is ${rest}.` : `All ${n} issues waiting on you are ${rest}.`}
                  {canTalk && <><br />Or say {quoted(spoken(QUEUE_PHRASE, inSession))}.</>}
                </>
              }
              actions={<Button variant="secondary" onClick={() => setQ({ filter: "all", issue: null })}>Show everything</Button>}
            />
          </Card>
        )}
        {selected && (
          <Detail
            ref={detailRef}
            key={selectedKey}
            issue={selected}
            position={position}
            total={waiting.length}
            weights={weights}
            onPrev={at > 0 ? () => step(-1) : undefined}
            onNext={at < visible.length - 1 ? () => step(1) : undefined}
            openLink={openLink}
            inSession={inSession}
            waitingOkay={!!pendingRef && sameRef(pendingRef, selected)}
            note={recent === selectedKey && selectedKey ? outcomeNote(outcomes[selectedKey]) : null}
            lastWrite={lastWriteKey === selectedKey}
            talk={talk}
            resume={resumeShown && sameRef(resumeShown, selected) ? { note: resumeShown.note, at: resumeShown.at } : null}
            offline={live.connection === "offline"}
            ask={{ onClick: () => ask(selected), loading: busy === "ask", disabled: !canTalk || (busy !== false && busy !== "ask"), primary: !pending && !inSession }}
          />
        )}
      </div>
    );
  }

  return (
    <Page
      width="wide"
      title="Review queue"
      description={q.view === "week" ? "What you decided this week and what it freed up." : "Issues waiting on your yes or no, ranked by what a decision unblocks."}
      status={<LiveStatus />}
      actions={viewSwitch}
      dock={dock}
    >
      <div ref={root} className="rev-root">
        {live.connection === "offline" && data && issues.length > 0 && (
          <Notice
            title={`The queue as of ${relativeTime(data.checkedAt)}`}
            className="rev-notice"
            action={inTauri ? <Button size="sm" variant="secondary" loading={starting} onClick={start}>Start Ghost</Button> : undefined}
          >
            Start Ghost to refresh it and talk any of it through.
            {!inTauri && <StartCommand />}
          </Notice>
        )}
        {q.state === "error" && data && (
          <Notice
            tone="warn"
            title={<span className="rev-notice__title">Couldn’t refresh from GitHub <Badge size="sm" variant="outline">Prototype</Badge></span>}
            className="rev-notice"
            action={
              <Button
                size="sm"
                variant="secondary"
                onClick={() => {
                  setQ({ state: null });
                  // Honest about the prototype: there is no refresh behind this yet.
                  toast({ title: `Showing the queue as of ${formatTime(data.checkedAt)}`, description: "Nothing refreshes yet: the queue is prototype data." });
                }}
              >
                Try again
              </Button>
            }
          >
            GitHub answered with a rate limit at {formatTime(data.checkedAt)}. This is the queue as of then; Ghost tries again in 4 minutes.
          </Notice>
        )}
        {error && (
          <Notice tone="danger" title={error.title} onDismiss={() => setError(null)} className="rev-notice">
            {error.text}
          </Notice>
        )}
        {body}
      </div>
    </Page>
  );
}

/* ---------------------------------------------------------------- context line */
/** The shell's command, word for word, so the two offline states say the same thing. */
const START_COMMAND = "systemctl --user enable --now ghostd";

/** The command with a Copy button, as the shell's Offline page draws it. */
function StartCommand() {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(START_COMMAND);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard unavailable: the command is selectable */
    }
  };
  return (
    <span className="rev-notice__cmd">
      <code className="t-mono">{START_COMMAND}</code>
      <IconButton size="sm" label={copied ? "Copied" : "Copy command"} icon={copied ? <Check /> : <Copy />} onClick={copy} />
    </span>
  );
}

interface ContextProps {
  now: { issue: ReviewIssue; lineSlug: string; lineLabel: string; lastSaid: string | null } | null;
  resume: ReviewFixture["resume"];
  selectedKey: string | null;
  onShow: (key: string) => void;
  onResume: () => void;
  resumeBusy: boolean;
  talk: { can: boolean; why: string | null; shown: string | null };
  /** A change waits (the dock names the same issue): the quote drops to one line. */
  brief?: boolean;
}

/**
 * One quiet block above the list: what Ghost is on right now (the issue on one line, the last
 * thing it said on up to two under it), or where the last session stopped. Never both, and
 * both the same weight. The resume note and its phrase live in the detail when that issue
 * is selected.
 */
function Context({ now, resume, selectedKey, onShow, onResume, resumeBusy, talk, brief }: ContextProps) {
  if (now) {
    const k = keyOf(now.issue);
    return (
      <div className="rev-ctx">
        <LineGlyph slug={now.lineSlug} name={now.lineLabel} size="sm" />
        <p className="rev-ctx__text t-small">
          <span className="rev-ctx__label t-1 t-medium">
            On <span className="t-num">{spokenRef(now.issue)}</span>
            <span className="t-3 t-regular"> · {now.lineLabel} line</span>
          </span>
          {now.lastSaid && <span className={cx("rev-ctx__quote t-2", brief && "rev-ctx__quote--one")}>{quoted(now.lastSaid)}</span>}
        </p>
        {selectedKey !== k && (
          <Button size="sm" variant="ghost" onClick={() => onShow(k)}>
            Show it
          </Button>
        )}
      </div>
    );
  }
  if (resume) {
    const k = keyOf(resume);
    return (
      <div className="rev-ctx" role="group" aria-labelledby="rev-resume-title">
        <span className="rev-ctx__icon" aria-hidden>
          <History />
        </span>
        <p className="rev-ctx__text rev-ctx__text--row t-small">
          {selectedKey === k ? (
            <span id="rev-resume-title" className="t-1 t-medium">
              Left off on <span className="t-num">{spokenRef(resume)}</span>
            </span>
          ) : (
            // The note itself is in the detail once the issue is shown; the Tooltip is a preview.
            <Tooltip content={resume.note} side="bottom">
              <button type="button" id="rev-resume-title" className="rev-ctx__link t-1 t-medium" onClick={() => onShow(k)}>
                Left off on <span className="t-num">{spokenRef(resume)}</span>
              </button>
            </Tooltip>
          )}
          <span className="t-3 t-num"> · {relativeTime(resume.at)}</span>
        </p>
        <Tooltip content={!talk.can ? talk.why : null}>
          <Button size="sm" variant="ghost" onClick={onResume} loading={resumeBusy} disabled={!talk.can} aria-describedby={!talk.can && talk.why ? "rev-resume-why" : undefined}>
            Continue
          </Button>
        </Tooltip>
        {!talk.can && talk.why && <span id="rev-resume-why" className="sr-only">{talk.why}</span>}
      </div>
    );
  }
  return null;
}
