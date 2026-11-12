/**
 * The app's postures (docs/design-system.md, The two postures, and The floating room). At the
 * desk: the briefing, the rail and the page. In the room: Ghost's presence fills the window,
 * legible from three metres, with the few things that need the operator in large type and what
 * Ghost is doing now. Floating (ADR 0059, opt-in): the room without a window, over the desktop.
 *
 * One rule decides it, here and nowhere else, so the main window, the floating surface, the
 * conversation screen's room view and the tests agree:
 *
 *   - A live session at this desk floats, when the operator turned the floating room on and it
 *     can show now (the platform has it, nothing is capturing the screen), unless this window
 *     is the one the operator is using: the windowed app is the place you open on purpose.
 *     A held room in the window still wins.
 *   - The operator's hold wins: “Room” (Ctrl+. or the rail's button) until they leave it with
 *     Escape, the button or the key again; “Desk” never goes to the room by itself.
 *   - A live session at this desk (not the phone's) goes to the room after the same quiet the
 *     conversation screen's room view waits for (20 s without a key, a pointer or a scroll):
 *     one idea, one threshold.
 *   - Outside a session, the room comes after the operator has stepped away (3 minutes by
 *     default; Settings › General sets 1, 3 or 10 minutes, or never).
 *   - Any key, pointer move or scroll brings the desk back at once, unless the room is held.
 *
 * Plain TypeScript: the shell's hook feeds it the clock and the window's input.
 */

export type Posture = "desk" | "room" | "float";
/** The operator's override: `auto` follows the rule above. */
export type PostureHold = "auto" | "desk" | "room";
export type PostureReason = "held" | "session" | "away" | "desk";

/** In a session, the room comes after this long without input (the conversation screen's room view reads the same rule). */
export const SESSION_IDLE_MS = 20_000;
/** Stepped away, outside a session, by default. */
export const AWAY_DEFAULT_MIN = 3;
export const AWAY_CHOICES = [1, 3, 10] as const;

export interface PostureInput {
  hold: PostureHold;
  /** A live session whose audio is at this desk (not the paired phone's). */
  deskSession: boolean;
  /** Since the last key, pointer move or scroll in this window. */
  idleMs: number;
  /** Outside a session, the room after this long away; null: never by itself. */
  awayAfterMs: number | null;
  /** In a session, the room after this long without input; null: never by itself. */
  sessionIdleMs?: number | null;
  /**
   * The floating room is on and can show now: the operator's switch, a platform that has it,
   * and no screen capture or focused full-screen app hiding it. Absent: off.
   */
  floating?: boolean;
  /** This window has the keyboard and is on screen. Absent: it is (the window's own view). */
  windowActive?: boolean;
}

export function postureOf(i: PostureInput): { posture: Posture; reason: PostureReason } {
  if (i.hold === "room") return { posture: "room", reason: "held" };
  if (i.deskSession && i.floating && i.windowActive === false) return { posture: "float", reason: "session" };
  if (i.hold === "desk") return { posture: "desk", reason: "desk" };
  const sessionIdle = i.sessionIdleMs === undefined ? SESSION_IDLE_MS : i.sessionIdleMs;
  if (i.deskSession && sessionIdle != null && i.idleMs >= sessionIdle) return { posture: "room", reason: "session" };
  if (!i.deskSession && i.awayAfterMs != null && i.idleMs >= i.awayAfterMs) return { posture: "room", reason: "away" };
  return { posture: "desk", reason: "desk" };
}

/** When the rule will next change its answer with no input, in ms from now, or null. */
export function nextChangeIn(i: PostureInput): number | null {
  if (i.hold !== "auto") return null;
  if (postureOf(i).posture === "float") return null;
  const sessionIdle = i.sessionIdleMs === undefined ? SESSION_IDLE_MS : i.sessionIdleMs;
  const at = i.deskSession ? sessionIdle : i.awayAfterMs;
  if (at == null) return null;
  return i.idleMs >= at ? null : at - i.idleMs;
}

/**
 * The key that turns the room on and off: Ctrl+. (a period, “a point across the room”). Escape
 * also leaves a held room. Neither fires while typing in a field, except Ctrl+.
 */
export function isRoomKey(e: { key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean }): boolean {
  return (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key === ".";
}

// ---------------------------------------------------------------- the operator's choice (this window)

const AWAY_KEY = "ghost.room.away";
const SESSION_KEY = "ghost.room.session";

/** Minutes away before the room, or null for never. Kept in this window's storage. */
export function readAway(): number | null {
  try {
    const v = localStorage.getItem(AWAY_KEY);
    if (v === "never") return null;
    const n = Number(v);
    return v && (AWAY_CHOICES as readonly number[]).includes(n) ? n : AWAY_DEFAULT_MIN;
  } catch {
    return AWAY_DEFAULT_MIN;
  }
}

export function writeAway(minutes: number | null): void {
  try {
    localStorage.setItem(AWAY_KEY, minutes == null ? "never" : String(minutes));
  } catch {
    /* a private window: the choice lasts until the window closes */
  }
}

/** Whether a spoken session goes to the room by itself. On unless turned off. */
export function readSessionRoom(): boolean {
  try {
    return localStorage.getItem(SESSION_KEY) !== "off";
  } catch {
    return true;
  }
}

export function writeSessionRoom(on: boolean): void {
  try {
    localStorage.setItem(SESSION_KEY, on ? "on" : "off");
  } catch {
    /* as above */
  }
}
