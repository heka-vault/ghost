/**
 * Owner: tray. One reading of Ghost's state for the tray, the HUD, the native menu and the
 * top-bar icon, so the four never disagree. Titles come from liveLabel(), the mapping the
 * sidebar card and every page status use; the subtitles say what the sidebar card says.
 * Subtitles run to two lines at most; the header keeps room for both, so it never changes
 * height when a state (or the mute switch inside it) changes.
 */
import type { Status } from "@ghost/client-ts";

import { type ConnectionState, type GhostPhase, lineName, liveLabel, quoted } from "../../ui";

/**
 * The middle dot that joins two clauses, with a no-break space after it, so when a line
 * wraps the dot travels with the clause it introduces instead of dangling at the line end.
 */
export const DOT = " ·\u00a0";

/** "Ghost can’t hear “Hey Ghost”" -> "ghost can’t hear “Hey Ghost”": only the first letter, so quoted names keep their case. */
export const lowerFirst = (s: string): string => (s ? s[0].toLowerCase() + s.slice(1) : s);

export interface TrayState {
  /** Drives VoicePresence and the top-bar dot. */
  presence: GhostPhase;
  title: string;
  sub: string;
  /**
   * Ghost is awake: listening, thinking or speaking in a session. Also true while the daemon
   * has dropped mid-session: the last thing we know is that the microphone was open, so the
   * HUD stays (as Reconnecting) rather than claiming Ghost went to sleep.
   */
  inSession: boolean;
  /** inSession, but from stale data: the daemon dropped and we are trying again. */
  stale: boolean;
  muted: boolean;
  /** Line Ghost is on, or null for the main line. */
  line: string | null;
  /**
   * "On the Accounting line" or "On the main line": where Ghost is, worded once for every
   * subtitle (popover, HUD, muted) and the native status line. While reconnecting the
   * popover's subtitle is one clause ("Trying again"); the HUD and native line keep this.
   */
  where: string;
}

export function trayState(opts: {
  connection: ConnectionState;
  phase: GhostPhase;
  status: Status | null;
  muted: boolean;
  setupNeeded: boolean;
  lineNameOf: (slug: string) => string | undefined;
}): TrayState {
  const { connection, phase, status, muted, setupNeeded, lineNameOf } = opts;
  const online = connection === "online";
  const stale = connection === "reconnecting" && !!status?.session_id;
  const inSession = stale || (online && !!status?.session_id && phase !== "passive" && phase !== "starting");
  const slug = status?.conversation ?? null;
  const line = inSession && slug && slug !== "ghost" ? lineName(slug, lineNameOf(slug)) : null;
  // A session the paired phone started (ADR 0016) says so first; the desk can still end it.
  const where = inSession && status?.session_origin === "phone" ? "On your phone" : line ? `On the ${line} line` : "On the main line";
  const { dot, title } = liveLabel(connection, phase);
  const base = { inSession, stale, muted, line, where };

  if (online && muted && phase !== "starting") {
    return {
      ...base,
      presence: "passive",
      title: "Microphone off",
      sub: inSession ? where : `Ghost can’t hear ${quoted("Hey Ghost")}`,
    };
  }
  // Not set up: say what's missing, as the session's start state does.
  if (online && setupNeeded && !inSession && phase !== "starting") {
    return { ...base, presence: dot, title: "Needs a model key", sub: "Ghost can’t answer yet" };
  }
  const sub =
    connection === "connecting"
      ? "Looking for Ghost"
      : connection === "offline"
        ? `Start it to use ${quoted("Hey Ghost")}`
        : connection === "reconnecting"
          ? stale
            ? "Trying again"
            : "Trying again now"
          : phase === "starting"
            ? `Loading voice models${DOT}under a minute`
            : inSession
              ? where
              : "Nothing leaves this computer";
  return { ...base, presence: dot, title, sub };
}

/** The legend's name for what the tray shows right now, so it follows Wake, Sleep and mute. */
export function stateName(s: TrayState, pending: boolean, connection: ConnectionState, setupNeeded: boolean): string {
  if (connection === "offline" || connection === "connecting") return "Not running";
  if (connection === "reconnecting") return "Reconnecting";
  if (s.muted) return "Muted";
  if (pending) return "Waiting for okay";
  if (s.presence === "starting") return "Starting";
  if (s.presence === "thinking") return "Thinking";
  if (s.presence === "speaking") return "Speaking";
  if (s.inSession) return "Listening";
  if (setupNeeded) return "Not set up";
  return "Passive";
}
