/** Shared pieces for the Ghost apps. Everything but ghost.tsx is plain TypeScript, safe for React Native, including the mascot (mascot.ts, wisp.ts). */
export { useGhost, GhostProvider } from "./ghost";
export type { GhostState } from "./ghost";
export { phaseLabel, autonomyLabel } from "./labels";
export { phaseOf, phaseText, lineName, relativeTime, formatTime, formatDateTime, errorText, quoted, statusLabel, statusTone } from "./format";
export type { GhostPhase, Tone } from "./format";
export { splitSummary, neverHappened, didNotHappen, pastTense, shortTarget } from "./audit";
export type { Summarized } from "./audit";
export { GHOST_PATH, GHOST_PATH_SMALL, lineHue } from "./brand";
export { createMotion, detailFor, stillPose, LOOK_AT_YOU } from "./mascot";
export type { Attrs as MascotAttrs, Character, Ctx as MascotCtx, Detail as MascotDetail, El as MascotEl, Frame as MascotFrame, Look, Mood as MascotMood, Motion as MascotMotion, Scene as MascotScene } from "./mascot";
export { WISP, wavePose, wispGlyph } from "./wisp";
