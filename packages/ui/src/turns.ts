/**
 * What the operator said, one message per thing said. The daemon keeps each stretch of
 * speech as its own turn, as it was heard, so a pause mid-sentence makes two ("Tell me a
 * little bit more about", "issue 415"). A run of the operator's spoken turns with nothing
 * between them (no reply, no note, no call) is one thing said, and that is how the model
 * is handed it too: a turn Ghost hadn't answered yet is reopened (`ghost_voice::turns`).
 * The words are kept as heard, joined with a space. Typed turns stay one message each.
 */
export function joinSpoken<T extends { role: string; text: string; via?: string | null }>(turns: readonly T[]): T[] {
  const out: T[] = [];
  for (const t of turns) {
    const last = out[out.length - 1];
    if (last && spoken(last) && spoken(t)) out[out.length - 1] = { ...last, text: `${last.text.trim()} ${t.text.trim()}` };
    else out.push(t);
  }
  return out;
}

/** The operator's words by voice: a stored turn doesn't say how it came, and most are. */
function spoken(t: { role: string; via?: string | null }): boolean {
  return t.role === "user" && t.via !== "text";
}

/**
 * The live session's turns as the screen shows them: `shown`, and `held`, the operator's
 * spoken pieces of a turn that hasn't closed yet. The daemon sends each stretch of speech
 * as a turn the moment it is read, at every short pause, so a sentence said with a pause
 * arrived on screen in pieces while the operator was still talking, and that hurried them.
 * Pieces are held while the operator speaks (the screen shows Ghost listening, nothing that
 * reads as words arriving) and shown together once the turn closes: the daemon's status
 * leaves `listening` (`thinking` when the turn ends, `speaking` when Ghost answers), or a
 * turn that isn't the operator's voice arrives (Ghost's reply, a note, a typed message).
 * A typed turn is never held. The daemon sends no whole-turn text when it reads a turn again
 * (ADR 0044): the pieces as heard are what there is, joined by `joinSpoken`.
 */
export interface Captions<T> {
  shown: T[];
  held: T[];
}

/** How many live turns are kept, as before: the last 200. */
const KEPT = 200;

export function noCaptions<T>(): Captions<T> {
  return { shown: [], held: [] };
}

/** A turn event from the daemon. */
export function captionTurn<T extends { role: string; via?: string | null }>(c: Captions<T>, turn: T): Captions<T> {
  if (turn.role === "user" && turn.via === "voice") return { shown: c.shown, held: [...c.held, turn] };
  return { shown: [...c.shown, ...c.held, turn].slice(-KEPT), held: [] };
}

/** A status event's phase: anything but `listening` closes the operator's turn. */
export function captionPhase<T>(c: Captions<T>, phase: string): Captions<T> {
  if (phase === "passive" || phase === "starting") return noCaptions();
  if (phase === "listening" || c.held.length === 0) return c;
  return { shown: [...c.shown, ...c.held].slice(-KEPT), held: [] };
}

/**
 * A stored transcript (which keeps each piece as it was read) minus the live turn's pieces
 * still held, so a screen that re-reads the store mid-sentence shows what the live view does.
 * Drops up to `held` user turns from its end; anything else ends the trim.
 */
export function withoutHeld<T extends { role: string }>(turns: readonly T[], held: number): T[] {
  let end = turns.length;
  while (held > 0 && end > 0 && turns[end - 1].role === "user") {
    end--;
    held--;
  }
  return turns.slice(0, end);
}
