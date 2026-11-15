/**
 * The app's two postures (docs/design-system.md, The two postures: the desk and floating). At
 * the desk: the main window as it is, the briefing, the rail and the page, which is also where
 * a live session is seen when Ghost doesn't float. Floating (ADR 0059): Ghost over the desktop,
 * with no window around it. There is no third: the in-window room was removed on 2026-11-14
 * (ADR 0059's amendment of that date).
 *
 * One rule decides it, here and nowhere else, so the main window, the floating surface and the
 * tests agree: a live session at this desk (not the phone's) floats when the operator turned
 * the floating room on and it can show now (the platform has it, nothing it is kept out of is
 * capturing the screen), unless the main window is the one the operator is using: the window is
 * the place you open on purpose. The first run's wake beat is the one exception: its moment
 * floats over the window. Everything else is the desk.
 *
 * Plain TypeScript: the floating surface feeds it the shell's state.
 */

export type Posture = "desk" | "float";

export interface PostureInput {
  /** A live session whose audio is at this desk (not the paired phone's). */
  deskSession: boolean;
  /**
   * The floating room is on and can show now: the operator's switch, a platform that has it,
   * and no screen capture or focused full-screen app hiding it. Absent: off.
   */
  floating?: boolean;
  /** The main window has the keyboard and is on screen. Absent: it is (the window's own view). */
  windowActive?: boolean;
  /**
   * The first run's wake beat is open (ADR 0059 as amended 2026-11-14): the session floats
   * even while the main window has the keyboard, so a newcomer's first “Hey Ghost” brings Ghost
   * over their desktop. `floating` still decides: a share or a desktop that can't float keeps
   * it in the window.
   */
  introducing?: boolean;
}

export function postureOf(i: PostureInput): Posture {
  if (i.deskSession && i.floating && (i.windowActive === false || i.introducing)) return "float";
  return "desk";
}

// ---------------------------------------------------------------- an older install's choices

/**
 * What the removed room kept in the main window's storage: how long away before the room, and
 * whether a spoken session went to it. Nothing reads them now; `forgetRoomChoices` removes them
 * once, quietly.
 */
export const ROOM_KEYS = ["ghost.room.away", "ghost.room.session"] as const;

/** Clear the room's old choices from this window's storage. Never throws (a private window, blocked storage). */
export function forgetRoomChoices(store: Pick<Storage, "removeItem"> | null = safeStorage()): void {
  if (!store) return;
  for (const k of ROOM_KEYS) {
    try {
      store.removeItem(k);
    } catch {
      /* storage refused: nothing reads the key anyway */
    }
  }
}

function safeStorage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}
