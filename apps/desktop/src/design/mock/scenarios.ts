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
   * The phone listener: off (GHOST_PHONE_LISTEN unset), on with no phone, or a phone paired
   * (on the home network only); `tunnel` adds a tunnel address the phone last came in on, and
   * `tunnel-down` one whose interface isn't up yet (ADR 0020). Without it the settings fixture
   * derives it from `configured` and `data`. Any scenario can override it with
   * ?phone=off|unpaired|paired|tunnel|tunnel-down|pairs|expires (see mock/index.ts).
   */
  phone?: "off" | "unpaired" | "paired" | "tunnel" | "tunnel-down";
  /**
   * Not listening (ADR 0057): the microphone is closed, until turned back on (`true`) or for
   * this many minutes from load. Any scenario can set it with ?listening=off|<minutes>.
   */
  notListening?: true | number;
  /**
   * Plays one spoken turn when the event stream opens, as the daemon sends it: three pieces
   * read at the operator's pauses (0.8 s, 2.6 s, 4.2 s), the turn closing (thinking, 5.2 s),
   * Ghost's reply (speaking, 6.1 s), then listening again (7.7 s).
   */
  pieces?: boolean;
  /**
   * A conversation-screen story (fixtures/session.ts): `thread` a session with tool calls, a
   * handover, a change Ghost made, a pause and an agent's question; `you` the operator speaking;
   * `cut` Ghost cut off mid-reply; `hold` the session going on hold; `ending` Ghost saying
   * goodbye; `error` a turn that failed. They play out over the first few seconds after load.
   */
  story?: "thread" | "you" | "cut" | "hold" | "ending" | "error";
}

export const SCENARIOS: Scenario[] = [
  { name: "passive", label: "Passive", description: "Idle, waiting for the wake word. Normal history.", daemon: true, phase: "passive", modelsReady: true, session: null, pending: false, configured: true, data: "normal" },
  { name: "session", label: "In a session", description: "A live session on the Accounting line with a multi-turn transcript.", daemon: true, phase: "listening", modelsReady: true, session: "accounting", pending: false, configured: true, data: "normal" },
  { name: "pending", label: "Waiting for okay", description: "Ghost proposed closing an issue and is waiting for a yes.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: true, configured: true, data: "normal" },
  { name: "speaking", label: "Speaking", description: "Mid-session, Ghost is talking.", daemon: true, phase: "speaking", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal" },
  { name: "thinking", label: "Thinking", description: "Mid-session, the model is working on a reply.", daemon: true, phase: "thinking", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal" },
  { name: "starting", label: "Starting", description: "The daemon is up and loading its voice models.", daemon: true, phase: "starting", modelsReady: false, session: null, pending: false, configured: true, data: "normal" },
  { name: "disconnected", label: "Not running", description: "ghostd is not running.", daemon: false, phase: "passive", modelsReady: false, session: null, pending: false, configured: true, data: "normal" },
  { name: "firstrun", label: "First run", description: "Installed, no passwords or model set yet.", daemon: true, phase: "passive", modelsReady: true, session: null, pending: false, configured: false, data: "empty" },
  { name: "empty", label: "Empty", description: "Configured, but nothing has happened yet.", daemon: true, phase: "passive", modelsReady: true, session: null, pending: false, configured: true, data: "empty" },
  { name: "expiring", label: "Okay running out", description: "A confirm-level change that expires 8 seconds after load.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: true, configured: true, data: "normal", pendingExpiresIn: 8 },
  { name: "reconnecting", label: "Reconnecting", description: "A live session; the daemon drops 1.5 seconds after load and stays away.", daemon: true, phase: "listening", modelsReady: true, session: "accounting", pending: false, configured: true, data: "normal", dropAfterMs: 1500 },
  { name: "phone", label: "On the phone", description: "A live session the paired phone started; the desk can end it.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", origin: "phone", phone: "paired" },
  { name: "thread", label: "A full session", description: "Ghost used apps, handed a turn over, made a change, paused, and an agent asked a question.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", story: "thread" },
  { name: "you", label: "You're speaking", description: "Mid-session, the operator is talking; Ghost hears them.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", story: "you" },
  { name: "cut", label: "Cut off", description: "Ghost starts a long answer; the operator cuts in after two seconds.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", story: "cut" },
  { name: "hold", label: "On hold", description: "The session goes on hold a moment after load (“hang on, Ghost”).", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", story: "hold" },
  { name: "ending", label: "Hanging up", description: "Ghost says goodbye and the session ends.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", story: "ending" },
  { name: "turn-error", label: "A turn failed", description: "The model failed on the last turn.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", story: "error" },
  { name: "notlistening", label: "Not listening", description: "The microphone is closed until the operator turns it back on: no wake, typing still works.", daemon: true, phase: "passive", modelsReady: true, session: null, pending: false, configured: true, data: "normal", notListening: true },
  { name: "pieces", label: "Said in pieces", description: "A live session; one spoken sentence arrives in three pieces with pauses, then Ghost answers.", daemon: true, phase: "listening", modelsReady: true, session: "ghost", pending: false, configured: true, data: "normal", pieces: true },
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
