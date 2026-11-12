/**
 * Owner: session. The stage: what is happening right now in a live session, on the composer,
 * and the same moment at room size. Its sign is Ghost's presence (`Presence.tsx`): Wisp in the
 * agent's color on a halo. While someone speaks the strip opens into a band, so the voice is
 * the thing on screen; at rest it is one quiet line.
 *
 *   Ghost speaking     Wisp talks with Ghost's own voice, the halo swells with it, and the
 *                      waveform runs across the band (the daemon's `voice` levels, never the
 *                      microphone)
 *   you speaking       “Listening to you”, Wisp leaning in and rings flowing in toward it: a
 *                      sign someone is heard, never words arriving (the turn's words appear
 *                      once it closes, #73). It comes from the session's activity (the turn
 *                      opened, ADR 0055) and the held pieces, not from the microphone's level.
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
import { Mic, Minimize2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button, IconButton, VoiceWave, cx } from "../../ui";
import type { Saying } from "./live";
import { Presence } from "./Presence";
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

/** The microphone is closed (ADR 0057): what the stage says instead of “go ahead”. */
export interface MicOff {
  /** “Mic off”, or “Mic off until 3:30 PM”. */
  text: string;
  /** Turn listening back on, from here. */
  onListen: () => void;
  busy?: boolean;
}

function words(state: StageState, name: string, calls: Call[], micOff?: MicOff | null): { title: string; hint?: string } {
  const running = [...new Set(calls.filter((c) => !c.endedAt).map((c) => appTitle(c.app ?? "an app")))];
  switch (state) {
    case "speaking":
      return { title: `${name} is speaking`, hint: "Talk over it to cut in" };
    case "you":
      return { title: "Listening to you", hint: "Ghost answers when you finish" };
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
      // A typed turn's session while the mic is closed: Ghost hears nothing, so it can't say “go ahead”.
      if (micOff) return { title: micOff.text, hint: "Type below. Ghost can’t hear you." };
      return { title: "Listening", hint: "Go ahead, or type below" };
  }
}

/** Someone is speaking: the strip opens into a band. */
const OPEN: StageState[] = ["speaking", "you"];

/** The strip on the composer. */
export function Stage({ state, slug, name, calls, room, onRoom, micOff }: { state: StageState; slug: string; name: string; calls: Call[]; room: boolean; onRoom?: () => void; micOff?: MicOff | null }) {
  const w = words(state, name, calls, micOff);
  const open = OPEN.includes(state);
  const off = state === "listening" && !!micOff;
  return (
    <div className={cx("stage", `stage--${state}`, open && "is-open", off && "is-mic-off", room && "is-room")}>
      <span className="stage__sign">
        <Presence state={state} slug={slug} size={open ? 64 : 32} className={off ? "is-mic-off" : undefined} />
      </span>
      <p className="stage__words" role="status">
        <span className="stage__title">{w.title}</span>
        {w.hint && <span className="stage__hint">{w.hint}</span>}
      </p>
      {off ? (
        <span className="stage__action">
          <Button size="sm" variant="secondary" leadingIcon={<Mic />} loading={micOff.busy} onClick={micOff.onListen}>
            Listen again
          </Button>
        </span>
      ) : (
        <span className="stage__wave" aria-hidden>
          <VoiceWave active={state === "speaking"} variant="ribbon" height={open ? 40 : 28} />
        </span>
      )}
      {onRoom && <IconButton size="sm" variant="ghost" label={room ? "Leave room view" : "Room view"} icon={room ? <Minimize2 /> : <RoomIcon />} onClick={onRoom} className="stage__room" />}
    </div>
  );
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

export function RoomView({ state, slug, name, reply, heard, calls, pending, startedAt, onLeave }: RoomViewProps) {
  const w = words(state, name, calls);
  const elapsed = useElapsed(startedAt);
  const leave = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onLeave();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onLeave]);
  const chars = reply?.saying?.chars ?? reply?.text.length ?? 0;
  const nowEnd = Math.max(chars, reply?.saying?.now?.end ?? chars);
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
            <Presence state="you" slug={slug} size={168} />
            <p className="room__label">Listening to you</p>
          </>
        ) : state === "working" ? (
          <>
            <Presence state="working" slug={slug} size={120} />
            {heard && (
              <p className="room__heard" data-user-content>
                {heard}
              </p>
            )}
            <p className="room__label">{w.title}…</p>
          </>
        ) : reply ? (
          <>
            {state === "speaking" && <Presence state="speaking" slug={slug} size={120} />}
            <p className={cx("room__words", reply.saying?.cut && "is-cut")} data-user-content>
              <span className="room__said">{reply.text.slice(0, chars)}</span>
              <span className="room__now">{reply.text.slice(chars, nowEnd)}</span>
              <span className="room__unsaid">{reply.text.slice(nowEnd)}</span>
            </p>
          </>
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

