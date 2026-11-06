/**
 * Owner: hear. Hold to talk (ADR 0042), as a small machine with no React and no DOM, so every
 * case is a test (test/hear.test.ts): what a key or the button going down or up, a repeat, the
 * window losing focus, or Escape asks the daemon to do.
 *
 *   hold   Space or the button down starts the take; up ends it. A press shorter than
 *          MIN_HOLD_MS is thrown away: a tap is never a take.
 *   quiet  across the room: one press starts it; the daemon counts down and ends it on silence.
 *   timed  a silent take: one press starts it; the daemon runs the plan's seconds.
 *
 * Escape cancels whatever take is going. The machine holds only which source is down and since
 * when; everything else it is told (the recorder's state, the selected take), so it never
 * disagrees with the daemon.
 */
import type { HearState, TakeEnding } from "@ghost/client-ts";

export type PressSource = "key" | "pointer";

/** Shorter than this, a held press is a slip, not a take. */
export const MIN_HOLD_MS = 250;

export interface Press {
  /** The source holding a take down, and when it went down (ms). */
  held: { source: PressSource; at: number } | null;
}

export const IDLE: Press = { held: null };

export interface PressContext {
  /** The recorder now. */
  state: HearState;
  /** The take a press would record, and how it ends. */
  take: string | null;
  ending: TakeEnding | null;
  /** A press may start a take: online, open, nothing in flight, no check waiting to be read. */
  canStart: boolean;
}

export type PressInput =
  | { type: "down"; source: PressSource; at: number; repeat?: boolean }
  | { type: "up"; source: PressSource; at: number }
  /** The window lost focus or the page was hidden. */
  | { type: "blur"; at: number }
  | { type: "escape" };

export type PressCall =
  | { call: "start"; take: string; ending: TakeEnding }
  | { call: "stop" }
  /** `short`: the press was a slip; say how to hold. */
  | { call: "cancel"; short?: boolean }
  | null;

const GOING: HearState[] = ["countdown", "recording", "stopping"];

/** What an input does: the machine after it, and the one call to make (or none). */
export function press(m: Press, input: PressInput, ctx: PressContext): { next: Press; call: PressCall } {
  const none = { next: m, call: null };
  switch (input.type) {
    case "down": {
      if (input.repeat || m.held) return none;
      if (!ctx.canStart || ctx.state !== "ready" || !ctx.take || !ctx.ending) return none;
      const call: PressCall = { call: "start", take: ctx.take, ending: ctx.ending };
      return ctx.ending === "hold" ? { next: { held: { source: input.source, at: input.at } }, call } : { next: m, call };
    }
    case "up":
      if (!m.held || m.held.source !== input.source) return none;
      return release(m.held.at, input.at);
    case "blur":
      return m.held ? release(m.held.at, input.at) : none;
    case "escape":
      if (m.held) return { next: IDLE, call: { call: "cancel" } };
      return GOING.includes(ctx.state) ? { next: m, call: { call: "cancel" } } : none;
  }
}

function release(from: number, at: number): { next: Press; call: PressCall } {
  return at - from < MIN_HOLD_MS ? { next: IDLE, call: { call: "cancel", short: true } } : { next: IDLE, call: { call: "stop" } };
}

/** How a take ends: a silent take is timed, across the room it ends on silence, otherwise it is held. */
export function endingFor(kind: string, away: boolean | undefined): TakeEnding {
  if (kind === "silence") return "timed";
  return away ? "quiet" : "hold";
}

export type Shortcut = "next" | "redo" | "play" | "skip";

export interface ShortcutContext {
  state: HearState;
  /** A check is on screen. */
  checked: boolean;
  /** The selected take has a recording to play. */
  recorded: boolean;
}

/** The screen's single keys, between takes only: Enter next, R redo, P play, S skip. */
export function shortcutFor(key: string, ctx: ShortcutContext): Shortcut | null {
  if (ctx.state !== "ready") return null;
  switch (key.toLowerCase()) {
    case "enter":
      return "next";
    case "r":
      return ctx.checked ? "redo" : null;
    case "p":
      return ctx.recorded ? "play" : null;
    case "s":
      return "skip";
    default:
      return null;
  }
}
