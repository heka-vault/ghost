/**
 * Owner: policy. The dock's state, mapped onto the shared SaveBar: unsaved changes say how
 * many, then Discard and Save; a save that failed keeps the pill (red edge) and says the
 * fault in its own detail when it has a line to point at, with a Notice above only when
 * Ghost refused without naming one. Levels apply the moment the file is written (ADR 0022),
 * so nothing here ever owes a restart.
 */
import type { ReactNode } from "react";

import { Notice, SaveBar as SharedSaveBar } from "../../ui";

export { useCopy } from "../../ui";

export interface BarState {
  kind: "dirty";
  /** “3 unsaved changes”, or “Couldn’t save · line 15”. */
  title: string;
  /** What changed, for the pill’s hover text and Save’s tooltip; the rows carry the badges. */
  summary: string;
  online: boolean;
  busy: boolean;
  /** The fault, said in the pill’s detail when it has a line to point at. */
  error: { text: ReactNode; plain: string; line: number | null } | null;
}

export function SaveBar({ state, onSave, onDiscard }: { state: BarState; onSave: () => void; onDiscard: () => void }) {
  const { title, summary, online, busy, error } = state;
  // The fault with a line takes the detail (the gutter and band point at it); a fault with
  // no line is the one case that needs its own Notice above the pill.
  const pointed = !!error?.line;
  const detail = pointed ? error!.text : !online ? "they wait here until Ghost is back" : undefined;
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
