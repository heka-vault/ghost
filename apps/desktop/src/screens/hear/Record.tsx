/**
 * Owner: hear. Record: the take to say, hold to talk, and the take's check (ADR 0042).
 *
 * Space (or the big button) held down records a take; release ends it, and the daemon keeps
 * half a second either side. A take across the room starts with one press, counts down and
 * ends on silence; a silent take starts with one press and runs the plan's time. Escape
 * cancels. After a take its check shows: Enter goes to the next take, R records this one
 * again, P plays it, S skips it. What Ghost heard is shown here and nowhere else.
 */
import type { HearBlock, HearCheck, HearPlan, HearSession, HearView, TakeEnding } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CircleCheck, CircleDashed, CircleSlash, Footprints, Music, Play, RotateCcw, SkipForward, Square, TriangleAlert, X } from "lucide-react";
import { type ButtonHTMLAttributes, type CSSProperties, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Badge, Button, Card, EmptyState, Kbd, Menu, Notice, Skeleton, Spinner, cx, errorText, scrollIntoPanel, useLive } from "../../ui";
import type { Lease } from ".";
import { Elapsed, LevelMeter } from "./LevelMeter";
import { type PlanEntry, STATUS_WORDS, type TakeStatus, dbfs, entriesOf, lineParts, nextTake, progressOf, refusal, secs, speechSpan, takeStatus, takesById } from "./model";
import { IDLE, type Press, type PressCall, type PressInput, endingFor, press, shortcutFor } from "./press";
import { dialogOpen, isActivatable, isTyping, serial } from "./recorder";

type PointerHandlers = Pick<ButtonHTMLAttributes<HTMLButtonElement>, "onPointerDown" | "onPointerUp" | "onPointerCancel" | "onLostPointerCapture" | "onClick">;

const KIND_WORDS: Record<string, string> = { speech: "Request", reply: "Reply", silence: "Silent" };

interface Props {
  view: HearView | null;
  plan: HearPlan | null;
  planError: string | null;
  onRetryPlan: () => void;
  lease: Lease;
  onScore: (session: string) => void;
}

export function RecordTab({ view, plan, planError, onRetryPlan, lease, onScore }: Props) {
  const { client, hear } = useGhost();
  const { online } = useLive();
  const state = view?.state ?? "closed";
  const open = state !== "closed" && state !== "opening";
  const sessionName = open ? (view?.session ?? null) : null;

  // ------------------------------------------------------------------ the session's takes
  const [session, setSession] = useState<HearSession | null>(null);
  const [sessionError, setSessionError] = useState<string | null>(null);
  const loadSession = useCallback(
    (name: string) =>
      client
        .hearSession(name)
        .then((s) => (setSession(s), setSessionError(null), s))
        .catch((e) => {
          setSessionError(errorText(e));
          return null;
        }),
    [client],
  );
  // Read again each time the recorder opens: a session closed with no takes is gone, and the
  // same name can come back as a new one.
  const [openGen, setOpenGen] = useState(0);
  const wasOpen = useRef(open);
  useEffect(() => {
    if (open && !wasOpen.current) setOpenGen((n) => n + 1);
    wasOpen.current = open;
  }, [open]);
  useEffect(() => {
    if (!sessionName) return;
    setSession((s) => (s?.name === sessionName ? s : null));
    void loadSession(sessionName);
  }, [sessionName, loadSession, openGen]);

  // ------------------------------------------------------------------ the take on screen
  const entries = useMemo(() => entriesOf(plan), [plan]);
  const byId = useMemo(() => takesById(session), [session]);
  const [selected, setSelected] = useState<string | null>(null);
  const [check, setCheck] = useState<HearCheck | null>(null);
  const [busy, setBusy] = useState<null | "start" | "stop" | "cancel" | "skip" | "play">(null);
  const [error, setError] = useState<{ title: string; text: string } | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [held, setHeld] = useState(false);
  /** A take across the room or a silent one: its check comes when the recorder is ready again. */
  const auto = useRef<string | null>(null);
  /** The last start failed: the release that follows it has nothing to end. */
  const startFailed = useRef(false);

  // A take going is the daemon's; otherwise the one the operator is on.
  const currentId = view?.take ?? selected;
  const current: PlanEntry | null = entries.find((e) => e.take.id === currentId) ?? null;
  const ending: TakeEnding | null = current ? endingFor(current.take.kind, current.block.away) : null;
  const shownCheck = check && check.take === currentId && check.session === sessionName ? check : null;

  // First take: the one a waiting check is about, or the first not yet done.
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const sessionReady = !!session && session.name === sessionName;
  const seeded = useRef<string | null>(null);
  useEffect(() => {
    if (!plan || !sessionReady || !sessionName || seeded.current === sessionName) return;
    seeded.current = sessionName;
    let on = true;
    setSelected(nextTake(plan, sessionRef.current, null));
    setCheck(null);
    // A check left from before (a reload with the recorder still open) carries on.
    if (!view?.checked) return;
    client
      .hearCheck()
      .then((c) => {
        if (!on || c.session !== sessionName || !entriesOf(plan).some((e) => e.take.id === c.take)) return;
        setCheck(c);
        setSelected(c.take);
      })
      .catch(() => {});
    return () => {
      on = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- seeded once per session
  }, [plan, sessionReady, sessionName, client]);

  // A take that ended on its own: read its check when the recorder is ready again.
  const prevState = useRef(state);
  useEffect(() => {
    const was = prevState.current;
    prevState.current = state;
    const id = auto.current;
    if (!id || !sessionName || state !== "ready" || was === "ready") return;
    auto.current = null;
    if (was !== "checking" && was !== "stopping" && was !== "recording") return;
    client
      .hearCheck()
      .then((c) => {
        if (c.take === id) setCheck(c);
      })
      .catch(() => {})
      .finally(() => void loadSession(sessionName));
  }, [state, sessionName, client, loadSession]);

  // ------------------------------------------------------------------ calls
  const fail = (title: string, e: unknown) => setError({ title, text: refusal(e) });
  const perform = (call: PressCall) => {
    if (!call) return;
    setError(null);
    if (call.call === "start") {
      setHint(null);
      setCheck(null);
      startFailed.current = false;
      auto.current = call.ending === "hold" ? null : call.take;
      setBusy("start");
      serial(() => client.hearStart(call.take, call.ending))
        .then((v) => hear.setView(v))
        .catch((e) => {
          startFailed.current = true;
          auto.current = null;
          fail("Couldn’t start the take", e);
        })
        .finally(() => setBusy((b) => (b === "start" ? null : b)));
    } else if (call.call === "stop") {
      setBusy("stop");
      serial(() => client.hearStop())
        .then((c) => {
          setCheck(c);
          if (sessionName) void loadSession(sessionName);
        })
        .catch((e) => {
          if (!startFailed.current) fail("Couldn’t check the take", e);
        })
        .finally(() => setBusy((b) => (b === "stop" ? null : b)));
    } else {
      auto.current = null;
      if (call.short) setHint("Hold Space, or the button, for as long as you speak: a tap isn’t a take.");
      setBusy("cancel");
      serial(() => client.hearCancel())
        .then((v) => hear.setView(v))
        .catch((e) => {
          if (!startFailed.current && !call.short) fail("Couldn’t cancel the take", e);
        })
        .finally(() => setBusy((b) => (b === "cancel" ? null : b)));
    }
  };

  const goNext = () => {
    if (!plan) return;
    setCheck(null);
    setHint(null);
    setSelected(nextTake(plan, session, currentId));
  };
  const redo = () => {
    setCheck(null);
    setHint(null);
  };
  const play = (channel: "aec" | "raw" = "aec") => {
    if (!sessionName || !currentId) return;
    setBusy("play");
    client
      .hearPlay(sessionName, currentId, channel)
      .catch((e) => fail("Couldn’t play the take", e))
      .finally(() => setBusy((b) => (b === "play" ? null : b)));
  };
  const skip = () => {
    if (!currentId || !plan) return;
    setBusy("skip");
    client
      .hearSkip(currentId)
      .then((s) => {
        setSession(s);
        setCheck(null);
        setSelected(nextTake(plan, s, currentId));
      })
      .catch((e) => fail("Couldn’t skip the take", e))
      .finally(() => setBusy((b) => (b === "skip" ? null : b)));
  };
  const takeCard = useRef<HTMLDivElement>(null);
  const jump = (id: string) => {
    if (going(state)) return;
    setSelected(id);
    setCheck(null);
    setHint(null);
    // The strip runs long; bring the take's card back into view under the page bar.
    requestAnimationFrame(() => {
      const el = takeCard.current;
      const bar = document.getElementById("main");
      if (el && bar && el.getBoundingClientRect().top < bar.getBoundingClientRect().top + 64) scrollIntoPanel(el, { block: "start" });
    });
  };

  // ------------------------------------------------------------------ hold to talk
  const machine = useRef<Press>(IDLE);
  const canStart = online && open && !busy && !shownCheck && !!current;
  const ctx = { state, take: current?.take.id ?? null, ending, canStart };
  const latest = useRef({ ctx, shownCheck, recorded: false, goNext, redo, play, skip });
  latest.current = { ctx, shownCheck, recorded: !!(currentId && byId.get(currentId)?.state === "recorded"), goNext, redo, play, skip };

  const dispatch = useCallback((input: PressInput) => {
    const l = latest.current;
    const { next, call } = press(machine.current, input, l.ctx);
    machine.current = next;
    setHeld(!!next.held);
    if (!call && input.type === "down" && !input.repeat && l.shownCheck && l.ctx.state === "ready") setHint("Press Enter for the next take, or R to record this one again.");
    perform(call);
    return call;
    // perform reads state through refs and setters only
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (dialogOpen() || isTyping(e.target) || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.code === "Space" || e.key === " ") {
        e.preventDefault();
        dispatch({ type: "down", source: "key", at: performance.now(), repeat: e.repeat });
        return;
      }
      if (e.key === "Escape") {
        if (dispatch({ type: "escape" })) e.preventDefault();
        return;
      }
      if (e.repeat || (e.key === "Enter" && isActivatable(e.target))) return;
      const l = latest.current;
      const s = shortcutFor(e.key, { state: l.ctx.state, checked: !!l.shownCheck, recorded: l.recorded });
      if (!s) return;
      e.preventDefault();
      if (s === "next") l.goNext();
      if (s === "redo") l.redo();
      if (s === "play") l.play();
      if (s === "skip") l.skip();
    };
    const up = (e: KeyboardEvent) => {
      if (e.code !== "Space" && e.key !== " ") return;
      if (isTyping(e.target)) return;
      // Even with a dialog open: a held take always ends on the release.
      e.preventDefault();
      dispatch({ type: "up", source: "key", at: performance.now() });
    };
    const blur = () => dispatch({ type: "blur", at: performance.now() });
    const hidden = () => document.visibilityState === "hidden" && blur();
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", blur);
    document.addEventListener("visibilitychange", hidden);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", blur);
      document.removeEventListener("visibilitychange", hidden);
      // Leaving mid-hold ends the take; the recorder's close follows.
      blur();
    };
  }, [dispatch]);

  const pointer: PointerHandlers = {
    onPointerDown: (e: ReactPointerEvent<HTMLButtonElement>) => {
      if (e.button !== 0) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      dispatch({ type: "down", source: "pointer", at: performance.now() });
    },
    onPointerUp: () => dispatch({ type: "up", source: "pointer", at: performance.now() }),
    onPointerCancel: () => dispatch({ type: "up", source: "pointer", at: performance.now() }),
    onLostPointerCapture: () => dispatch({ type: "up", source: "pointer", at: performance.now() }),
    // Enter on the focused button starts a take that ends on its own; a held one needs Space.
    onClick: (e: ReactMouseEvent<HTMLButtonElement>) => {
      if (e.detail !== 0) return;
      if (ending === "hold") setHint("Hold Space while you say the line, and let go when you finish.");
      else dispatch({ type: "down", source: "key", at: performance.now() });
    },
  };

  // ------------------------------------------------------------------ what to show
  if (!view || (state === "closed" && (lease.opening || !online))) return <RecordSkeleton />;
  if (state === "opening" || (lease.opening && !open)) return <RecordSkeleton opening />;
  if (!open) {
    const why = lease.error ?? view.unavailable;
    return (
      <Card>
        <EmptyState
          icon={<CircleSlash />}
          title={why ? "The recorder can’t open now" : "The recorder is closed"}
          description={why ?? "It closes on its own after ten minutes with no take, and gives Ghost its mic back. Open it again to carry on."}
          actions={
            <Button onClick={lease.retry} disabled={!online}>
              {why ? "Try again" : "Open the recorder"}
            </Button>
          }
        />
      </Card>
    );
  }
  if (planError && !plan) {
    return (
      <Notice tone="danger" title="Couldn’t load the take list" action={<Button size="sm" onClick={onRetryPlan}>Try again</Button>}>
        {planError}
      </Notice>
    );
  }
  if (!plan || !session) return sessionError ? <Notice tone="danger" title="Couldn’t load the session">{sessionError}</Notice> : <RecordSkeleton />;

  const progress = progressOf(plan, session);
  const allDone = !currentId && progress.left === 0;
  const takeNo = current ? current.index + 1 : 0;
  const status = currentId ? takeStatus(byId.get(currentId)) : "todo";

  return (
    <div className="hear-record">
      <div className="hear-record__main">
        <Notice tone="info" className="hear-open-notice" title="Ghost isn’t listening for its name while the recorder is open">
          Leave this tab, or close the window, and it has its mic back.
        </Notice>

        <div className="hear-session-line">
          <p className="t-small t-2">
            Session <span className="t-mono t-1">{session.name}</span> · {progress.recorded} of {progress.total} recorded
            {progress.flagged > 0 && <> · {progress.flagged} worth a redo</>}
            {progress.skipped > 0 && <> · {progress.skipped} skipped</>}
          </p>
          <Button size="sm" variant="ghost" onClick={lease.newSession} disabled={going(state) || !online}>
            New session
          </Button>
        </div>

        {view.problem && (
          <Notice tone="danger" title="Something went wrong">
            {view.problem}
          </Notice>
        )}
        {error && (
          <Notice tone="danger" title={error.title} onDismiss={() => setError(null)}>
            {error.text}
          </Notice>
        )}

        {allDone ? (
          <Card>
            <EmptyState
              icon={<CircleCheck />}
              title="Every take in the list is done"
              description="Score the session to see how well Ghost heard you, or pick a take on the right to record it again."
              actions={
                <Button variant="primary" onClick={() => onScore(session.name)}>
                  Score this session
                </Button>
              }
            />
          </Card>
        ) : current ? (
          <>
            <BlockCard block={current.block} entering={current.first || !current.block.takes.some((t) => byId.has(t.id))} />
            <Card ref={takeCard} padding="lg" className={cx("hear-take", shownCheck && "has-check")}>
              <div className="hear-take__over">
                <span className="t-overline t-2">
                  Take {takeNo} of {entries.length}
                </span>
                <span className="t-mono t-small t-2">{current.take.id}</span>
                <Badge size="sm">{KIND_WORDS[current.take.kind] ?? current.take.kind}</Badge>
                {status !== "todo" && !going(state) && <StatusBadge status={status} />}
              </div>
              <TakeLine kind={current.take.kind} text={current.take.text ?? ""} say={current.take.say} seconds={current.take.seconds} />
              {(current.take.terms ?? []).length > 0 && (
                <p className="t-small t-2 hear-take__terms">
                  Names Ghost must hear: <span data-user-content>{(current.take.terms ?? []).join(", ")}</span>
                </p>
              )}

              {shownCheck ? (
                <CheckView
                  check={shownCheck}
                  raw={!!view.raw_device}
                  playing={!!view.playing}
                  busy={busy}
                  online={online}
                  onNext={goNext}
                  onRedo={redo}
                  onPlay={play}
                  onStopPlaying={() => void client.hearStopPlaying().catch(() => {})}
                  onSkip={skip}
                />
              ) : (
                <Control view={view} ending={ending!} held={held} canStart={canStart} busy={busy} pointer={pointer} onCancel={() => dispatch({ type: "escape" })} seconds={current.take.seconds} away={!!current.block.away} />
              )}
              {hint && <p className="hear-hint t-small" role="status">{hint}</p>}
            </Card>
            {!shownCheck && !going(state) && (
              <div className="hear-keys t-small t-2">
                <span><Kbd>Enter</Kbd> next take</span>
                <span><Kbd>S</Kbd> skip</span>
                {byId.get(current.take.id)?.state === "recorded" && <span><Kbd>P</Kbd> play</span>}
                <span><Kbd>Esc</Kbd> cancel a take</span>
              </div>
            )}
          </>
        ) : null}
      </div>

      <TakeStrip entries={entries} session={session} current={currentId} disabled={going(state)} onPick={jump} />
    </div>
  );
}

const going = (s: string) => s === "countdown" || s === "recording" || s === "stopping" || s === "checking";

// ------------------------------------------------------------------ parts

function RecordSkeleton({ opening }: { opening?: boolean }) {
  return (
    <div className="hear-record" aria-busy="true" aria-label={opening ? "Opening the recorder" : "Loading"}>
      <div className="hear-record__main">
        <Skeleton height={56} radius="var(--radius-md)" />
        <Card padding="lg" className="hear-take">
          <div className="stack gap-4">
            <Skeleton width={140} height={12} />
            <Skeleton width="80%" height={32} />
            <Skeleton width={220} height={64} radius="var(--radius-full)" />
          </div>
          {opening && (
            <p className="hear-hint t-small t-2" role="status">
              <Spinner size={14} /> Opening the recorder…
            </p>
          )}
        </Card>
      </div>
      <div className="hear-strip">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <Skeleton key={i} height={20} width={i % 3 ? "70%" : "40%"} />
        ))}
      </div>
    </div>
  );
}

/** The block's title and what to do: prominent on its first take; across the room and with music, impossible to miss. */
function BlockCard({ block, entering }: { block: HearBlock; entering: boolean }) {
  const special = block.away || block.music;
  if (!entering && !special) {
    return (
      <p className="hear-block-line t-small t-2">
        <span className="t-1">Block {block.block}</span> · {block.title || block.condition}
      </p>
    );
  }
  const icon = block.away ? <Footprints aria-hidden /> : block.music ? <Music aria-hidden /> : null;
  return (
    <Card tone={special ? "warn" : "default"} padding="md" className={cx("hear-block", special && "is-special")}>
      <div className="hear-block__head">
        {icon && <span className="hear-block__icon">{icon}</span>}
        <div>
          <p className="t-overline t-2">Block {block.block}{entering ? " · new block" : ""}</p>
          <h2 className="t-heading">{block.away ? "Walk 3 m or more from the mic" : block.music ? "Start some music yourself" : block.title || block.condition}</h2>
        </div>
      </div>
      {block.instruction && <p className="t-body-lg hear-block__text">{block.instruction}</p>}
      {block.away && <p className="t-small hear-block__text">Each take here starts with one press, counts down while you walk, and ends two seconds after your last word.</p>}
    </Card>
  );
}

/** The line, big, with each pause shown where it goes. A silent take shows what to do instead. */
function TakeLine({ kind, text, say, seconds }: { kind: string; text: string; say?: string | null; seconds?: number | null }) {
  if (kind === "silence") {
    return (
      <div className="hear-line hear-line--silent">
        <p className="t-title" data-user-content>
          {(say ?? "Say nothing").replace(/^\(|\)$/g, "")}
        </p>
        <p className="t-small t-2">{seconds ? `${seconds} seconds, saying nothing.` : "Say nothing."}</p>
      </div>
    );
  }
  const parts = lineParts(text);
  return (
    <p className="hear-line t-display" data-user-content>
      {parts.map((p, i) => (
        <span key={i}>
          {i > 0 && (
            <span className="hear-pause" aria-label="pause for about a second">
              (pause)
            </span>
          )}
          {i > 0 && " "}
          {p}
          {i < parts.length - 1 && " "}
        </span>
      ))}
    </p>
  );
}

function StatusBadge({ status }: { status: TakeStatus }) {
  const tone = status === "flagged" ? "warn" : status === "recorded" ? "ok" : "neutral";
  return (
    <Badge size="sm" tone={tone} dot>
      {STATUS_WORDS[status]}
    </Badge>
  );
}

interface ControlProps {
  view: HearView;
  ending: TakeEnding;
  held: boolean;
  canStart: boolean;
  busy: string | null;
  away: boolean;
  seconds?: number | null;
  pointer: PointerHandlers;
  onCancel: () => void;
}

/** The hold button and what the recorder is doing: ready, counting down, recording, keeping the end, checking. */
function Control({ view, ending, held, canStart, busy, pointer, onCancel, seconds, away }: ControlProps) {
  const { hear } = useGhost();
  const s = view.state;
  const live = s === "recording" || s === "stopping" || s === "countdown";
  let body;
  if (s === "countdown") {
    body = (
      <div className="hear-count" role="timer" aria-live="polite">
        <span className="hear-count__n t-num">{view.countdown ?? ""}</span>
        <span className="t-body-lg t-2">{away || ending === "quiet" ? "Walk to your spot. Starts in" : "Starts in"} {view.countdown ?? ""} s</span>
      </div>
    );
  } else if (s === "recording") {
    body = (
      <div className="hear-state">
        <span className="hear-rec-dot" aria-hidden />
        <span className="t-heading">
          Recording · <Elapsed live={hear} />
        </span>
        <span className="t-small t-2">
          {view.ending === "hold" ? "Let go when you finish the line." : view.ending === "quiet" ? "Ends two seconds after your last word." : `Say nothing. Ends after ${seconds ?? 6} s.`}
        </span>
      </div>
    );
  } else if (s === "stopping") {
    body = (
      <div className="hear-state">
        <Spinner size={14} />
        <span className="t-heading">Keeping the half second after…</span>
      </div>
    );
  } else if (s === "checking") {
    body = (
      <div className="hear-state">
        <Spinner size={14} />
        <span className="t-heading">Checking…</span>
      </div>
    );
  } else {
    const label = ending === "hold" ? "Hold to talk" : "Start (it ends on its own)";
    body = (
      <div className="hear-ready">
        <button
          type="button"
          className={cx("hear-hold", held && "is-held")}
          disabled={!canStart && !held}
          aria-pressed={ending === "hold" ? held : undefined}
          aria-describedby="hear-hold-help"
          {...pointer}
        >
          {ending === "hold" ? <span className="hear-rec-dot" aria-hidden /> : <Play aria-hidden />}
          <span>{label}</span>
          <Kbd>Space</Kbd>
        </button>
        <p id="hear-hold-help" className="t-small t-2">
          {ending === "hold"
            ? "Hold it for the whole line, pauses too, and let go when you finish. Half a second either side is kept."
            : ending === "quiet"
              ? "One press: it counts down 8 s while you walk, then ends two seconds after your last word."
              : `One press: it counts down 2 s, then records ${seconds ?? 6} s of you saying nothing.`}
        </p>
      </div>
    );
  }
  return (
    <div className="hear-control">
      {body}
      <div className="hear-control__meter">
        <LevelMeter active={s !== "checking"} />
        {live && (
          <Button size="sm" variant="ghost" leadingIcon={<X />} onClick={onCancel} loading={busy === "cancel"}>
            Cancel
          </Button>
        )}
      </div>
    </div>
  );
}

interface CheckProps {
  check: HearCheck;
  raw: boolean;
  playing: boolean;
  busy: string | null;
  online: boolean;
  onNext: () => void;
  onRedo: () => void;
  onPlay: (channel?: "aec" | "raw") => void;
  onStopPlaying: () => void;
  onSkip: () => void;
}

/** A take's check: the line against what Ghost heard, word by word, the level and where the speech sits. */
function CheckView({ check, raw, playing, busy, online, onNext, onRedo, onPlay, onStopPlaying, onSkip }: CheckProps) {
  const redo = check.warnings.filter((w) => w.redo);
  const notes = check.warnings.filter((w) => !w.redo);
  const span = speechSpan(check.speech_start, check.speech_end, check.seconds);
  const cutEnd = check.speech_end != null && check.seconds - check.speech_end < 0.15;
  const cutStart = check.speech_start != null && check.speech_start < 0.15;
  const silent = check.said === 0;
  return (
    <div className="hear-check" aria-live="polite">
      {redo.length > 0 && (
        <Notice tone="warn" title="Redo this take">
          <ul className="hear-warnings">
            {redo.map((w) => (
              <li key={w.kind}>{w.text}</li>
            ))}
          </ul>
        </Notice>
      )}
      <div className="hear-check__head">
        <h3 className="t-subheading">What Ghost heard</h3>
        <span className="t-small t-2 t-num">
          {silent ? (check.words.length ? `${check.words.length} made-up word${check.words.length === 1 ? "" : "s"}` : "Nothing, as it should") : `${check.matched} of ${check.said} words`} · peak {dbfs(check.peak)} · {secs(check.seconds)}
        </span>
      </div>
      <p className="hear-words t-body-lg" data-user-content>
        {check.words.length === 0 ? (
          <span className="t-2">{silent ? "No words." : "No words at all."}</span>
        ) : (
          check.words.map((w, i) => <Word key={i} w={w} />)
        )}
      </p>
      <div className="hear-span" role="img" aria-label={span ? `Speech from ${secs(check.speech_start)} to ${secs(check.speech_end)} of ${secs(check.seconds)}` : "No speech in the take"}>
        <span className="hear-span__track">
          {span && <span className={cx("hear-span__speech", cutStart && "cut-start", cutEnd && "cut-end")} style={{ "--from": span.from, "--to": span.to } as CSSProperties} />}
        </span>
        <span className="hear-span__labels t-caption t-2 t-num">
          <span>0 s</span>
          <span>{span ? "speech" : "no speech"}</span>
          <span>{secs(check.seconds)}</span>
        </span>
      </div>
      {notes.length > 0 && (
        <ul className="hear-notes t-small t-2">
          {notes.map((w) => (
            <li key={w.kind}>
              <TriangleAlert aria-hidden /> {w.text}
            </li>
          ))}
        </ul>
      )}
      <div className="hear-check__actions">
        <Button variant="primary" onClick={onNext} trailingIcon={<Kbd>Enter</Kbd>}>
          Next take
        </Button>
        <Button leadingIcon={<RotateCcw />} onClick={onRedo} trailingIcon={<Kbd>R</Kbd>}>
          Redo
        </Button>
        {playing ? (
          <Button variant="ghost" leadingIcon={<Square />} onClick={onStopPlaying} disabled={!online}>
            Stop playing
          </Button>
        ) : raw ? (
          <Menu
            trigger={
              <Button variant="ghost" leadingIcon={<Play />} loading={busy === "play"} disabled={!online}>
                Play
              </Button>
            }
            items={[
              { label: "Play ghost.mic", icon: <Play />, shortcut: "P", onSelect: () => onPlay("aec") },
              { label: "Play the raw array", icon: <Play />, onSelect: () => onPlay("raw") },
            ]}
          />
        ) : (
          <Button variant="ghost" leadingIcon={<Play />} onClick={() => onPlay("aec")} loading={busy === "play"} disabled={!online} trailingIcon={<Kbd>P</Kbd>}>
            Play
          </Button>
        )}
        <Button variant="ghost" leadingIcon={<SkipForward />} onClick={onSkip} loading={busy === "skip"} disabled={!online}>
          Skip
        </Button>
      </div>
    </div>
  );
}

function Word({ w }: { w: HearCheck["words"][number] }) {
  if (w.op === "same") return <span className="hear-w">{w.said} </span>;
  if (w.op === "misheard")
    return (
      <span className="hear-w is-misheard" title={`Said “${w.said}”, heard “${w.heard}”`}>
        <s>{w.said}</s>
        <span aria-hidden> → </span>
        <span className="sr-only"> heard as </span>
        {w.heard}{" "}
      </span>
    );
  if (w.op === "missed")
    return (
      <span className="hear-w is-missed" title={`Said “${w.said}”, not heard`}>
        <s>{w.said}</s>
        <span className="sr-only"> (not heard)</span>{" "}
      </span>
    );
  return (
    <span className="hear-w is-extra" title={`Heard “${w.heard}”, not said`}>
      +{w.heard}
      <span className="sr-only"> (not said)</span>{" "}
    </span>
  );
}

const STRIP_ICON: Record<TakeStatus, ReactNode> = {
  todo: <CircleDashed aria-hidden />,
  recorded: <CircleCheck aria-hidden />,
  flagged: <TriangleAlert aria-hidden />,
  skipped: <SkipForward aria-hidden />,
};

/** Every take in the list by block, with its state in this session; a click goes to it. */
function TakeStrip({ entries, session, current, disabled, onPick }: { entries: PlanEntry[]; session: HearSession; current: string | null; disabled: boolean; onPick: (id: string) => void }) {
  const byId = takesById(session);
  const blocks: { block: HearBlock; items: PlanEntry[] }[] = [];
  for (const e of entries) {
    if (e.first || !blocks.length) blocks.push({ block: e.block, items: [] });
    blocks[blocks.length - 1].items.push(e);
  }
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // Keep the current take in view inside the strip, without moving the page.
    const el = ref.current?.querySelector<HTMLElement>("[aria-current='step']");
    const box = ref.current;
    if (el && box && box.scrollHeight > box.clientHeight) {
      const top = el.offsetTop - box.offsetTop;
      if (top < box.scrollTop || top > box.scrollTop + box.clientHeight - el.offsetHeight) box.scrollTop = top - box.clientHeight / 3;
    }
  }, [current]);
  return (
    <nav className="hear-strip" aria-label="Takes in the list" ref={ref}>
      {blocks.map(({ block, items }) => (
        <div key={block.block} className="hear-strip__block">
          <p className="hear-strip__title t-caption t-2">
            <span className="t-1">{block.block}</span> {block.title || block.condition}
            {block.away && <Footprints aria-label="across the room" />}
            {block.music && <Music aria-label="with music" />}
          </p>
          <ul role="list">
            {items.map((e) => {
              const st = takeStatus(byId.get(e.take.id));
              const here = e.take.id === current;
              return (
                <li key={e.take.id}>
                  <button
                    type="button"
                    className={cx("hear-strip__take", `is-${st}`, here && "is-current")}
                    aria-current={here ? "step" : undefined}
                    disabled={disabled && !here}
                    onClick={() => onPick(e.take.id)}
                    title={`${e.take.id} · ${STATUS_WORDS[st]}`}
                  >
                    <span className="hear-strip__icon">{STRIP_ICON[st]}</span>
                    <span className="t-mono hear-strip__id">{e.take.id}</span>
                    <span className="hear-strip__text" data-user-content>
                      {e.take.kind === "silence" ? (e.take.say ?? "Silent") : (e.take.text ?? "").replace(/\s*\.\.\.\s*/g, " … ")}
                    </span>
                    <span className="sr-only">, {STATUS_WORDS[st]}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </nav>
  );
}
