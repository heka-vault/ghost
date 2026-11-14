/**
 * How Ghost's presence (VoicePresence) shows a voice phase as the mark (docs/design-system.md,
 * The mark): the state's colour, how bright the disc's light is, the ring's lit arc. Plain
 * TypeScript, so it is tested without React Native.
 */
import { ARCS, type GhostPhase, MARK_STATES, type MarkState } from "@ghost/ui";

/** Not listening's light and a dim mark's: the mark's grey (mark.ts). */
export const OFF_LIGHT = "#9aa3b2";

/** The mark's state when it isn't the phase's: `waiting` while a change waits for a yes, `off` when the desk isn't listening. */
export type PresenceMark = "waiting" | "off" | null | undefined;

/** How the mark looks in a phase. A low `glow` is a quiet mark (hold, a desk out of reach). */
export interface Look {
  state: MarkState;
  light: string;
  /** 0..1, how bright the disc's light is. */
  glow: number;
  /** The ring's lit arc, or null for none. */
  arc: [number, number] | null;
  /** The whole ring lit (speaking). */
  full: boolean;
  /** The red stroke of Not listening. */
  strike: boolean;
}

export function lookOf(phase: GhostPhase, mark: PresenceMark, float: boolean): Look {
  const plain = (state: MarkState, glow: number, arc: boolean): Look => ({ state, light: MARK_STATES[state].tint, glow, arc: arc ? ARCS[state] : null, full: false, strike: false });
  if (mark === "off") return { state: "off", light: OFF_LIGHT, glow: 0.3, arc: null, full: false, strike: true };
  if (mark === "waiting") return plain("waiting", 1, true);
  switch (phase) {
    case "listening":
      return plain("listening", 1, true);
    case "thinking":
      return plain("reaching", 1, true);
    case "speaking":
      return { ...plain("speaking", 1, false), full: true };
    case "starting":
      return { ...plain("idle", 0.6, true) };
    case "hold":
      return { ...plain("idle", 0.3, false), light: OFF_LIGHT };
    case "offline":
      return { state: "off", light: OFF_LIGHT, glow: 0.25, arc: null, full: false, strike: false };
    default:
      return plain("idle", float ? 1 : 0.65, float);
  }
}

