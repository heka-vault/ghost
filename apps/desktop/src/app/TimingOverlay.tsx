/**
 * Developer's live timing overlay (ADR 0073, amended 2026-11-14): each turn of the live
 * session's stages and their times, small and monospace, under Today's live strip and the
 * conversation screen's stage, and as one compact line on the floating view.
 *
 * Numbers only, from the daemon's `timing` events (the turn's latency record, at its end) and,
 * for the turn under way, the session's `activity` (the operator's turn closed, a call asked
 * for and back). Never a word heard or said. Off by default, switched on Developer, remembered
 * on this computer, and off whenever Developer is hidden (`app/developer.ts`).
 */
import type { TimingView } from "@ghost/client-ts";
import { liveRow, rowOf, STAGES, useGhost, type TimingRow } from "@ghost/ui";
import { useSyncExternalStore } from "react";

import { useTimingOverlay } from "./developer";
import "./timing.css";

/** The floating line's stages. */
const FLOATING_STAGES: readonly string[] = ["first_word", "first_tool", "first_sound", "reply_end"];

/** How many turns the window's overlay shows; Developer's Copy takes ten. */
const SHOWN = 3;

function useTimings(): readonly TimingView[] {
  const { dev } = useGhost();
  return useSyncExternalStore(dev.subscribe, dev.timings, dev.timings);
}

/** The session's rows: its finished turns, newest last, and the one under way. */
export function useTimingRows(limit = SHOWN): TimingRow[] {
  const { status, activity } = useGhost();
  const all = useTimings();
  const session = status?.session_id ?? null;
  if (!session) return [];
  const mine = all.filter((t) => t.session_id === session);
  const rows = mine.slice(-limit).map(rowOf);
  const live = liveRow(activity, mine.length ? mine[mine.length - 1].at : null);
  return live ? [...rows.slice(-(limit - 1)), live] : rows;
}

function Cells({ r }: { r: TimingRow }) {
  return (
    <>
      {STAGES.map(([k, label]) => (
        <td key={k} data-stage={k} title={label} className={r.cells[k] === "–" ? "is-none" : undefined}>
          {r.cells[k]}
        </td>
      ))}
    </>
  );
}

/** The window's overlay: a few rows under the live strip or the stage. Nothing outside a session. */
export function TimingOverlay() {
  const on = useTimingOverlay();
  const rows = useTimingRows();
  if (!on || rows.length === 0) return null;
  return (
    <div className="timing-overlay" data-testid="timing-overlay">
      <table>
        <caption className="sr-only">Turn timings, seconds from when you stopped talking</caption>
        <thead>
          <tr>
            <th scope="col">s</th>
            {STAGES.map(([k, label]) => (
              <th key={k} scope="col">
                {label}
              </th>
            ))}
            <th scope="col" className="timing-overlay__model">
              model
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={r.live ? "live" : `${r.turn}-${i}`} className={r.live ? "is-live" : undefined}>
              <th scope="row">{r.live ? "now" : `#${r.turn}`}</th>
              <Cells r={r} />
              <td className="timing-overlay__model">
                {r.model}
                {r.cut ? " · cut" : ""}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The floating view's line: the latest turn only, never a target for a click or the keyboard. */
export function TimingLine({ className }: { className?: string }) {
  const on = useTimingOverlay();
  const rows = useTimingRows(1);
  const r = rows[rows.length - 1];
  if (!on || !r) return null;
  // The stages that say most in a glance: the rest are in the window's overlay.
  const parts = STAGES.filter(([k]) => FLOATING_STAGES.includes(k) && r.cells[k] !== "–").map(([k, label]) => `${label} ${r.cells[k]}`);
  return (
    <p className={["timing-line", className].filter(Boolean).join(" ")} aria-hidden="true" data-testid="timing-line">
      {r.live ? "now" : `#${r.turn}`} {parts.join(" · ")}
      {r.model ? ` · ${r.model.split(" · ").filter((p, i) => i !== 1).join(" · ")}` : ""}
    </p>
  );
}
