/**
 * Owner: session. The stage: what is happening right now in a live session, on the composer.
 * Its sign is Ghost's presence (`Presence.tsx`): Wisp in the
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
 * Ghost big and present over the desktop is the floating room's (ADR 0059); the window keeps
 * this strip and the timeline (the room view was removed on 2026-11-14).
 */
import type { Call } from "@ghost/ui";
import { Mic } from "lucide-react";

import { Button, VoiceWave, cx } from "../../ui";
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

/** The strip's words. `typed`: a typed session (2026-11-13), which opened no microphone. */
export function stageWords(state: StageState, name: string, calls: Call[], micOff?: MicOff | null, typed = false): { title: string; hint?: string } {
  const running = [...new Set(calls.filter((c) => !c.endedAt).map((c) => appTitle(c.app ?? "an app")))];
  switch (state) {
    case "speaking":
      return { title: `${name} is speaking`, hint: typed ? "Type to go on" : "Talk over it to cut in" };
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
      // Typed with no session open: the mic stays closed; Ghost hears only its name.
      if (typed) return { title: `Typing with ${name}`, hint: "Your mic stays closed. Type below, or say “Hey Ghost” to talk." };
      return { title: "Listening", hint: "Go ahead, or type below" };
  }
}

/** Someone is speaking: the strip opens into a band. */
const OPEN: StageState[] = ["speaking", "you"];

/** The strip on the composer. */
export function Stage({ state, slug, name, calls, micOff, typed = false }: { state: StageState; slug: string; name: string; calls: Call[]; micOff?: MicOff | null; typed?: boolean }) {
  const w = stageWords(state, name, calls, micOff, typed);
  const open = OPEN.includes(state);
  const off = state === "listening" && !!micOff;
  return (
    <div className={cx("stage", `stage--${state}`, open && "is-open", off && "is-mic-off")}>
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
    </div>
  );
}
