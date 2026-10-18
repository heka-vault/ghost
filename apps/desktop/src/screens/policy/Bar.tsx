/**
 * Owner: policy. The dock's state, mapped onto the shared SaveBar: unsaved changes say how
 * many, then Discard and Save; a save that failed keeps the pill (red edge) and says the
 * fault in its own detail when it has a line to point at, with a Notice above only when
 * Ghost refused without naming one. After a save that owes a restart, the same pill carries
 * the command in mono with a Copy button, and Dismiss; the lead says the one thing left to do.
 */
import type { ReactNode } from "react";

import { Notice, SaveBar as SharedSaveBar } from "../../ui";

export { useCopy } from "../../ui";

export const RESTART_CMD = "systemctl --user restart ghostd";

export type BarState =
  | {
      kind: "dirty";
      /** “3 unsaved changes”, or “Couldn’t save · line 15”. */
      title: string;
      /** What changed, for the pill’s hover text and Save’s tooltip; the rows carry the badges. */
      summary: string;
      online: boolean;
      busy: boolean;
      /** The fault, said in the pill’s detail when it has a line to point at. */
      error: { text: ReactNode; plain: string; line: number | null } | null;
      /** A restart from an earlier save is still owed; said in the pill so it isn’t lost. */
      restartOwed: boolean;
    }
  | { kind: "restart" };

export function SaveBar({ state, onSave, onDiscard, onDismiss }: { state: BarState; onSave: () => void; onDiscard: () => void; onDismiss: () => void }) {
  if (state.kind === "restart") {
    return (
      <SharedSaveBar
        variant="restart"
        title="Restart Ghost to apply"
        detail="abilities load on the next start"
        hover="The levels apply now. Which abilities load changes the next time Ghost starts. Run the command, or restart from Settings."
        command={RESTART_CMD}
        onDismiss={onDismiss}
      />
    );
  }
  const { title, summary, online, busy, error, restartOwed } = state;
  // The fault with a line takes the detail (the gutter and band point at it); a fault with
  // no line is the one case that needs its own Notice above the pill.
  const pointed = !!error?.line;
  const detail = pointed ? error!.text : !online ? "they wait here until Ghost is back" : restartOwed ? "restart still needed" : undefined;
  const hover = pointed ? `${title}: ${error!.plain}` : summary || undefined;
  return (
    <SharedSaveBar
      variant={error ? "error" : !online ? "offline" : "dirty"}
      title={title}
      detail={detail}
      hover={hover}
      busy={busy}
      onSave={onSave}
      onDiscard={onDiscard}
      notice={
        error && !pointed ? (
          <Notice tone="danger" className="policy-dock__notice">
            {error.text}
          </Notice>
        ) : undefined
      }
    />
  );
}
