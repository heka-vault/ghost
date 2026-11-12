/**
 * Owner: session. What the live session is doing beyond its turns, gathered for the timeline
 * (`timeline.ts`) and the stage (`Stage.tsx`): how much of Ghost's reply has been said, the
 * changes Ghost made and announced, the stretches on hold, and the questions asked.
 */
import type { QuestionView, Turn } from "@ghost/client-ts";
import { type Activity, createVoicedClock, saidChars, useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

import type { Announced, Held } from "./timeline";

/** How far Ghost got with a reply: characters said, and whether the operator cut in there. */
export interface Saying {
  chars: number;
  cut: boolean;
}

/**
 * How much of each of Ghost's replies in this session was said, by the reply's time
 * (`turn.at`). A reply in progress moves word by word with Ghost's voice (`said.ts`); one the
 * operator cut off keeps the point it reached. A reply missing from the map was said whole
 * (or this window didn't see it being said).
 *
 * Ghost's voice is counted only while one reply is being said: the newest assistant turn, from
 * when it arrives until the voice stops or the operator cuts in.
 */
export function useSaying(turns: readonly Turn[], speaking: boolean, activity: readonly Activity[], speed: number): Record<string, Saying> {
  const { voice } = useGhost();
  const [saying, setSaying] = useState<Record<string, Saying>>({});
  const clock = useRef(createVoicedClock());
  const current = useRef<{ at: string; text: string } | null>(null);
  const speedRef = useRef(speed);
  speedRef.current = speed;
  const reply = lastReply(turns);
  const session = turns[0]?.session_id ?? null;

  // A new session starts with nothing said.
  useEffect(() => {
    setSaying({});
    current.current = null;
  }, [session]);

  // A new reply: the one before it is done, this one starts at nothing.
  useEffect(() => {
    if (!reply || current.current?.at === reply.at) return;
    const was = current.current;
    current.current = { at: reply.at, text: reply.text };
    clock.current.reset();
    setSaying((s) => {
      const next = { ...s };
      if (was && !next[was.at]?.cut) delete next[was.at];
      next[reply.at] = { chars: 0, cut: false };
      return next;
    });
  }, [reply?.at, reply?.text]);

  // Ghost's voice moves it on, word by word: a render only when a word is crossed.
  useEffect(
    () =>
      voice.subscribe((level) => {
        const c = current.current;
        if (!c) return;
        clock.current.level(level);
        const chars = saidChars(c.text, clock.current.secs, speedRef.current, true);
        setSaying((s) => (s[c.at] && !s[c.at].cut && s[c.at].chars !== chars ? { ...s, [c.at]: { chars, cut: false } } : s));
      }),
    [voice],
  );

  // A cut freezes the reply where it got to; the voice stopping on its own finishes it.
  const lastCut = [...activity].reverse().find((a) => a.kind === "cut")?.at ?? null;
  useEffect(() => {
    const c = current.current;
    if (!c || !lastCut || lastCut < c.at) return;
    current.current = null;
    setSaying((s) => ({ ...s, [c.at]: { chars: s[c.at]?.chars ?? 0, cut: true } }));
  }, [lastCut]);
  useEffect(() => {
    if (speaking) return;
    const c = current.current;
    if (!c) return;
    // The voice stopped: said whole, unless a cut already took it.
    current.current = null;
    setSaying((s) => {
      if (s[c.at]?.cut) return s;
      const next = { ...s };
      delete next[c.at];
      return next;
    });
  }, [speaking]);

  return saying;
}

function lastReply(turns: readonly Turn[]): Turn | null {
  for (let i = turns.length - 1; i >= 0; i--) if (turns[i].role === "assistant") return turns[i];
  return null;
}

/**
 * Changes Ghost made in this session without waiting for the operator (ADR 0034: what no rule
 * asks about runs, and Ghost says so), from the gate's audit of the session: the ones that ran
 * at any level but `confirm`, done or failed, at the time they finished. A change that waited
 * for a yes is a decision instead (`useDecisions`). Re-read at every outcome.
 */
export function useAnnounced(sessionId: string | null): Announced[] {
  const { client, lastEvent } = useGhost();
  const [list, setList] = useState<Announced[]>([]);
  // Each outcome event is a new object: the audit is read again after every one.
  const tick = lastEvent?.type === "outcome" ? lastEvent : null;
  useEffect(() => {
    if (!sessionId) {
      setList([]);
      return;
    }
    let live = true;
    client
      .audit({ session_id: sessionId, limit: 50 })
      .then((page) => {
        if (!live) return;
        const done = page.items
          .filter((e) => e.autonomy !== "confirm" && (e.status === "executed" || e.status === "failed"))
          .map((e) => {
            const end = [...e.events].reverse().find((x) => x.event === "finished" || x.event === "failed");
            return { id: e.id, at: end?.at ?? e.created_at, summary: e.summary, ok: e.status === "executed" };
          });
        setList(done);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, sessionId, tick]);
  return list;
}

/** The stretches of this session spent on hold (ADR 0021), from the phase. */
export function useHeld(sessionId: string | null, phase: string): Held[] {
  const [held, setHeld] = useState<Held[]>([]);
  const prev = useRef(phase);
  useEffect(() => setHeld([]), [sessionId]);
  useEffect(() => {
    const was = prev.current;
    prev.current = phase;
    if (!sessionId || was === phase) return;
    const now = new Date().toISOString();
    if (phase === "hold") setHeld((h) => [...h, { at: now, back: null }]);
    else if (was === "hold") setHeld((h) => h.map((x, i) => (i === h.length - 1 && !x.back ? { ...x, back: now } : x)));
  }, [phase, sessionId]);
  return held;
}

/**
 * Questions asked while this session is open (ADR 0051), by any agent, in the order asked:
 * open ones to answer here, settled ones as a record. Re-read when the waiting list changes.
 */
export function useSessionQuestions(since: string | null): QuestionView[] {
  const { client, waitingVersion, connected } = useGhost();
  const [list, setList] = useState<QuestionView[]>([]);
  useEffect(() => {
    if (!since || !connected) {
      setList([]);
      return;
    }
    let live = true;
    client
      .questions()
      .then((v) => {
        if (!live) return;
        setList(v.questions.filter((q) => q.kind === "question" && q.asked_at >= since).sort((a, b) => (a.asked_at < b.asked_at ? -1 : 1)));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, since, waitingVersion, connected]);
  return list;
}
