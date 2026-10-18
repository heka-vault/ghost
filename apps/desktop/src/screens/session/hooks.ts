/** Owner: session. Data hooks for the session view. */
import type { PendingProposal, Turn } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { type MutableRefObject, useCallback, useEffect, useRef, useState } from "react";

import type { Decision, LineInfo, ObservedSwitch } from "./transcript";

/** Every line's name and aliases, re-read whenever a conversation changes. Empty until it loads. */
export function useLines(): LineInfo[] {
  const { client, conversationsVersion } = useGhost();
  const [lines, setLines] = useState<LineInfo[]>([]);
  useEffect(() => {
    let live = true;
    client
      .conversations()
      .then((list) => live && setLines(list.map((c) => ({ slug: c.slug, name: c.name, aliases: c.aliases, lastActive: c.last_active_at }))))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, conversationsVersion]);
  return lines;
}

/**
 * Line switches seen on the status stream during this session, each placed after the turn
 * that was last when it happened. Cleared when the session changes.
 */
export function useObservedSwitches(sessionId: string | null, line: string | null, turns: Turn[]): ObservedSwitch[] {
  const [switches, setSwitches] = useState<ObservedSwitch[]>([]);
  const prev = useRef<{ session: string | null; line: string | null }>({ session: sessionId, line });
  const lastAt = turns.length ? turns[turns.length - 1].at : null;
  // The last turn as of the previous commit: a switch and the new line's first words can
  // arrive in one batch, and the marker belongs before those words.
  const committedLastAt = useRef(lastAt);

  useEffect(() => {
    const p = prev.current;
    prev.current = { session: sessionId, line };
    if (p.session !== sessionId) {
      setSwitches([]);
      return;
    }
    if (sessionId && p.line && line && p.line !== line) setSwitches((s) => [...s, { afterAt: committedLastAt.current, to: line }]);
  }, [sessionId, line]);
  // Declared after the effect above, so it runs after it in every commit.
  useEffect(() => {
    committedLastAt.current = lastAt;
  });
  return switches;
}

const UNDO_WINDOW_MS = 10 * 60_000;

/**
 * What Undo would reverse, or null when there is nothing: the gate undoes the last executed,
 * not-yet-undone change in this session from the last ten minutes. Re-read after every
 * outcome and turn, and again when the newest one ages out. If the audit can't be read,
 * keep the control (with no summary).
 */
export function useUndoable(sessionId: string | null, outcome: unknown, turnCount: number): { summary: string | null } | null {
  const { client } = useGhost();
  const [last, setLast] = useState<{ until: number; summary: string | null }>({ until: 0, summary: null });
  const [, setTick] = useState(0);

  useEffect(() => {
    if (!sessionId) {
      setLast({ until: 0, summary: null });
      return;
    }
    let live = true;
    client
      .audit({ session_id: sessionId, status: "executed", limit: 10 })
      .then((page) => {
        const e = page.items.find((x) => !x.undo_of);
        if (live) setLast(e ? { until: new Date(e.created_at).getTime() + UNDO_WINDOW_MS, summary: e.summary } : { until: 0, summary: null });
      })
      .catch(() => live && setLast({ until: Infinity, summary: null }));
    return () => {
      live = false;
    };
  }, [client, sessionId, outcome, turnCount]);

  useEffect(() => {
    const left = last.until - Date.now();
    if (left <= 0 || !Number.isFinite(left)) return;
    const t = window.setTimeout(() => setTick((n) => n + 1), left + 50);
    return () => window.clearTimeout(t);
  }, [last.until]);

  return last.until > Date.now() ? { summary: last.summary } : null;
}

/** What the operator just did with the buttons, so a settled change can say “You confirmed”. */
export type OwnSettle = { id: string; action: "confirm" | "cancel" } | null;

/** “On ghost-mock/app#163: close the issue.” reads “On ghost-mock/app#163, close the issue.” */
export function plainSummary(summary: string): string {
  return summary.replace(/^(On [^:]+): /, "$1, ");
}

/**
 * The gate's summary as a phrase that fits after “You confirmed ·”: “On ghost-mock/app#163:
 * close the issue.” reads “close the issue on ghost-mock/app#163”, “On the Standup line:
 * persona set, in your words.” reads “persona set, in your words on the Standup line”.
 * Whatever the ability, the words are the gate's, never the operation id.
 */
export function shortSummary(summary: string): string {
  const s = summary.trim().replace(/\.$/, "");
  const lower = (t: string) => t.charAt(0).toLowerCase() + t.slice(1);
  const m = /^On ([^:]+): ([\s\S]+)$/.exec(s);
  const what = brief(lower(m ? m[2] : s));
  return m ? `${what} on ${m[1]}` : what;
}

/** A caption stays one line: a long payload after a colon (a comment's text) is left to the audit log. */
function brief(what: string, max = 56): string {
  if (what.length <= max) return what;
  const colon = what.indexOf(": ");
  if (colon > 0 && colon <= max) return what.slice(0, colon);
  const cut = what.lastIndexOf(" ", max);
  return `${what.slice(0, cut > 0 ? cut : max)}…`;
}

/**
 * Every change that waited for the operator's okay in this session, once it settles:
 * confirmed (with the buttons or by voice), canceled, expired or failed, read back from the
 * gate's audit record. Placed after the turn that was last when it settled; cleared when the
 * session changes.
 */
export function useDecisions(
  sessionId: string | null,
  pending: PendingProposal | null,
  turns: Turn[],
  own: MutableRefObject<OwnSettle>,
): { decisions: Decision[]; recordUndo: (summary: string, afterAt: string | null) => void } {
  const { client } = useGhost();
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const prev = useRef<{ session: string | null; pending: PendingProposal | null }>({ session: sessionId, pending });
  const lastAt = turns.length ? turns[turns.length - 1].at : null;
  const committedLastAt = useRef(lastAt);

  useEffect(() => {
    const p = prev.current;
    prev.current = { session: sessionId, pending };
    if (p.session !== sessionId) {
      setDecisions([]);
      return;
    }
    const was = p.pending;
    if (!was || !sessionId || pending?.id === was.id) return;
    const afterAt = committedLastAt.current;
    const at = new Date().toISOString();
    const mine = own.current?.id === was.id ? own.current.action : null;
    if (mine) own.current = null;
    const what = shortSummary(was.summary);
    const add = (outcome: Decision["outcome"] | null, byVoice: boolean) => {
      // Not cancelled by a later pending or status change; only a new session drops it.
      if (!outcome || prev.current.session !== sessionId) return;
      setDecisions((list) => (list.some((d) => d.key === was.id) ? list : [...list, { key: was.id, afterAt, outcome, byVoice, what, summary: was.summary, at }]));
    };
    // Without the audit, only what this window saw for itself: its own click, or the clock.
    const guess = (): Decision["outcome"] | null =>
      mine === "confirm" ? "confirmed" : mine === "cancel" ? "canceled" : Date.now() >= new Date(was.expires_at).getTime() - 1000 ? "expired" : null;
    client
      .audit({ session_id: sessionId, limit: 20 })
      .then((page) => {
        const e = page.items.find((x) => x.id === was.id);
        if (!e) return add(guess(), false);
        const outcome: Decision["outcome"] | null =
          e.status === "executed" || e.status === "undone" ? "confirmed" : e.status === "cancelled" ? "canceled" : e.status === "expired" ? "expired" : e.status === "pending" ? guess() : "failed";
        add(outcome, !mine && (outcome === "confirmed" || outcome === "canceled"));
      })
      .catch(() => add(guess(), false));
  }, [client, sessionId, pending, own]);
  // Declared after the effect above, so it runs after it in every commit.
  useEffect(() => {
    committedLastAt.current = lastAt;
  });

  /**
   * The operator undid a change from this window. The change it reversed, if its okay is in
   * the transcript, now reads “You confirmed, then undid it”; otherwise (a change made by voice
   * or at a level that didn't ask) the undo gets its own caption after the turn that was last
   * when the operator pressed Undo, before Ghost's “Undone.”
   */
  const recordUndo = useCallback((summary: string, afterAt: string | null) => {
    setDecisions((list) => {
      const i = list.map((d) => d.outcome === "confirmed" && !d.undone && d.summary === summary).lastIndexOf(true);
      if (i >= 0) return list.map((d, j) => (j === i ? { ...d, undone: true } : d));
      const at = new Date().toISOString();
      return [...list, { key: `undo-${at}`, outcome: "undone", byVoice: false, what: shortSummary(summary), summary, at, afterAt }];
    });
  }, []);
  return { decisions, recordUndo };
}

/** The last session this window saw end, kept for the page's life so sleeping never strands what you were reading. */
let remembered: { slug: string; at: string } | null = null;

export function useLastSession(sessionId: string | null, line: string | null, lastAt: string | null): { slug: string; at: string } | null {
  const [last, setLast] = useState(remembered);
  const prev = useRef({ sessionId, line, lastAt });
  useEffect(() => {
    const p = prev.current;
    prev.current = { sessionId, line, lastAt };
    if (p.sessionId && p.sessionId !== sessionId && p.line) {
      remembered = { slug: p.line, at: p.lastAt ?? new Date().toISOString() };
      setLast(remembered);
    }
  }, [sessionId, line, lastAt]);
  return last;
}
