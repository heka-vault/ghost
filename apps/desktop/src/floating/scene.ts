/**
 * What the floating room shows, as a pure function of what the daemon and the shell said
 * (ADR 0059, docs/design-system.md, The floating room). Plain TypeScript: the page feeds it,
 * the tests drive it.
 *
 * Nothing here reads the microphone: the operator's turn is a sign from the session's activity
 * (`you_started`), never a level and never words while they speak (#73, ADR 0055).
 */
import type { Posture } from "../app/posture";

export type Scene =
  /** Nothing at all, not even the line: the screen is being shared or recorded. */
  | "hidden"
  /** At rest: the line at the bottom edge (or the struck microphone). */
  | "line"
  /** On hold (ADR 0021): the room is talking; Ghost waits at the edge for its name. */
  | "hold"
  /** The operator is speaking. */
  | "you"
  | "listening"
  | "thinking"
  | "speaking"
  /** A change waits for the operator's yes. */
  | "okay"
  /** An agent, a run, an outside tool or an app asked the operator something (ADR 0051). */
  | "question"
  /** Ghost has decided to hang up; its goodbye may still be playing. */
  | "ending";

export type Capture = "yes" | "no" | "unknown";

/**
 * What the floating room does about a screen share now, as the shell decides it
 * (`src-tauri/src/floating/share.rs`, the operator's "While my screen is shared"): no share,
 * kept out of it (the default), shown in it with the shared mark, or asking with a small card.
 */
export type Sharing = "none" | "hidden" | "shown" | "asking";

/** The shell's word, or what a capture meant before there was a choice: hidden. */
export function sharingOf(capture: Capture | null, sharing: Sharing | null | undefined): Sharing {
  if (sharing) return sharing;
  return capture === "yes" ? "hidden" : "none";
}

export interface SceneInput {
  /** `postureOf()` with this surface's inputs: `float` is a session that comes forward here. */
  posture: Posture;
  capture: Capture | null;
  /** The shell's word for a share (absent: a capture hides, as it always did). */
  sharing?: Sharing | null;
  phase: string | null;
  ending: boolean;
  youTalking: boolean;
  pending: boolean;
  question: boolean;
  /** Calls of this turn still running. The phase follows the audio, not the work (below). */
  openCalls?: number;
  /** The goodbye has had its time (`goneAfterGoodbye`): the session is over for this surface. */
  gone?: boolean;
}

/**
 * The phase is the daemon's word for the audio (`ghost_voice::tap`): `speaking` from Ghost's
 * first sound, `listening` again when it stops. So while Ghost says "One sec." over its calls
 * the phase is `speaking`, and after it, `listening` with the calls still running: the work
 * has to come from the calls themselves, or the reach never shows (2026-11-12, live).
 *
 * Ghost's own voice wins over a turn the VAD opened: the daemon interrupts Ghost when the
 * operator's turn really starts (`turns.rs`, `turn_started`), so a `speaking` phase with a
 * turn open is Ghost still at the speaker, and the label must say so.
 */
export function sceneOf(i: SceneInput): Scene {
  // Kept out of a share, or asking about one: nothing of Ghost, not even the line.
  const shared = sharingOf(i.capture, i.sharing);
  if (shared === "hidden" || shared === "asking") return "hidden";
  if (i.posture !== "float") return "line";
  if (i.gone) return "line";
  if (i.ending) return "ending";
  if (i.phase === "hold") return "hold";
  if (i.pending) return "okay";
  if (i.question) return "question";
  if (i.phase === "speaking") return "speaking";
  if (i.youTalking) return "you";
  if ((i.openCalls ?? 0) > 0) return "thinking";
  if (i.phase === "thinking") return "thinking";
  return "listening";
}

/** Seconds with nothing out and no reply before the label names the model as the wait. */
export const SLOW_MODEL_SECS = 4;

/** The goodbye's longest stay: past it the surface leaves even if the voice still says something. */
export const GOODBYE_MS = 4_000;
/** Quiet this long after the goodbye's voice has stopped, and the surface leaves. */
export const GOODBYE_QUIET_MS = 700;

/**
 * The session is ending and the surface should go back to the line now: the goodbye's voice
 * has been quiet a moment, or it has had its time. It must not linger (owner, 2026-11-12).
 */
export function goneAfterGoodbye(o: { endingFor: number | null; quietFor: number; heard: boolean }): boolean {
  if (o.endingFor == null) return false;
  if (o.endingFor >= GOODBYE_MS) return true;
  return o.heard && o.quietFor >= GOODBYE_QUIET_MS;
}

/** Scenes where Ghost has come forward over the desktop (the frame loop runs, Wisp is drawn). */
export function isForward(s: Scene): boolean {
  return s !== "hidden" && s !== "line" && s !== "hold";
}

/**
 * Frames a second the scene needs: the arrival's zoom wants 60; everything else reads at 30,
 * the voice included (syllables come four to eight a second). Every frame is the whole
 * monitor's surface copied through XWayland, so this is the cost (docs/floating-room.md).
 */
export function fpsOf(s: Scene, arriving: boolean): number {
  if (!isForward(s)) return 0;
  return arriving ? 60 : 30;
}

/** The scene's word, in the label under Wisp. A few words: the beams say where. */
export function labelOf(
  s: Scene,
  o: { agent: string; using: string | null; asker: string | null; micOff: boolean; modelSecs?: number | null; typed?: boolean },
): string {
  switch (s) {
    case "you":
      return "Listening to you";
    case "listening":
      if (o.micOff) return "Mic off · type in Ghost";
      // A typed session opened no microphone: it waits for the next line typed.
      return o.typed ? `Typing with ${o.agent}` : "Listening";
    case "thinking":
      if (o.using) return `Looking in ${o.using}`;
      // Nothing out, no reply yet: the model is the wait, and says how long (a slow provider).
      if (o.modelSecs != null && o.modelSecs >= SLOW_MODEL_SECS) return `Waiting on the model · ${Math.floor(o.modelSecs)} s`;
      return "Thinking";
    case "speaking":
      return o.agent;
    case "okay":
      return "Waiting for your yes";
    case "question":
      return o.asker ? `${o.asker} asks` : "A question for you";
    case "ending":
      return "Goodbye";
    case "hold":
      return "On hold · say “Ghost” to pick up";
    default:
      return "";
  }
}

/**
 * Captions (Settings › Captions): Ghost's own words while it speaks, never the operator's. Off
 * by default: Ghost is a voice. "When the sound is off" is the default output muted or under
 * 8% (`wpctl`); an output that can't be read is not taken as off.
 */
export function captionsShown(mode: "off" | "muted" | "always", scene: Scene, sound: { volume: number; muted: boolean } | null): boolean {
  if (scene !== "speaking" && scene !== "ending") return false;
  if (mode === "always") return true;
  if (mode === "muted") return !!sound && (sound.muted || sound.volume < 0.08);
  return false;
}

/**
 * What the two caption lines hold: the reply up to the end of what is being said, its last
 * `max` characters cut at a word, so the newest words are always the ones on screen.
 */
export function captionText(text: string, upto: number | null, max = 120): string {
  const end = upto == null ? text.length : Math.min(text.length, Math.max(0, upto));
  const said = text.slice(0, end).trim();
  if (said.length <= max) return said;
  const tail = said.slice(said.length - max);
  const space = tail.indexOf(" ");
  return `…${space >= 0 ? tail.slice(space + 1) : tail}`;
}
