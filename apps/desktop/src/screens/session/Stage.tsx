/**
 * Owner: session. The stage: what is happening right now in a live session, in one strip that
 * sits on the composer, and the same moment at room size.
 *
 *   Ghost speaking     its name, and a waveform across the strip driven by Ghost's own voice
 *                      (the daemon's `voice` levels, never the microphone)
 *   you speaking       “Listening to you” and a slow breathing ring: a sign someone is heard,
 *                      never words arriving (the turn's words appear once it closes, #73). It
 *                      comes from the session's activity (the turn opened, ADR 0055) and the
 *                      held pieces, not from the microphone's level.
 *   working            what Ghost is using right now (“Using GitHub”), from the live calls
 *   listening          a quiet line saying what to do
 *   on hold, hanging up, on the phone: each says so
 *
 * Room view (`RoomView`) is the same moment for someone across the room: the reply being said
 * at display size, said words bright and the rest dim, the waveform wide. It comes on by itself
 * after a while with nobody at the keyboard in a spoken session and goes as soon as anyone
 * touches the computer; the header's button turns it on and off by hand. It draws on top of
 * the record, which stays in place for screen readers.
 */
import type { PendingProposal } from "@ghost/client-ts";
import type { Call } from "@ghost/ui";
import { Minimize2 } from "lucide-react";
import { type CSSProperties, useEffect, useRef, useState } from "react";

import { IconButton, LineGlyph, VoiceWave, cx } from "../../ui";
import type { Saying } from "./live";
import { appTitle } from "./Transcript";

export type StageState = "speaking" | "you" | "working" | "listening" | "hold" | "ending" | "phone" | "offline";

export function stageState({ online, phase, youTalking, ending, onPhone }: { online: boolean; phase: string; youTalking: boolean; ending: boolean; onPhone: boolean }): StageState {
  if (!online) return "offline";
  if (ending) return "ending";
  if (phase === "hold") return "hold";
  if (phase === "speaking") return "speaking";
  if (youTalking) return "you";
  if (phase === "thinking") return "working";
  if (onPhone) return "phone";
  return "listening";
}

function words(state: StageState, name: string, calls: Call[]): { title: string; hint?: string } {
  const running = [...new Set(calls.filter((c) => !c.endedAt).map((c) => appTitle(c.app ?? "an app")))];
  switch (state) {
    case "speaking":
      return { title: `${name} is speaking`, hint: "Talk over it to cut in" };
    case "you":
      return { title: "Listening to you" };
    case "working":
      return { title: running.length ? `Using ${running.join(" and ")}` : "Working on it" };
    case "hold":
      return { title: "On hold", hint: "Say “Ghost” to pick up again" };
    case "ending":
      return { title: "Saying goodbye" };
    case "phone":
      return { title: "On your phone", hint: "Ghost hears you through the phone" };
    case "offline":
      return { title: "Reconnecting" };
    default:
      return { title: "Listening", hint: "Go ahead, or type below" };
  }
}

/** The strip on the composer. */
export function Stage({ state, slug, name, calls, room, onRoom }: { state: StageState; slug: string; name: string; calls: Call[]; room: boolean; onRoom: () => void }) {
  const w = words(state, name, calls);
  return (
    <div className={cx("stage", `stage--${state}`, room && "is-room")}>
      <span className="stage__sign" aria-hidden>
        {state === "you" ? <Breath /> : state === "working" ? <Orbit /> : <LineGlyph slug={slug} name={name} size="md" />}
      </span>
      <p className="stage__words" role="status">
        <span className="stage__title">{w.title}</span>
        {w.hint && <span className="stage__hint">{w.hint}</span>}
      </p>
      <span className="stage__wave" aria-hidden>
        <VoiceWave active={state === "speaking"} variant="ribbon" height={28} />
      </span>
      <IconButton size="sm" variant="ghost" label={room ? "Leave room view" : "Room view"} icon={room ? <Minimize2 /> : <RoomIcon />} onClick={onRoom} className="stage__room" />
    </div>
  );
}

/** The operator is heard: a slow ring that breathes, the same pace whatever the volume. */
function Breath({ size = 24 }: { size?: number }) {
  return <span className="stage-breath" style={{ "--breath-size": `${size}px` } as CSSProperties} />;
}

/** Ghost is working: a dot circling, quiet. */
function Orbit() {
  return <span className="stage-orbit" />;
}

function RoomIcon() {
  // A screen seen from across the room: a frame with large type in it.
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="18" height="14" rx="2" />
      <path d="M7 9h10M7 13h6M9 21h6" />
    </svg>
  );
}

// ---------------------------------------------------------------- room view

export interface RoomViewProps {
  state: StageState;
  slug: string;
  name: string;
  /** The reply being said, or the last one. */
  reply: { text: string; saying?: Saying } | null;
  /** The operator's last words, once their turn closed. */
  heard: string | null;
  calls: Call[];
  pending: PendingProposal | null;
  startedAt: string | null;
  onLeave: () => void;
}

export function RoomView({ state, name, reply, heard, calls, pending, startedAt, onLeave }: RoomViewProps) {
  const w = words(state, name, calls);
  const elapsed = useElapsed(startedAt);
  const leave = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onLeave();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onLeave]);
  const chars = reply?.saying?.chars ?? reply?.text.length ?? 0;
  return (
    // Visual only: the record under it carries the same words for screen readers.
    <div className={cx("room page-fill", `room--${state}`)} aria-hidden>
      {/* The page's header already names the agent: the room adds how long the session has run. */}
      <div className="room__top">
        {elapsed && <span className="room__time">{elapsed}</span>}
        <button ref={leave} type="button" className="room__leave" tabIndex={-1} onClick={onLeave}>
          Leave room view
        </button>
      </div>
      <div className="room__middle">
        {pending ? (
          <>
            <p className="room__label">Waiting for your okay</p>
            <p className="room__words" data-user-content>
              {pending.summary}
            </p>
            <p className="room__hint">Say yes to go ahead, or no to leave it.</p>
          </>
        ) : state === "you" ? (
          <>
            <Breath size={96} />
            <p className="room__label">Listening to you</p>
          </>
        ) : state === "working" ? (
          <>
            {heard && (
              <p className="room__heard" data-user-content>
                {heard}
              </p>
            )}
            <p className="room__label">{w.title}…</p>
          </>
        ) : reply ? (
          <p className={cx("room__words", reply.saying?.cut && "is-cut")} data-user-content>
            <span className="room__said">{reply.text.slice(0, chars)}</span>
            <span className="room__unsaid">{reply.text.slice(chars)}</span>
          </p>
        ) : (
          <p className="room__label">{w.title}</p>
        )}
      </div>
      <div className="room__bottom">
        <VoiceWave active={state === "speaking"} variant="ribbon" height={72} />
        <p className="room__state">
          {w.title}
          {w.hint && <span> · {w.hint}</span>}
        </p>
      </div>
    </div>
  );
}

function useElapsed(since: string | null): string | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!since) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [since]);
  if (!since) return null;
  const s = Math.max(0, Math.floor((now - new Date(since).getTime()) / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/**
 * Room view: on by hand (the stage's button) until turned off or Escape; on by itself after
 * `idleMs` with nobody touching the computer while `auto` holds (a spoken session), off again at
 * the first pointer move, key or scroll.
 */
export function useRoomView(auto: boolean, idleMs = 20_000): { room: boolean; toggle: () => void; leave: () => void } {
  const [manual, setManual] = useState(false);
  const [idle, setIdle] = useState(false);
  useEffect(() => {
    if (!auto) {
      setIdle(false);
      return;
    }
    let t = window.setTimeout(() => setIdle(true), idleMs);
    const touched = () => {
      setIdle(false);
      window.clearTimeout(t);
      t = window.setTimeout(() => setIdle(true), idleMs);
    };
    const events = ["pointermove", "pointerdown", "keydown", "wheel"] as const;
    for (const e of events) window.addEventListener(e, touched, { passive: true });
    return () => {
      window.clearTimeout(t);
      for (const e of events) window.removeEventListener(e, touched);
    };
  }, [auto, idleMs]);
  return {
    room: manual || (auto && idle),
    toggle: () => {
      setManual((m) => !m);
      setIdle(false);
    },
    leave: () => {
      setManual(false);
      setIdle(false);
    },
  };
}
