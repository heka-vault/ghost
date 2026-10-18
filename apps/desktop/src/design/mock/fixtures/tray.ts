/** Owner: tray (prototype). Tray menu and status popover; read with useFixture("tray"). */
import { ago, type Scenario } from "../scenarios";

export interface TrayFixture {
  /** The last few things Ghost did, newest first. */
  recent: { summary: string; at: string; undoable: boolean }[];
  /** Global shortcut in formatShortcut() notation ("super+g"); render with <Kbd shortcut>. Wakes Ghost from anywhere. */
  shortcut: string;
  /**
   * Microphone muted at load. No API exists for this yet: the tray shows the control only in
   * the prototype, marked as such. `?muted=1` on any scenario overrides it.
   */
  muted: boolean;
  /** What the fake Ubuntu top bar's clock reads, so screenshots don't drift. */
  clock: string;
}

export default function tray(s: Scenario): TrayFixture {
  return {
    recent:
      s.data === "empty"
        ? []
        : [
            { summary: "Approved app 208", at: ago(4), undoable: true },
            { summary: "Labeled app 212 bug, priority high", at: ago(5), undoable: true },
            { summary: "Commented on infra 91", at: ago(60 * 24), undoable: false },
          ],
    shortcut: "super+g",
    muted: false,
    clock: "Sat Oct 18  3:42 PM",
  };
}
