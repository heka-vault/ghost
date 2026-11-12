/**
 * Owner: session. What the live session is doing beyond its turns, gathered for the timeline
 * (`timeline.ts`) and the stage (`Stage.tsx`): how much of Ghost's reply has been said, the
 * changes Ghost made and announced, the stretches on hold, and the questions asked.
 */
import type { QuestionView, Turn } from "@ghost/client-ts";
import { type Activity, charsOfWords, createVoicedClock, mergeActivity, saidChars, type Span, useGhost } from "@ghost/ui";
import { useEffect, useMemo, useRef, useState } from "react";

import type { Announced, Held } from "./timeline";

/**
 * How far Ghost got with a reply. `chars` were said; `now` is the piece being said at this
 * moment (the clause the voice is on, from the daemon's stamp, ADR 0058), up to its end; the
 * rest wasn't said yet, or ever when `cut`. `exact` when the daemon said which words: without
 * its stamps (a cloud voice, an older daemon) the point is the estimate in `said.ts`.
 */
export interface Saying {
  chars: number;
  now?: { end: number } | null;
  cut: boolean;
  exact?: boolean;
}

/**
 * How much of each of Ghost's replies in this session was said, by the reply's time
 * (`turn.at`). A reply in progress moves clause by clause with the daemon's `saying` stamps,
 * or word by word with Ghost's voice when there are none (`said.ts`); one the operator cut off
 * keeps the point it reached. A reply missing from the map was said whole (or this window
 * didn't see it being said).
 *
 * The daemon numbers replies itself; a number is tied to the newest reply on screen the first
 * time it is heard, never to one before a number already tied (a reply with nothing to say
 * aloud gets no stamp, so numbers can skip). A stamp that comes before its reply's turn (a long
 * reply still being written as it is spoken) waits for it.
 */
export function useSaying(turns: readonly Turn[], speaking: boolean, activity: readonly Activity[], speed: number): Record<string, Saying> {
  const { voice, said } = useGhost();
  const [saying, setSaying] = useState<Record<string, Saying>>({});
  const clock = useRef(createVoicedClock());
  /** Voiced time since the piece being said began: where a cut falls inside it. */
  const pieceClock = useRef(createVoicedClock());
  const current = useRef<{ at: string; text: string; reply: number | null; span: { start: number; end: number } | null } | null>(null);
  const tied = useRef(-1);
  const waiting = useRef<Span | null>(null);
  const speedRef = useRef(speed);
  speedRef.current = speed;
  const reply = lastReply(turns);
  const session = turns[0]?.session_id ?? null;

  // A new session starts with nothing said.
  useEffect(() => {
    setSaying({});
    current.current = null;
    tied.current = -1;
    waiting.current = null;
  }, [session]);

  const apply = (span: Span) => {
    const c = current.current;
    if (!c) return;
    if (c.reply == null) {
      if (span.reply <= tied.current) return; // an earlier reply's
      c.reply = span.reply;
      tied.current = span.reply;
    } else if (span.reply !== c.reply) {
      if (span.reply > c.reply) waiting.current = span; // the next reply, whose turn hasn't come
      return;
    }
    const start = charsOfWords(c.text, span.from);
    const end = Math.max(start, charsOfWords(c.text, span.to));
    c.span = { start, end };
    pieceClock.current.reset();
    setSaying((s) => (s[c.at]?.cut ? s : { ...s, [c.at]: { chars: start, now: { end }, cut: false, exact: true } }));
  };

  // A new reply: the one before it is done, this one starts at nothing.
  useEffect(() => {
    if (!reply || current.current?.at === reply.at) {
      if (reply && current.current) current.current.text = reply.text;
      return;
    }
    const was = current.current;
    current.current = { at: reply.at, text: reply.text, reply: null, span: null };
    clock.current.reset();
    setSaying((s) => {
      const next = { ...s };
      if (was && !next[was.at]?.cut) delete next[was.at];
      next[reply.at] = { chars: 0, cut: false };
      return next;
    });
    const w = waiting.current;
    waiting.current = null;
    if (w && w.session_id === session) apply(w);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reply?.at, reply?.text]);

  // The daemon's stamps: which words are heard now.
  useEffect(
    () =>
      said.subscribe((span) => {
        if (span && span.session_id === session) apply(span);
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [said, session],
  );

  // Ghost's voice: the estimate, word by word, until a stamp makes it exact.
  useEffect(
    () =>
      voice.subscribe((level) => {
        const c = current.current;
        if (!c) return;
        clock.current.level(level);
        pieceClock.current.level(level);
        if (c.span) return;
        const chars = saidChars(c.text, clock.current.secs, speedRef.current, true);
        setSaying((s) => (s[c.at] && !s[c.at].cut && s[c.at].chars !== chars ? { ...s, [c.at]: { chars, cut: false } } : s));
      }),
    [voice],
  );

  // A cut freezes the reply where it got to (inside the piece being said, by its voiced time);
  // the voice stopping on its own finishes it.
  const lastCut = [...activity].reverse().find((a) => a.kind === "cut")?.at ?? null;
  useEffect(() => {
    const c = current.current;
    if (!c || !lastCut || lastCut < c.at) return;
    current.current = null;
    setSaying((s) => {
      let chars = s[c.at]?.chars ?? 0;
      if (c.span) {
        const into = saidChars(c.text.slice(c.span.start, c.span.end), pieceClock.current.secs, speedRef.current, true);
        chars = c.span.start + into;
      }
      return { ...s, [c.at]: { chars, cut: true, exact: !!c.span } };
    });
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

/**
 * What the sessions on screen did between their turns, kept by the daemon beside the
 * transcript (`GET /activity`, ADR 0058): the earlier sessions' calls, handovers and cuts, and
 * the live session's from before this page loaded, merged with what arrives live.
 */
export function useSessionActivity(sessions: readonly string[], live: readonly Activity[]): Activity[] {
  const { client, connected } = useGhost();
  const [stored, setStored] = useState<Activity[]>([]);
  const ids = [...new Set(sessions.filter(Boolean))].slice(-40);
  const key = ids.join(",");
  useEffect(() => {
    if (!connected || !ids.length) {
      setStored([]);
      return;
    }
    let on = true;
    client
      .activity(ids)
      .then((list) => on && setStored(list))
      .catch(() => on && setStored([]));
    return () => {
      on = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, connected, key]);
  return useMemo(() => mergeActivity(stored, live), [stored, live]);
}
