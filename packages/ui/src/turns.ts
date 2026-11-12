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
 * reads as words arriving) and replaced, once the turn closes, by the words the model was
 * given for it: the daemon reads a turn of several pieces again whole (ADR 0044) and says so
 * with a `heard` event (ADR 0058). A turn taken back before its answer was heard (`heard`
 * with `reopened`) goes back to being held, and the next `heard` has its words too.
 * A typed turn is never held. If a `heard` never comes (an older daemon), the pieces show
 * joined when anything else arrives: Ghost's reply, a note, a typed message.
 */
export interface Captions<T> {
  shown: T[];
  held: T[];
}

/** The daemon's word on a closed turn (`HeardView`). */
export interface Heard {
  text: string;
  at: string;
  reopened?: boolean;
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

/**
 * A `heard` event: the operator's turn closed with these words (they replace its pieces, at
 * the first piece's time), or was taken back (its words wait with the turn that goes on).
 * The pieces are the held ones; a page that loaded mid-turn shows them already, as the last
 * spoken turns after the last thing that wasn't one, and those are replaced instead.
 */
export function captionHeard<T extends { role: string; text: string; via?: string | null }>(c: Captions<T>, heard: Heard): Captions<T> {
  const spoken = (t: T) => t.role === "user" && t.via === "voice";
  if (heard.reopened) {
    const last = c.shown[c.shown.length - 1];
    if (!last || !spoken(last)) return c;
    return { shown: c.shown.slice(0, -1), held: [last, ...c.held] };
  }
  const text = heard.text.trim();
  if (!text) return c;
  if (c.held.length) return { shown: [...c.shown, { ...c.held[0], text }].slice(-KEPT), held: [] };
  let from = c.shown.length;
  while (from > 0 && spoken(c.shown[from - 1])) from--;
  if (from === c.shown.length) return c; // nothing of it is on screen: a typed turn's, or one already replaced
  return { shown: [...c.shown.slice(0, from), { ...c.shown[from], text }], held: [] };
}

/** A status event's phase: a session's end drops everything. The turn's close is `heard`'s. */
export function captionPhase<T>(c: Captions<T>, phase: string): Captions<T> {
  if (phase === "passive" || phase === "starting") return noCaptions();
  return c;
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
