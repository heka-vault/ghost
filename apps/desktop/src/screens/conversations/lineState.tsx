/**
 * Owner: conversations. An agent's state in words, the way the sidebar's agent rows say it:
 * "Waiting on your okay" on the live agent while a change waits (amber, the one state that
 * needs the operator), "Live · listening" on the live agent otherwise, else when it was last
 * used. The sidebar computes the same mapping inline (src/app/Sidebar.tsx); keep the two in
 * step until the foundation exports one helper for both (shared request).
 */
import { type ConnectionState, type GhostPhase, StatusDot, phaseText, relativeTime } from "../../ui";

export type LineTone = "waiting" | "live" | "idle";

export interface LineState {
  tone: LineTone;
  words: string;
}

export function lineState(o: { live: boolean; pending: boolean; connection: ConnectionState; phase: GhostPhase; lastActive: string | null }): LineState {
  if (o.live && o.pending) return { tone: "waiting", words: "Waiting on your okay" };
  if (o.live) return { tone: "live", words: `Live · ${o.connection === "reconnecting" ? "reconnecting" : phaseText(o.phase).toLowerCase()}` };
  return { tone: "idle", words: o.lastActive ? relativeTime(o.lastActive) : "Not used yet" };
}

/** The dot and words for a live agent: amber for a waiting okay, the phase dot otherwise (still: the page status already moves). */
export function LineStateText({ state, connection, phase }: { state: LineState; connection: ConnectionState; phase: GhostPhase }) {
  return (
    <span className={state.tone === "waiting" ? "conv-state is-waiting" : "conv-state"}>
      {state.tone === "waiting" ? (
        <span className="conv-state__dot" aria-hidden />
      ) : (
        <StatusDot phase={connection === "reconnecting" ? "starting" : phase} size="sm" still />
      )}
      {state.words}
    </span>
  );
}
