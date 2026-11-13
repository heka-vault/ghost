/**
 * Owner: onboarding. What the wake and ask steps watch for, from Ghost's own phase and turns:
 * a wake heard by voice (never a click), and a question answered (and whether aloud).
 */
import { useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

/** A wake within this long of our own click is the click's. */
const CLICK_WINDOW_MS = 1_500;
/** How long to watch a new session for a typed first turn before calling the wake a voice wake. */
const SETTLE_MS = 700;

export const isAwake = (online: boolean, phase: string) => online && ["listening", "thinking", "speaking"].includes(phase);

/**
 * Watches Ghost's phase while the step is open and says how each wake happened. Only a change
 * from passive to awake seen here counts: a session already running when the step opened is
 * not a wake. A wake right after our own click is a click; a session whose first user turn
 * arrives typed is not a voice wake either.
 */
export function useWakeWatch(awake: boolean, onWake: (how: "voice" | "click", at: Date) => void) {
  const { status, turns } = useGhost();
  const sessionId = status?.session_id ?? null;
  const clickAt = useRef(0);
  const prev = useRef(awake);
  const [alreadyAwake, setAlreadyAwake] = useState(awake);
  const turnsRef = useRef(turns);
  turnsRef.current = turns;
  const report = useRef(onWake);
  report.current = onWake;

  useEffect(() => {
    const was = prev.current;
    prev.current = awake;
    if (was && !awake) setAlreadyAwake(false);
    if (was || !awake) return;
    const at = new Date();
    if (Date.now() - clickAt.current < CLICK_WINDOW_MS) return report.current("click", at);
    const session = sessionId;
    const t = setTimeout(() => {
      const typed = turnsRef.current.some((x) => x.role === "user" && x.via !== "voice" && (!session || x.session_id === session));
      if (!typed) report.current("voice", at);
    }, SETTLE_MS);
    return () => clearTimeout(t);
    // Only the phase edge matters; the session id is read at that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [awake]);

  return { alreadyAwake: alreadyAwake && awake, markClick: () => (clickAt.current = Date.now()) };
}

export interface Exchange {
  question: string;
  answer: string;
  /** The question was spoken, not typed. */
  spoken: boolean;
  /** Ghost's voice was heard while the answer came. */
  aloud: boolean;
}

/**
 * The first question asked after `since` and Ghost's answer to it, from the live turns. Pure, so
 * it is tested: a turn from before the step opened never counts, and an answer is the first
 * assistant turn after the question in the same session.
 */
export function exchangeOf(turns: readonly { role: string; text: string; via: string; session_id: string; at: string }[], since: number): Omit<Exchange, "aloud"> | null {
  const q = turns.find((t) => t.role === "user" && Date.parse(t.at) >= since && t.text.trim());
  if (!q) return null;
  const qi = turns.indexOf(q);
  const a = turns.slice(qi + 1).find((t) => t.role === "assistant" && t.session_id === q.session_id && t.text.trim());
  if (!a) return null;
  return { question: q.text, answer: a.text, spoken: q.via === "voice" };
}
