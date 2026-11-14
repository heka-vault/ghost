/**
 * Wisp's mood on the rail and in the room, from the session's phase. Plain TypeScript, so the
 * tests run it without React.
 */
import type { GhostPhase, MarkState } from "@ghost/ui";

export type Mood = "listening" | "thinking" | "speaking" | "starting" | "offline" | "passive" | "hold";

/**
 * `off`: listening is off (ADR 0057). `typed`: a typed session (2026-11-13), which opened no
 * microphone: while it waits for the operator's next line Wisp rests as it does waiting for its
 * name, never as if it were listening to the room.
 */
export function moodOf(phase: GhostPhase, off: boolean, typed = false): Mood {
  if (off) return "hold";
  if (phase === "listening" && typed) return "passive";
  if (phase === "listening" || phase === "thinking" || phase === "speaking" || phase === "starting" || phase === "offline") return phase;
  return "passive";
}

/**
 * The mark's state (docs/design-system.md, The mark): the light's colour and the ring's lit arc
 * on the rail's chip and the tray. `pending`: a change waits for a yes (amber wins over the
 * phase, as the floating room's).
 */
export function markStateOf(phase: GhostPhase, off: boolean, pending = false): MarkState {
  if (off) return "off";
  if (pending) return "waiting";
  if (phase === "listening") return "listening";
  if (phase === "thinking") return "reaching";
  if (phase === "speaking") return "speaking";
  return "idle";
}
