/**
 * The live session, built for the car: Ghost's state as one presence and one word, End and
 * Mute low in thumb reach, nothing the operator has to read. Captions are optional and off by
 * default.
 *
 * A question from Ghost, driving: the presence, a “Ghost is asking” overline, the question
 * large, the issue, and how to answer by voice. Parked (or Settings > Always): the presence
 * and the issue sit right above the waiting-change card, which holds the question once and
 * Confirm and Cancel as 56dp buttons in thumb reach; Mute and End step down to 48dp secondary
 * pills under it, still bordered, so End never reads as a label. When the question settles
 * (yes, no, expired) the screen
 * holds the result for a few seconds before it goes back to the session. Reconnecting keeps
 * everything on screen, stale and disabled, with the mic paused and a spinner that says the
 * phone is trying (and, after 20 seconds, for how long). Captions take a reserved band at the
 * bottom of the stage (never an overlay), so the presence and its words keep their room.
 */
import type { OutcomeView, PendingProposal, Turn } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Captions, CaptionsOff, Check, CircleX, Inbox, Mic, MicOff, Minus } from "lucide-react";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";

import { hrefFor, navigate } from "../../app/router";
import { EmptyState, LineGlyph, PendingChange, Spinner, VoicePresence, cx, errorText, lineName, phaseText, quoted, useLive, useToast } from "../../ui";
import { TargetText, pastTense, splitSummary } from "./entries";
import { Sentences, TopBar, TouchButton, TouchIcon, useElapsed, useLink } from "./parts";
import { setPref, update, type PhoneState } from "./store";

/** Only listening gets a hint: the phase word already says the rest. */
const HINT: Partial<Record<string, string>> = {
  listening: "Go ahead.",
  starting: "Getting ready.",
};

/** How long a settled question's result stays on screen before the session comes back. */
const RESULT_MS = 3600;
/** A question that ends without a known outcome waits this long for one before saying “Left it alone”. */
const SETTLE_GRACE_MS = 900;

interface Settled {
  p: PendingProposal;
  /** Settled at, epoch ms. */
  at: number;
  /** The deadline had passed when it settled. */
  expired: boolean;
  /** The operator tapped Cancel on this phone. */
  canceledHere: boolean;
}

export function Session({ phone, mode }: { phone: PhoneState; mode: "session" | "decision" | "reconnecting" }) {
  const { client, status, turns, pending, lastEvent } = useGhost();
  const { phase, online } = useLive();
  const toast = useToast();
  const link = useLink(mode === "reconnecting" ? "reconnecting" : undefined);
  const reconnecting = link === "reconnecting" || link === "offline";
  const [ending, setEnding] = useState(false);
  // Set before End clears startedAt, so the “no session” redirect below never beats the summary.
  const endingRef = useRef(false);
  const [busy, setBusy] = useState<false | "confirm" | "cancel">(false);
  const [starting, setStarting] = useState(false);
  const lostFor = useLostFor(reconnecting);
  const pausedId = useId();
  const noSession = !status?.session_id && !phone.startedAt;

  // #/mobile/decision always means “the question”. With no session there is none: go to the session screen.
  // Not while End is running: the session goes away on purpose then, and the summary is next.
  useEffect(() => {
    if (mode === "decision" && noSession && link === "online" && !endingRef.current) navigate("/mobile/session", { replace: true });
  }, [mode, noSession, link]);

  // ---------------------------------------------------------------- the question's result
  const [outcomes, setOutcomes] = useState<Record<string, OutcomeView>>({});
  const [settled, setSettled] = useState<Settled | null>(null);
  const [tick, setTick] = useState(0);
  const lastPending = useRef<PendingProposal | null>(null);
  const canceledHere = useRef<string | null>(null);

  useEffect(() => {
    if (lastEvent?.type === "outcome") {
      const o = lastEvent.outcome;
      setOutcomes((m) => ({ ...m, [o.proposal_id]: o }));
    }
  }, [lastEvent]);

  useEffect(() => {
    if (pending) {
      lastPending.current = pending;
      setSettled(null);
      return;
    }
    const p = lastPending.current;
    if (!p) return;
    lastPending.current = null;
    setSettled({ p, at: Date.now(), expired: Date.parse(p.expires_at) <= Date.now() + 1000, canceledHere: canceledHere.current === p.id });
  }, [pending]);

  // Re-render once the grace for a late outcome is over, then clear the result and go back.
  useEffect(() => {
    if (!settled) return;
    const g = window.setTimeout(() => setTick((t) => t + 1), SETTLE_GRACE_MS);
    const h = window.setTimeout(() => {
      setSettled(null);
      if (mode === "decision") navigate("/mobile/session", { replace: true });
    }, RESULT_MS);
    return () => {
      window.clearTimeout(g);
      window.clearTimeout(h);
    };
  }, [settled, mode]);

  const outcome = settled ? outcomes[settled.p.id] : undefined;
  const undecided = !!settled && !outcome && !settled.expired && !settled.canceledHere && Date.now() - settled.at < SETTLE_GRACE_MS;
  void tick;

  // ---------------------------------------------------------------- session facts
  const sessionId = status?.session_id ?? null;
  const slug = status?.conversation ?? "ghost";
  const name = lineName(slug);
  const startedAt = phone.startedAt ?? turns[0]?.at ?? null;
  const elapsed = useElapsed(startedAt);
  const captions = phone.prefs.captions;
  const buttons = phone.prefs.decisionButtons === "always" || phone.motion === "parked";
  const canWrite = online && !reconnecting;
  const muted = phone.muted;
  const trying = <TryingAgain lostFor={lostFor} />;

  const end = async () => {
    endingRef.current = true;
    setEnding(true);
    const snapshot = {
      sessionId: sessionId ?? "",
      line: { slug, name },
      startedAt: startedAt ?? new Date().toISOString(),
      endedAt: new Date().toISOString(),
      turns: turns.filter((t) => t.role !== "system"),
    };
    let endFailed = false;
    try {
      if (!reconnecting) await client.sleep();
      else endFailed = true;
    } catch {
      endFailed = true;
    }
    // Summary first, then clear the session: no render ever sees “no session” on the decision route.
    navigate("/mobile/summary");
    update({ startedAt: null, muted: false, ended: { ...snapshot, endFailed } });
  };

  const settle = async (which: "confirm" | "cancel") => {
    setBusy(which);
    if (which === "cancel" && pending) canceledHere.current = pending.id;
    try {
      // The tap is confirmation evidence (ADR 0013): confirm() and nothing else.
      if (which === "confirm") {
        const o = await client.confirm();
        setOutcomes((m) => ({ ...m, [o.proposal_id]: o }));
      } else {
        await client.cancel();
      }
    } catch (e) {
      canceledHere.current = null;
      toast({ title: which === "confirm" ? "Couldn’t confirm" : "Couldn’t cancel", description: errorText(e), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  // The reconnecting route with nothing to reconnect: no session bar, no End, no fake clock. The
  // Home hero for a lost desk, and a way back.
  if (mode === "reconnecting" && noSession && !settled) {
    return (
      <div className="mob-screen mob-session--idle">
        <TopBar back="/mobile/home" />
        <div className="mob-hero mob-hero--fill">
          <VoicePresence phase={reconnecting ? "offline" : "passive"} size={96} label="" />
          <h1 className="mob-hero__title">{reconnecting ? "Lost your desk for a moment" : "Nothing to reconnect"}</h1>
          <p className="mob-hero__body">
            {reconnecting ? "No session was running. Start comes back on Home as soon as the phone reaches your desk again." : "No session was running, and your desk is reachable. Start one from Home."}
          </p>
          {reconnecting && <p className="mob-hero__trying">{trying}</p>}
        </div>
        <div className="mob-cta">
          <TouchButton variant="secondary" size="xl" fullWidth href={hrefFor("/mobile/home")}>Back to Home</TouchButton>
        </div>
      </div>
    );
  }

  // No session: the phone never opens the mic by itself. Home's idle layout, Start in thumb reach.
  if (!sessionId && !reconnecting && !settled) {
    const start = async () => {
      setStarting(true);
      try {
        await client.wake();
        update({ startedAt: new Date().toISOString(), muted: false });
      } catch (e) {
        toast({ title: "Couldn’t start", description: errorText(e), tone: "danger" });
      } finally {
        setStarting(false);
      }
    };
    return (
      <div className="mob-screen mob-session--idle">
        <TopBar back="/mobile/home" />
        <div className="mob-hero mob-hero--fill">
          <VoicePresence phase="passive" size={96} label="" />
          <h1 className="mob-hero__title">No session right now</h1>
          <p className="mob-hero__body">
            <Sentences text="Tap Start and talk. The mic closes when you tap End." />
          </p>
        </div>
        <div className="mob-cta">
          <TouchButton variant="primary" size="xl" fullWidth icon={<Mic size={22} />} loading={starting} disabled={!online} onClick={start}>
            Start session
          </TouchButton>
        </div>
      </div>
    );
  }

  // The question on screen: the live one, or the one that just settled while its outcome is on its way.
  const question = pending ?? (undecided ? settled!.p : null);
  const showResult = !!settled && !pending && !undecided;
  const parkedCard = !!question && buttons;
  const presencePhase = reconnecting ? "offline" : muted ? "passive" : phase;
  const lastTwo = captionTurns(turns);
  const word = reconnecting ? "Reconnecting" : muted ? "Muted" : phaseText(phase);
  const hint = muted ? "Ghost can’t hear you. Tap Unmute to talk." : (HINT[phase] ?? "");
  const showCaptions = captions && !question && !showResult && lastTwo.length > 0;

  return (
    <div className="mob-screen mob-session" data-phase={presencePhase}>
      <header className="mob-session__bar">
        <span className="mob-session__line">
          <LineGlyph slug={slug} name={name} size="lg" />
          <span>
            <h1 className="mob-session__name">
              <span className="sr-only">Session on the </span>
              {slug === "ghost" ? "Ghost" : name}
              <span className="sr-only"> line</span>
            </h1>
            <span className="mob-session__clock t-num" aria-label={`Elapsed ${elapsed}`}>{elapsed}</span>
          </span>
        </span>
        <TouchIcon
          label={captions ? "Hide captions" : "Show captions"}
          pressed={captions}
          icon={captions ? <Captions size={22} /> : <CaptionsOff size={22} />}
          onClick={() => setPref("captions", !captions)}
        />
      </header>

      <div className={cx("mob-session__stage", parkedCard && "is-parked", showCaptions && "has-captions")}>
        {showResult ? (
          <ResultView settled={settled!} outcome={outcome} />
        ) : question ? (
          <>
            <AskStage p={question} phase={presencePhase} parked={parkedCard} stale={reconnecting} muted={muted} trying={trying} pausedId={pausedId} />
            {parkedCard && (
              <PendingChange
                touch
                summary={splitSummary(question).text}
                rationale={question.rationale}
                operation={`${question.source}.${question.operation}`}
                expiresAt={question.expires_at}
                busy={pending ? busy : true}
                disabled={!canWrite}
                onConfirm={() => void settle("confirm")}
                onCancel={() => void settle("cancel")}
              />
            )}
          </>
        ) : mode === "decision" && !reconnecting ? (
          <EmptyState
            className="mob-empty"
            icon={<Inbox />}
            title="Nothing waiting on you"
            description="When Ghost asks before changing something, it says so out loud and the question shows here."
            headingLevel="h2"
            actions={<TouchButton variant="secondary" href={hrefFor("/mobile/session")}>Back to the session</TouchButton>}
          />
        ) : (
          <div className="mob-presence">
            <VoicePresence phase={presencePhase} size={160} label="" />
            <p className="mob-presence__word" role="status">{word}</p>
            {reconnecting ? (
              <>
                <p className="mob-presence__hint">{trying}</p>
                {/* With captions up, the one sentence about the mic lives in their label instead. */}
                {!showCaptions && <p className="mob-presence__sub" id={pausedId}>The mic is paused until your desk is back.</p>}
              </>
            ) : (
              <p className="mob-presence__hint">{hint}</p>
            )}
          </div>
        )}

        {showCaptions && (
          <div className={cx("mob-captions", reconnecting && "is-stale")} aria-label="Captions">
            {reconnecting && <p className="mob-captions__label" id={pausedId}>Last heard before the drop · mic paused until your desk is back</p>}
            {lastTwo.map((t, i) =>
              t.role === "user" ? (
                <div key={`${t.at}-${i}`} className="mob-captions__turn">
                  <p className="mob-captions__label">You</p>
                  <p className="mob-captions__you" data-user-content>{t.text}</p>
                </div>
              ) : (
                <p key={`${t.at}-${i}`} className="mob-captions__ghost" data-user-content>{t.text}</p>
              ),
            )}
          </div>
        )}
      </div>

      <div className={cx("mob-session__actions", parkedCard && "is-asking")}>
        <div className={cx("mob-controls", parkedCard && "mob-controls--quiet")}>
          <MuteButton muted={muted} size={parkedCard ? "md" : "hero"} disabled={reconnecting} describedBy={reconnecting ? pausedId : undefined} onToggle={() => update({ muted: !muted })} />
          <TouchButton variant={parkedCard ? "secondary" : "inverse"} size={parkedCard ? "md" : "hero"} fullWidth loading={ending} onClick={end}>
            End
          </TouchButton>
        </div>
      </div>
    </div>
  );
}

/**
 * The question. Driving: the overline, the question large, the issue, answered by voice.
 * Parked: the waiting-change card below holds the question, its overline and Confirm, so the
 * stage keeps only the presence and the issue; a line of how-to appears only when it adds
 * something (muted, or the desk is away).
 */
function AskStage({ p, phase, parked, stale, muted, trying, pausedId }: { p: PendingProposal; phase: Parameters<typeof VoicePresence>[0]["phase"]; parked: boolean; stale: boolean; muted: boolean; trying: ReactNode; pausedId: string }) {
  const { text, target, conversation } = splitSummary(p);
  const how = stale
    ? "Ghost can’t hear you until your desk is back. The question is still open."
    : muted
      ? `You’re muted. Tap Unmute to say ${quoted("yes")}${parked ? ", or tap Confirm" : ""}.`
      : parked
        ? null
        : `Say ${quoted("yes")} to go ahead, or ${quoted("no")} to leave it.`;
  return (
    <div className={cx("mob-ask", stale && "is-stale", parked && "is-parked")}>
      <VoicePresence phase={phase} size={96} label="" />
      {!parked && <p className="mob-ask__over">Ghost is asking</p>}
      {!parked && <p className="mob-ask__question" data-user-content>{text}</p>}
      <p className="mob-ask__target"><TargetText target={target} conversation={conversation} /></p>
      {stale && <p className="mob-ask__trying">{trying}</p>}
      {how && <p className="mob-ask__say" role="status" id={stale ? pausedId : undefined}>{how}</p>}
    </div>
  );
}

/** “Trying again…” with a spinner; after 20 seconds, how long ago the desk went away. */
function TryingAgain({ lostFor }: { lostFor: number | null }) {
  const long = lostFor != null && lostFor >= 20;
  const ago = lostFor == null ? "" : lostFor < 60 ? `${lostFor} s` : `${Math.floor(lostFor / 60)} min`;
  return (
    <span className="mob-trying">
      <Spinner size={16} />
      <span className="t-num">{long ? `Trying again · lost it ${ago} ago` : "Trying again…"}</span>
    </span>
  );
}

/** Seconds since the phone lost the desk, ticking; null while connected. */
function useLostFor(reconnecting: boolean): number | null {
  const [since, setSince] = useState<number | null>(() => (reconnecting ? Date.now() : null));
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!reconnecting) {
      setSince(null);
      return;
    }
    setSince((s) => s ?? Date.now());
    const h = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(h);
  }, [reconnecting]);
  return reconnecting && since != null ? Math.max(0, Math.floor((now - since) / 1000)) : null;
}

/** What happened to the question, held for a few seconds: the Result state for the one tap that writes. */
function ResultView({ settled, outcome }: { settled: Settled; outcome?: OutcomeView }) {
  const { target, conversation } = splitSummary(settled.p);
  const done = outcome && outcome.status === "success";
  const failed = outcome && outcome.status !== "success";
  const tone = done ? "ok" : failed ? "danger" : "neutral";
  const title = done ? pastTense(settled.p.operation) : failed ? "Didn’t finish" : "Left it alone";
  const body = done
    ? null
    : failed
      ? (outcome.error ?? "Ghost couldn’t finish the change. It’s in Activity with what went wrong.")
      : settled.expired
        ? "No answer in time, so nothing changed."
        : "Nothing changed.";
  return (
    <div className={cx("mob-result", `mob-result--${tone}`)} role="status">
      <span className="mob-result__mark" aria-hidden>
        {done ? <Check size={44} /> : failed ? <CircleX size={44} /> : <Minus size={44} />}
      </span>
      <p className="mob-result__title">{title}</p>
      <p className="mob-result__target"><TargetText target={target} conversation={conversation} /></p>
      {body && <p className="mob-result__body">{body}</p>}
    </div>
  );
}

/**
 * Mute, beside End. Phone-local and a prototype: the phone stops sending its mic, Ghost keeps
 * the session. There is no API for mic routing yet. Off while the desk is unreachable (the mic
 * is already paused then, and the hint it points at says so).
 */
function MuteButton({ muted, size, disabled, describedBy, onToggle }: { muted: boolean; size: "md" | "hero"; disabled?: boolean; describedBy?: string; onToggle: () => void }) {
  const icon = size === "hero" ? 24 : 20;
  return (
    <button
      type="button"
      className={cx("mob-btn", "mob-btn--secondary", `mob-btn--${size}`, "mob-mute", muted && "is-on")}
      aria-pressed={muted}
      disabled={disabled}
      aria-describedby={describedBy}
      onClick={onToggle}
      data-prototype
    >
      <span className="mob-btn__icon" aria-hidden>{muted ? <MicOff size={icon} /> : <Mic size={icon} />}</span>
      <span className="mob-mute__label">{muted ? "Unmute" : "Mute"}</span>
    </button>
  );
}

/** The last thing the operator said and Ghost's reply after it, oldest first. */
function captionTurns(turns: Turn[]): Turn[] {
  const spoken = turns.filter((t) => t.role !== "system" && t.text.trim());
  return spoken.slice(-2);
}
