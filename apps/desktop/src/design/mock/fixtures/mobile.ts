/**
 * Owner: mobile (prototype). The phone companion has no API of its own yet: pairing, the
 * phone's preferences and a sample ended session live here. Live state (connection, phase,
 * pending, audit) comes from the mock daemon through useGhost() and useLive().
 * Read with useFixture<MobileFixture>("mobile").
 */
import type { AuditEntry, Turn } from "@ghost/client-ts";

import { ago, type Scenario } from "../scenarios";

export type MobileScreenId =
  | "pair"
  | "pair-code"
  | "paired"
  | "home"
  | "session"
  | "decision"
  | "reconnecting"
  | "summary"
  | "activity"
  | "settings";

export interface MobileScreenInfo {
  id: MobileScreenId;
  title: string;
  /** One line for the stage's index: what this state is for. */
  note: string;
  /** The scenario that shows this screen at its best (the stage links there). */
  scenario: string;
}

/** A designed state, one click away on the stage: the scenario, the route, and any phone query (motion, captions…). */
export interface MobileStateLink {
  label: string;
  scenario: string;
  path: string;
  /** Extra query after ?mock=, e.g. "motion=parked" or "drop=1000". */
  query?: string;
}

export interface MobileFixture {
  /** Every screen under #/mobile/:screen, in the order the operator meets them. */
  screens: MobileScreenInfo[];
  /** Every designed state, grouped for the stage's index. */
  states: { group: string; items: MobileStateLink[] }[];
  /**
   * What the phone saved the last time it reached the desk (a real phone keeps this in app
   * storage), so Home and Activity have something honest to show while the desk is away.
   */
  cached: { savedAt: string; audit: AuditEntry[] };
  pairing: {
    paired: boolean;
    workstation: string;
    /** What the operator types on the phone, shown on the workstation. */
    address: string;
    /** How the phone reaches the desk. */
    via: "Wi-Fi" | "Tailscale";
    pairedAt: string;
    lastSeen: string | null;
  };
  /** The phone's own preferences (local to the phone; no daemon setting behind them). */
  prefs: {
    captions: boolean;
    decisionButtons: "parked" | "always";
    keepAwake: boolean;
    haptics: boolean;
  };
  /** Driving-safe mode: decision buttons show only when the phone says it is parked. */
  motion: "driving" | "parked";
  /** The session shown at #/mobile/summary when this phone hasn't ended one yet. */
  sampleSummary: {
    sessionId: string;
    line: { slug: string; name: string };
    startedAt: string;
    endedAt: string;
    turns: Turn[];
  };
  appVersion: string;
}

const SAMPLE_SESSION = "5f2c9e1a8b7d4c3e9a1f0b6d2e4c8a7f";

function cachedEntry(target: string, operation: string, summary: string, status: AuditEntry["status"], minutesAgo: number): AuditEntry {
  return {
    id: `cached-${target}-${operation}`,
    session_id: SAMPLE_SESSION,
    ability: "github_issues",
    operation,
    target,
    summary,
    autonomy: status === "expired" ? "confirm" : "announce",
    status,
    created_at: ago(minutesAgo),
    events: [{ at: ago(minutesAgo), event: status === "expired" ? "expired" : "finished", detail: status === "expired" ? undefined : { status: "success" } }],
  };
}

export default function mobile(s: Scenario): MobileFixture {
  const t = (role: Turn["role"], text: string, m: number): Turn => ({ role, text, at: ago(m), session_id: SAMPLE_SESSION, via: role === "user" ? "voice" : "system" });
  return {
    screens: [
      { id: "pair", title: "Pair", note: "Scan the code on your desk", scenario: "firstrun" },
      { id: "pair-code", title: "Pair by address", note: "Address and code", scenario: "firstrun" },
      { id: "paired", title: "Paired", note: "What the phone can do", scenario: "passive" },
      { id: "home", title: "Home", note: "Start a session", scenario: "passive" },
      { id: "session", title: "Live session", note: "Driving-safe", scenario: "session" },
      { id: "decision", title: "Decision", note: "Say yes or no", scenario: "pending" },
      { id: "reconnecting", title: "Reconnecting", note: "Lost the desk", scenario: "session" },
      { id: "summary", title: "Session summary", note: "After End", scenario: "passive" },
      { id: "activity", title: "Activity", note: "What Ghost changed", scenario: "passive" },
      { id: "settings", title: "Settings", note: "This phone and your desk", scenario: "passive" },
    ],
    states: [
      {
        group: "Session",
        items: [
          { label: "Listening", scenario: "session", path: "/mobile/session" },
          { label: "Thinking", scenario: "thinking", path: "/mobile/session" },
          { label: "Speaking, captions on", scenario: "speaking", path: "/mobile/session", query: "captions=1" },
          { label: "Muted", scenario: "session", path: "/mobile/session", query: "muted=1" },
          { label: "Drops mid-session", scenario: "reconnecting", path: "/mobile/session", query: "captions=1" },
          { label: "No session", scenario: "passive", path: "/mobile/session" },
        ],
      },
      {
        group: "Ghost asks",
        items: [
          { label: "Driving", scenario: "pending", path: "/mobile/decision" },
          { label: "Parked", scenario: "pending", path: "/mobile/decision", query: "motion=parked" },
          { label: "Running out", scenario: "expiring", path: "/mobile/decision", query: "motion=parked" },
          { label: "Lapses, driving", scenario: "expiring", path: "/mobile/decision" },
          { label: "Desk drops, parked", scenario: "pending", path: "/mobile/decision", query: "motion=parked&drop=1000" },
          { label: "Desk drops, driving", scenario: "pending", path: "/mobile/decision", query: "drop=1000" },
          { label: "Nothing waiting", scenario: "session", path: "/mobile/decision" },
        ],
      },
      {
        group: "Home",
        items: [
          { label: "Ready", scenario: "passive", path: "/mobile/home" },
          { label: "Connecting", scenario: "passive", path: "/mobile/home", query: "connect=1500" },
          { label: "Session at the desk", scenario: "session", path: "/mobile/home" },
          { label: "Models loading", scenario: "starting", path: "/mobile/home" },
          { label: "Setup needed", scenario: "firstrun", path: "/mobile/home", query: "paired=1" },
          { label: "Nothing yet", scenario: "empty", path: "/mobile/home" },
          { label: "Desk not running", scenario: "disconnected", path: "/mobile/home" },
          { label: "Reconnecting", scenario: "passive", path: "/mobile/home", query: "drop=1200" },
        ],
      },
      {
        group: "Pairing and the rest",
        items: [
          { label: "Not paired yet", scenario: "firstrun", path: "/mobile/pair" },
          { label: "Camera blocked", scenario: "firstrun", path: "/mobile/pair", query: "camera=denied" },
          { label: "Code didn’t match", scenario: "firstrun", path: "/mobile/pair-code", query: "pair=fail" },
          { label: "Desk didn’t answer", scenario: "firstrun", path: "/mobile/pair-code", query: "pair=unreachable" },
          { label: "Activity, busy", scenario: "busy", path: "/mobile/activity" },
          { label: "Activity, offline", scenario: "disconnected", path: "/mobile/activity" },
          { label: "Activity, empty", scenario: "empty", path: "/mobile/activity" },
          { label: "End while reconnecting", scenario: "reconnecting", path: "/mobile/reconnecting" },
          { label: "Reconnecting, no session", scenario: "passive", path: "/mobile/reconnecting" },
        ],
      },
    ],
    // Summaries in the gate's own style (past tense, quoted comments), like the shared audit
    // mock; the phone turns a change that never happened back into its proposal on screen.
    cached: {
      savedAt: ago(60 * 3),
      audit: s.data === "empty" ? [] : [
        cachedEntry("ghost-mock/app#208", "approve", "On ghost-mock/app#208: approved, with a comment: “Dana’s fix looks right to me.”", "executed", 60 * 3 + 4),
        cachedEntry("ghost-mock/app#212", "set_labels", "On ghost-mock/app#212: labels: add bug, priority-high.", "executed", 60 * 3 + 6),
        cachedEntry("ghost-mock/app#199", "comment", "On ghost-mock/app#199: comment: “Can you share the stack trace from the failing worker?”", "executed", 60 * 3 + 9),
        cachedEntry("ghost-mock/infra#77", "close", "On ghost-mock/infra#77: closed the issue, with a comment: “Fixed by the runner image bump.”", "expired", 60 * 5),
      ],
    },
    pairing: {
      paired: s.configured,
      workstation: "workstation",
      address: "192.168.1.20:8765",
      via: "Wi-Fi",
      pairedAt: ago(60 * 24 * 12),
      lastSeen: s.daemon ? ago(0.1) : ago(60 * 3),
    },
    prefs: { captions: false, decisionButtons: "parked", keepAwake: true, haptics: true },
    motion: "driving",
    sampleSummary: {
      sessionId: SAMPLE_SESSION,
      line: { slug: "accounting", name: "Accounting" },
      startedAt: ago(6.4),
      endedAt: ago(0.4),
      turns: [
        t("assistant", "Accounting line. Three billing issues are waiting on your sign-off.", 6.0),
        t("user", "Label 212 as a bug and high priority.", 4.9),
        t("assistant", "Done. 212 now has bug and priority high.", 4.8),
        t("user", "Approve 208, Dana’s fix looks right to me.", 4.1),
        t("assistant", "Approved app 208 and left your note that Dana’s fix looks right.", 4.0),
        t("user", "That’s it for now.", 0.6),
        t("assistant", "Talk soon.", 0.5),
      ],
    },
    appVersion: "0.1.0",
  };
}
