/**
 * Mock scenarios. Fixtures read these flags rather than scenario names, so a new scenario
 * is one entry here and every owner's fixture adapts. Foundation-owned.
 */
import type { Phase } from "@ghost/client-ts";

export interface Scenario {
  name: string;
  label: string;
  description: string;
  /** false: the daemon is not running (every request fails, the event stream closes). */
  daemon: boolean;
  phase: Phase;
  modelsReady: boolean;
  /** Slug of the conversation a live session is in, or null for no session. */
  session: string | null;
  /** A confirm-level change is waiting. */
  pending: boolean;
  /** Secrets and the model endpoint are configured. */
  configured: boolean;
  /** How much history and configuration exists. */
  data: "empty" | "normal" | "busy";
  /**
   * The daemon answers, then drops after this many ms: the event stream closes and every
   * request fails, so the shell's reconnecting state shows over stale data. Any scenario can
   * drop with ?drop=<ms>; add &back=<ms> to have it come back.
   */
  dropAfterMs?: number;
  /**
   * The waiting change from the fixture expires after this many seconds (audit status
   * "expired", a pending:null event). Without it the fixture's change is held open for
   * design work. Changes proposed while you use the mock always expire after 60s.
   */
  pendingExpiresIn?: number;
  /** Where the live session started: the desk (default) or the paired phone (ADR 0016). */
  origin?: "desk" | "phone";
  /**
   * The phone listener: off (GHOST_PHONE_LISTEN unset), on with no phone, or a phone paired.
   * Without it the settings fixture derives it from `configured` and `data`. Any scenario can
   * override it with ?phone=off|unpaired|paired|pairs|expires (see mock/index.ts).
   */
  phone?: "off" | "unpaired" | "paired";
}

export const SCENARIOS: Scenario[] = [
  { name: "passive", label: "Passive", description: "Idle, waiting for the wake word. Normal history.", daemon: true, phase: "passive", modelsReady: true, session: null, pending: false, configured: true, data: "normal" },
  { name: "session", label: "In a session", description: "A live session on the Accounting line with a multi-turn transcript.", daemon: true, phase: "listening", modelsReady: true, session: "accounting", pending: false, configured: true, data: "normal" },
  { name: "pending", label: "Waiting for okay", description: "Ghost proposed closing an issue and is waiting for a yes.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: true, configured: true, data: "normal" },
  { name: "speaking", label: "Speaking", description: "Mid-session, Ghost is talking.", daemon: true, phase: "speaking", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal" },
  { name: "thinking", label: "Thinking", description: "Mid-session, the model is working on a reply.", daemon: true, phase: "thinking", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal" },
  { name: "starting", label: "Starting", description: "The daemon is up and loading its voice models.", daemon: true, phase: "starting", modelsReady: false, session: null, pending: false, configured: true, data: "normal" },
  { name: "disconnected", label: "Not running", description: "ghostd is not running.", daemon: false, phase: "passive", modelsReady: false, session: null, pending: false, configured: true, data: "normal" },
  { name: "firstrun", label: "First run", description: "Installed, no secrets or endpoint set yet.", daemon: true, phase: "passive", modelsReady: true, session: null, pending: false, configured: false, data: "empty" },
  { name: "empty", label: "Empty", description: "Configured, but nothing has happened yet.", daemon: true, phase: "passive", modelsReady: true, session: null, pending: false, configured: true, data: "empty" },
  { name: "expiring", label: "Okay running out", description: "A confirm-level change that expires 8 seconds after load.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: true, configured: true, data: "normal", pendingExpiresIn: 8 },
  { name: "reconnecting", label: "Reconnecting", description: "A live session; the daemon drops 1.5 seconds after load and stays away.", daemon: true, phase: "listening", modelsReady: true, session: "accounting", pending: false, configured: true, data: "normal", dropAfterMs: 1500 },
  { name: "phone", label: "On the phone", description: "A live session the paired phone started; the desk can end it.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", origin: "phone", phone: "paired" },
  { name: "busy", label: "Busy", description: "Months of use: many lines, 200 audit rows, seven apps, one of them not connecting.", daemon: true, phase: "listening", modelsReady: true, session: "legal", pending: false, configured: true, data: "busy" },
];

export function scenarioByName(name: string): Scenario {
  return SCENARIOS.find((s) => s.name === name) ?? SCENARIOS[0];
}

/** ISO time `minutes` ago (fractions allowed). Fixtures use it so relative times stay fresh. */
export function ago(minutes: number): string {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

/** A stable pseudo-random generator so busy fixtures are identical on every load. */
export function seeded(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

export function hex(n: number, rand: () => number): string {
  let out = "";
  for (let i = 0; i < n; i++) out += Math.floor(rand() * 16).toString(16);
  return out;
}
