/**
 * The summon sequence's states and mock data. SPIKE: nothing here comes from the daemon.
 *
 * In the product each state is something the daemon already says or would say: the wake
 * event (`summon`), the operator's turn opening and closing (ADR 0055 activity), tool calls
 * by app and tool name (activity), Ghost's voice level (`voice`), the reply text, a change
 * waiting for a yes (`pending`), the session ending.
 */

export type Phase = "idle" | "wake" | "listening" | "thinking" | "speaking" | "okay" | "dismiss";

export interface Reach {
  id: string;
  /** What the operator reads: an app or an agent by name, never a tool's raw name. */
  name: string;
  kind: "app" | "agent";
  /** Seconds into the thinking phase when the call starts, and how long it takes. */
  at: number;
  takes: number;
  /** Detail under the name, in mono: the tool, for the curious. */
  detail: string;
}

export const REACH: Reach[] = [
  { id: "gh", name: "GitHub", kind: "app", at: 0.2, takes: 1.1, detail: "issue_read · app#151" },
  { id: "ci", name: "CI", kind: "app", at: 0.5, takes: 0.9, detail: "runs_list · infra" },
  { id: "ledger", name: "Ledger", kind: "agent", at: 0.9, takes: 1.3, detail: "asked" },
  { id: "cal", name: "Calendar", kind: "app", at: 1.3, takes: 0.6, detail: "events_list · today" },
];

export const REPLY =
  "Two things need you. The infra deploy failed at the migration step, and Ledger wants a yes to close app#151 as a duplicate of #148.";

export const OKAY = {
  asker: "Ledger",
  summary: "Close app#151 as a duplicate of #148",
  operation: "github_issue_write",
};

export const AFTER_YES = "Done. It’s closed, with a link to #148.";
export const AFTER_NO = "Left it open.";

/** The demo's timing, seconds per phase. `okay` waits for an answer (or `auto` in the demo). */
export const TIMING = { wake: 0.95, listening: 3.4, thinking: 2.6, okayAuto: 3.2, dismiss: 0.75 };

/** Each phase's light, RGB 0..1: the design system's phase hues, pushed to neon on black. */
export const TINT: Record<Phase, [number, number, number]> = {
  idle: [0.35, 0.85, 1.0],
  wake: [0.45, 0.9, 1.0],
  listening: [0.3, 1.0, 0.72],
  thinking: [0.66, 0.5, 1.0],
  speaking: [0.35, 0.82, 1.0],
  okay: [1.0, 0.7, 0.25],
  dismiss: [0.35, 0.85, 1.0],
};

export const LABEL: Record<Phase, string> = {
  idle: "",
  wake: "Ghost",
  listening: "Listening",
  thinking: "Working",
  speaking: "Ghost",
  okay: "Waiting for your yes",
  dismiss: "",
};
