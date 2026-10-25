/**
 * Owner: session. The session view, `#/`.
 *
 * Before the first turn the page has one focal point and no header to compete with it:
 * Ghost's presence, one line at display size saying what to do, and the composer right under
 * it, sitting at the optical center of the panel (AirOps, Gumloop, Unify). The presence only
 * shows state; waking is the composer's labeled voice control (Mic, “Wake Ghost”), because
 * that is where audio stops being local-only and it must never look like decoration. In a
 * session the same control reads “Hang up” (PhoneOff). The sidebar's status card already says
 * what Ghost is doing, so the page doesn't repeat it. Once there is a transcript it takes over
 * the page in a centered reading column under the line's header, and the composer moves to
 * the Page dock with the live presence at its leading edge; a change waiting for the
 * operator's okay sits directly above it, so its Confirm can never scroll away from its
 * summary. A typed turn goes through the same pipeline, tools and gate as speech. “Ghost
 * isn’t running” is the shell's (src/app/Offline.tsx).
 */
import { useGhost } from "@ghost/ui";
import { ArrowDown, ArrowRight, AudioLines, Mic, PhoneOff, TriangleAlert, X } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { Link } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import { Button, IconButton, LineGlyph, LiveStatus, Notice, Page, PendingChange, VoicePresence, cx, errorText, lineName, quoted, relativeTime, scrollIntoPanel, useLive, useSetupPromptHere, type GhostPhase } from "../../ui";
import { Composer, type VoiceControl } from "./Composer";
import { type OwnSettle, useDecisions, useLastSession, useLines, useObservedSwitches } from "./hooks";
import { Transcript } from "./Transcript";
import { buildTranscript } from "./transcript";
import "./session.css";

/**
 * Overflow below this (the compact bar's height) stays put rather than scrolling. The
 * transcript reserves the same 48px under its newest item (session.css), so it either fits
 * fully above the dock or overflows by enough to scroll; `underFade` is the belt to that
 * brace: nothing may ever read half-erased under the dock.
 */
const MIN_SCROLL_PX = 48;
/** Within this of the end, the transcript counts as followed and new turns keep it there. */
const PINNED_PX = 64;

type Action = "send" | "wake" | "sleep" | "confirm" | "cancel";
const FAILED: Record<Action, string> = {
  send: "Couldn’t send that",
  wake: "Couldn’t wake Ghost",
  sleep: "Couldn’t put Ghost to sleep",
  confirm: "Couldn’t confirm",
  cancel: "Couldn’t cancel",
};

export default function SessionScreen(_: RouteProps) {
  const { client, status, turns, pending } = useGhost();
  // The shell's state: the same words as the sidebar card, and `online` gates every write.
  const { phase, online, loadingModels: loading, setupNeeded, setupHref } = useLive();
  const design = useDesign();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<Action | null>(null);
  const [error, setError] = useState<{ action: Action; text: ReactNode } | null>(() => designError(!!design));
  const composer = useRef<HTMLTextAreaElement>(null);
  const end = useRef<HTMLDivElement>(null);
  const dock = useRef<HTMLDivElement>(null);
  const own = useRef<OwnSettle>(null);

  const sessionId = status?.session_id ?? null;
  const inSession = !!sessionId;
  // The paired phone started this session (ADR 0016). The desk shows it and can still end it.
  const onPhone = inSession && status?.session_origin === "phone";
  const line = (inSession && status?.conversation) || "ghost";
  const lines = useLines();
  const names = useMemo(() => new Map(lines.map((l) => [l.slug, l.name])), [lines]);
  const nameOf = (slug: string) => lineName(slug, names.get(slug));
  const lineLabel = nameOf(line);
  const switches = useObservedSwitches(sessionId, inSession ? line : null, turns);
  const decisions = useDecisions(sessionId, pending, turns, own);
  const items = useMemo(() => buildTranscript(turns, line, lines, switches, decisions), [turns, line, lines, switches, decisions]);
  const seenLast = useLastSession(sessionId, inSession ? line : null, turns.length ? turns[turns.length - 1].at : null);
  // What this window saw end, or else the line used most recently (survives a reload).
  const lastSession = useMemo(() => {
    if (seenLast) return seenLast;
    const used = lines.filter((l) => l.lastActive).sort((a, b) => (b.lastActive! > a.lastActive! ? 1 : -1))[0];
    return used ? { slug: used.slug, at: used.lastActive! } : null;
  }, [seenLast, lines]);

  const hasTranscript = turns.length > 0;
  // Before the first turn the start state is the focal point; when Ghost can't answer yet it
  // says so itself and the shell's setup banner steps aside (one signal per place).
  const needsSetup = online && !loading && setupNeeded && !inSession && !hasTranscript;
  useSetupPromptHere(needsSetup);
  const thinking = inSession && online && phase === "thinking";

  // ---------------------------------------------------------------- following the transcript
  // New turns keep the transcript at its end only while the operator is there (or has just
  // sent something, or a change arrives for their okay). Scrolled up to re-read, it stays put
  // and a “New reply” pill offers the way back.
  const pinned = useRef(true);
  const follow = useRef(false);
  const [unseen, setUnseen] = useState(false);
  const seen = useRef({ turns: turns.length, pending: pending?.id ?? null });

  useEffect(() => {
    const panel = document.getElementById("main");
    if (!panel) return;
    let lastTop = panel.scrollTop;
    const onScroll = () => {
      const near = panel.scrollHeight - panel.scrollTop - panel.clientHeight < PINNED_PX;
      // Scrolling up lets go; content growing or our own scroll down never does.
      pinned.current = near || (pinned.current && panel.scrollTop >= lastTop);
      lastTop = panel.scrollTop;
      if (near) setUnseen(false);
    };
    panel.addEventListener("scroll", onScroll, { passive: true });
    return () => panel.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    const was = seen.current;
    seen.current = { turns: turns.length, pending: pending?.id ?? null };
    if (!hasTranscript) {
      pinned.current = true;
      setUnseen(false);
      return;
    }
    const arrived = !!pending && pending.id !== was.pending;
    if (!pinned.current && !follow.current && !arrived) {
      if (turns.length > was.turns) setUnseen(true);
      return;
    }
    follow.current = false;
    // Not scrollIntoView: in Chrome that moves the Tab starting point to the composer.
    // A few pixels of overflow isn't worth it (any scroll raises the compact bar over the
    // header), unless the newest item has reached the fade above the dock: Ghost's question
    // is the one line the operator must read before pressing Confirm.
    const panel = document.getElementById("main");
    if (panel && panel.scrollHeight - panel.clientHeight < MIN_SCROLL_PX && !underFade(end.current, dock.current)) return;
    pinned.current = true;
    scrollIntoPanel(end.current, { block: "panel-end" });
  }, [hasTranscript, turns.length, thinking, pending, decisions.length]);

  const toLatest = () => {
    pinned.current = true;
    setUnseen(false);
    scrollIntoPanel(end.current, { block: "panel-end" });
  };

  // ---------------------------------------------------------------- actions
  const run = async <T,>(action: Action, f: () => Promise<T>): Promise<{ ok: true; value: T } | { ok: false }> => {
    setBusy(action);
    setError(null);
    try {
      return { ok: true, value: await f() };
    } catch (e) {
      setError({ action, text: errorText(e) });
      return { ok: false };
    } finally {
      setBusy(null);
    }
  };

  const modelsReady = !!status?.models_ready && !loading;
  const canWrite = online && busy === null;
  const canType = online && modelsReady;
  const canWake = canWrite && modelsReady;

  const send = async () => {
    const t = text.trim();
    if (!t || !canType || busy) return;
    setText("");
    follow.current = true; // the operator's own words always bring the transcript to its end
    const r = await run("send", () => client.say(t));
    if (!r.ok) setText((now) => now || t); // give the words back if they didn't go
    composer.current?.focus();
  };

  // A control that goes away after it works hands keyboard focus to the composer, so Tab
  // never restarts from <body>. Only for keyboard use: a mouse click shouldn't light the field.
  // Checked after the re-render (the composer may have remounted between the start state and
  // the dock, and the ref follows whichever is mounted now).
  const keepFocus = () => {
    const keyboard = !!document.activeElement?.matches(":focus-visible");
    return () => {
      if (!keyboard) return;
      const lost = () => !document.activeElement || document.activeElement === document.body;
      if (lost()) composer.current?.focus();
      requestAnimationFrame(() => lost() && composer.current?.focus());
    };
  };

  // Confirm calls client.confirm() and nothing else (the click is confirmation evidence); the
  // card unmounts with it, so focus moves on afterwards.
  const settle = async (action: "confirm" | "cancel") => {
    const refocus = keepFocus();
    if (pending) own.current = { id: pending.id, action };
    const r = action === "confirm" ? await run("confirm", () => client.confirm()) : await run("cancel", () => client.cancel());
    if (r.ok) refocus();
    else own.current = null;
  };

  // Wake and sleep: the voice control may remount with the composer (start state <-> dock);
  // focus comes back to the composer either way.
  const wake = async () => {
    const refocus = keepFocus();
    const r = await run("wake", () => client.wake());
    if (r.ok) refocus();
  };
  const sleep = async () => {
    const refocus = keepFocus();
    const r = await run("sleep", () => client.sleep());
    if (r.ok) refocus();
  };

  const presencePhase: GhostPhase = !online ? "offline" : loading ? "starting" : phase;
  // Reconnecting is the field's own fact to state. Models loading is not: the shell's banner and
  // the status card already say it, so the field keeps its normal placeholder and is simply off.
  const placeholder = !online ? "Reconnecting…" : inSession ? `Type to ${lineLabel}…` : "Type to Ghost…";

  const showDock = hasTranscript || !!pending;
  // The start state owns the page: no header, so nothing outranks its one line. A live
  // session keeps the header (the line, its state, Go to sleep) even before the first turn.
  const startOnly = !hasTranscript && !inSession && !pending;

  // The one voice control. Hang up (PhoneOff) sits in the composer in a session. Wake is where
  // audio stops being local-only, so in the start state it is a labeled button under the
  // composer (a mic inside a composer reads as dictation everywhere else); only in the dock,
  // where space is tight, is it the Session icon alone, with its label as the tooltip.
  const sleepOff = !online || (busy !== null && busy !== "sleep");
  const wakeOff = !canWake && busy !== "wake";
  const wakeWhy = !online ? "Ghost is reconnecting" : !modelsReady ? "Ghost is still loading its voice models" : "Wake Ghost";
  const voice: VoiceControl | undefined = inSession
    ? onPhone
      ? { label: "End the phone session", icon: <PhoneOff />, onClick: sleep, loading: busy === "sleep", disabled: sleepOff }
      : { label: "Hang up", icon: <PhoneOff />, onClick: sleep, loading: busy === "sleep", disabled: sleepOff }
    : showDock
      ? { label: wakeWhy, icon: <AudioLines />, onClick: wake, loading: busy === "wake", disabled: wakeOff }
      : undefined;
  const wakeButton = (
    <Button variant="ghost" leadingIcon={<Mic />} loading={busy === "wake"} disabled={wakeOff} onClick={wake}>
      Wake Ghost
    </Button>
  );

  const composerEl = (
    <Composer
      ref={composer}
      value={text}
      onChange={setText}
      onSubmit={send}
      disabled={!canType}
      sending={busy === "send"}
      placeholder={placeholder}
      presence={hasTranscript ? presencePhase : undefined}
      label={inSession ? `Message ${lineLabel}` : "Message Ghost"}
      voice={voice}
    />
  );

  // A failure is a Notice near the composer; while a change waits, it is one plain line under
  // the composer instead, so the page never stacks two tinted cards and Confirm never moves.
  const errorEl =
    error &&
    (pending ? (
      <p className="session-error-line" role="alert">
        <TriangleAlert aria-hidden />
        <span>
          <span className="session-error-line__title">{FAILED[error.action]}.</span> {error.text}
        </span>
        <IconButton size="sm" variant="ghost" label="Dismiss" icon={<X />} onClick={() => setError(null)} className="session-error-line__dismiss" />
      </p>
    ) : (
      <Notice tone="danger" title={FAILED[error.action]} onDismiss={() => setError(null)} className="session-error">
        {error.text}
      </Notice>
    ));

  const start = startCopy({ online, loading, needsSetup, inSession, onPhone });
  const showLast = startOnly && online && !loading && !needsSetup && lastSession;
  const newest = turns[turns.length - 1];

  return (
    <Page
      width="wide"
      bare={startOnly}
      title={inSession ? lineLabel : "Ghost"}
      leading={<LineGlyph slug={line} name={lineLabel} size="lg" />}
      status={
        onPhone ? (
          <>
            <LiveStatus /> · on your phone
          </>
        ) : (
          <LiveStatus />
        )
      }
      dock={
        showDock ? (
          <div className="session-dock" ref={dock}>
            {/* Polite: “New reply” is said once when it appears, never on every turn. */}
            <div className="session-jump" aria-live="polite">
              {unseen && hasTranscript && (
                <button type="button" className="session-jump__btn" onClick={toLatest}>
                  {newest?.role === "assistant" ? "New reply" : "Latest"}
                  <ArrowDown aria-hidden />
                </button>
              )}
            </div>
            {pending && (
              <PendingChange
                summary={pending.summary}
                rationale={pending.rationale}
                operation={[`${pending.ability}.${pending.operation}`, status?.department ? `in ${lineName(status.department)}` : null, status?.toolkit ?? null].filter(Boolean).join(" · ")}
                expiresAt={pending.expires_at}
                busy={busy === "confirm" ? "confirm" : busy === "cancel" ? "cancel" : busy !== null}
                disabled={!online}
                onConfirm={() => settle("confirm")}
                onCancel={() => settle("cancel")}
              />
            )}
            {!pending && errorEl}
            {composerEl}
            {pending && errorEl}
          </div>
        ) : undefined
      }
    >
      {hasTranscript ? (
        <>
          <Transcript items={items} nameOf={nameOf} thinkingAs={thinking ? line : null} />
          <div ref={end} />
        </>
      ) : (
        <div className={cx("session-start page-fill", !startOnly && "session-start--live")}>
          <div className="session-start__stack">
            {/* Presence only: it shows Ghost's state and is never a button, so a stray click can't open the mic. */}
            <VoicePresence size={96} phase={presencePhase} label="" />
            <h2 className="session-start__title t-display">{start.title}</h2>
            {start.text && <p className="session-start__text">{start.text}</p>}
            {needsSetup ? (
              <div className="session-start__actions">
                <Button variant="primary" href={setupHref}>
                  Finish setup
                </Button>
                {/* Wake drops to secondary: Ghost can listen, but can't answer yet. */}
                {wakeButton}
              </div>
            ) : (
              !showDock && (
                <div className="session-start__composer">
                        {composerEl}
                </div>
              )
            )}
            {/* Below the composer, outside the centering: Wake, the way back, or an error never move the focal point. */}
            {!showDock && !needsSetup && (
              <div className="session-start__below">
                {errorEl}
                {startOnly && (
                  <div className="session-start__row">
                    {wakeButton}
                    {showLast && (
                      <Link to={`/lines/${lastSession.slug}`} className="session-start__last">
                        <span>
                          Last session · {nameOf(lastSession.slug)} · <time dateTime={lastSession.at}>{relativeTime(lastSession.at)}</time>
                        </span>
                        <ArrowRight aria-hidden className="session-start__last-icon" />
                      </Link>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </Page>
  );
}

/** The start state before the first turn: one presence, one line, one hint. Never promises what can’t happen yet. */
function startCopy({ online, loading, needsSetup, inSession, onPhone }: { online: boolean; loading: boolean; needsSetup: boolean; inSession: boolean; onPhone: boolean }): { title: string; text?: string } {
  if (!online) return { title: "Reconnecting", text: "This picks up again as soon as Ghost is back." };
  // The shell's banner and the status card already say the models are loading; this says
  // the next thing instead of repeating it.
  if (loading) return { title: "Almost ready", text: `Say ${quoted("Hey Ghost")} once the models finish` };
  if (needsSetup) return { title: "Finish setup to talk to Ghost", text: "Ghost needs a model key before it can answer." };
  // The header already says “Listening”; this says what to do with it.
  if (onPhone) return { title: "On your phone", text: "Ghost hears you through your phone. You can end the session here." };
  if (inSession) return { title: "Go ahead", text: "Ask what’s waiting on you." };
  // The field under it reads the same in every state; this line carries “or type”.
  return { title: `Say ${quoted("Hey Ghost")}`, text: "or type below" };
}

/**
 * True when the transcript's newest item has reached the fade above the dock (the dock's
 * parent is the sticky host whose top padding is the fade), so it would read half-erased.
 */
function underFade(end: HTMLElement | null, dock: HTMLElement | null): boolean {
  const host = dock?.parentElement;
  const last = end?.previousElementSibling?.lastElementChild;
  if (!host || !last) return false;
  return last.getBoundingClientRect().bottom > host.getBoundingClientRect().top;
}

/**
 * Mock only: `&error=1` opens the screen with a failed action showing, so the error state
 * can be designed and screenshotted. Never active against the real daemon.
 */
function designError(mock: boolean): { action: Action; text: ReactNode } | null {
  if (!mock || !new URLSearchParams(window.location.search).has("error")) return null;
  return {
    action: "send",
    text: (
      <>
        Ghost answered with an error (HTTP 500). Try again; if it keeps happening, <span className="t-mono session-nowrap">journalctl --user -u ghostd</span> shows why.
      </>
    ),
  };
}
