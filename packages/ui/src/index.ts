/** Shared pieces for the Ghost apps. Everything but ghost.tsx is plain TypeScript, safe for React Native. */
export { useGhost, GhostProvider } from "./ghost";
export type { GhostState } from "./ghost";
export { phaseLabel, autonomyLabel } from "./labels";
export { phaseOf, phaseText, lineName, relativeTime, formatTime, formatDateTime, errorText, quoted, statusLabel, statusTone } from "./format";
export type { GhostPhase, Tone } from "./format";
export { splitSummary, neverHappened, didNotHappen, pastTense, shortTarget } from "./audit";
export type { Summarized } from "./audit";
export { GHOST_PATH, GHOST_PATH_SMALL, lineHue } from "./brand";
