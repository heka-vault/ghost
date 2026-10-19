/** Owner: session. Data hooks for the session view. */
import type { PendingProposal, Turn } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { type MutableRefObject, useEffect, useRef, useState } from "react";

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

/** What the operator just did with the buttons, so a settled change can say “You confirmed”. */
export type OwnSettle = { id: string; action: "confirm" | "cancel" } | null;

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
): Decision[] {
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
      setDecisions((list) => (list.some((d) => d.key === was.id) ? list : [...list, { key: was.id, afterAt, outcome, byVoice, what, at }]));
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
          e.status === "executed" ? "confirmed" : e.status === "cancelled" ? "canceled" : e.status === "expired" ? "expired" : e.status === "pending" ? guess() : "failed";
        add(outcome, !mine && (outcome === "confirmed" || outcome === "canceled"));
      })
      .catch(() => add(guess(), false));
  }, [client, sessionId, pending, own]);
  // Declared after the effect above, so it runs after it in every commit.
  useEffect(() => {
    committedLastAt.current = lastAt;
  });
  return decisions;
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
