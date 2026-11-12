/**
 * How much of Ghost's reply has been said, while it is being said.
 *
 * The daemon records a reply when Ghost's voice starts on it (`ghost_voice::tap`), whole, and
 * sends the loudness of the voice as it is heard, a level every 50 ms (`voiceLevel.ts`). A
 * window with any level is 50 ms of Ghost's voice sounding; the pauses between clauses send
 * nothing or 0. So the voiced time so far, at the voice's pace, is about how many characters
 * have been said. It is an estimate, and it is shown as one: it moves word by word, never
 * claims the last word while the voice is still going, and is only exact at the two ends
 * (nothing yet; all of it once the voice stops on its own). A cut (ADR 0055's `cut`) freezes it
 * where it was: the words after that point were never heard.
 *
 * Plain TypeScript, safe for React Native.
 */

/**
 * Characters Ghost says per second of voice at speed 1.0, spaces and punctuation included.
 * Kokoro speaks about 165 words a minute at 1.0, about 14 characters a second over a whole
 * reply; with the pauses between clauses not counted (they send 0), the voiced pace is higher.
 */
export const CHARS_PER_VOICED_SEC = 16;

/** One level window of the daemon's (`ghost_voice::level::WINDOW`). */
export const WINDOW_SECS = 0.05;

/**
 * The characters of `text` said after `voicedSecs` of voice at `speed`, snapped back to the end
 * of the last whole word. While `speaking`, the last word is never claimed (the estimate may
 * run ahead); stopped on its own, all of it was said.
 */
export function saidChars(text: string, voicedSecs: number, speed = 1, speaking = true): number {
  if (!speaking) return text.length;
  const raw = Math.floor(Math.max(0, voicedSecs) * CHARS_PER_VOICED_SEC * (speed > 0 ? speed : 1));
  const lastWordStart = lastWordAt(text);
  const upTo = Math.min(raw, lastWordStart);
  return wordEnd(text, upTo);
}

/** Where the last word of `text` starts (its trailing space excluded). */
function lastWordAt(text: string): number {
  const t = text.replace(/\s+$/, "");
  const i = t.search(/\S+$/);
  return i < 0 ? 0 : i;
}

/** The end of the last word wholly inside `text[0..n]`: n itself at a boundary, else back to one. */
function wordEnd(text: string, n: number): number {
  if (n <= 0) return 0;
  if (n >= text.length) return text.length;
  if (/\s/.test(text[n]) || /\s/.test(text[n - 1])) return text.slice(0, n).replace(/\s+$/, "").length;
  const back = text.slice(0, n).search(/\s\S*$/);
  return back < 0 ? 0 : back;
}

/** The text split where the saying is: what was said, and what wasn't (yet, or ever). */
export function splitSaid(text: string, chars: number): { said: string; rest: string } {
  const n = Math.max(0, Math.min(text.length, chars));
  return { said: text.slice(0, n), rest: text.slice(n) };
}

/**
 * Voiced time from the level stream: each level above 0 is one window of Ghost's voice. Feed it
 * the levels as they arrive; `secs` is the voiced time so far.
 */
export function createVoicedClock() {
  let secs = 0;
  return {
    level(l: number) {
      if (l > 0) secs += WINDOW_SECS;
    },
    get secs() {
      return secs;
    },
    reset() {
      secs = 0;
    },
  };
}

// ---------------------------------------------------------------- the exact clock (ADR 0058)

/**
 * The daemon's TTS stage numbers the words of each reply as it renders them and the output
 * stage says which are heard, in step with the voice (`{"type": "saying"}`): the words `from`
 * up to `to` of reply number `reply` in the session. A word is a run between spaces with a
 * letter or a digit in it, and a line's list or heading marker (`1.`, `-`, `#`) is not one,
 * exactly as `ghost_voice::spoken::word_count` counts what the voice says of the reply.
 */
export interface Span {
  session_id: string;
  reply: number;
  from: number;
  to: number;
}

const MARKER = /^(?:#{1,6}|[-*+]|\d+[.)]|>)$/;

/** Where each counted word of `text` ends, in characters: the n-th word ends at `wordEnds(text)[n - 1]`. */
export function wordEnds(text: string): number[] {
  const out: number[] = [];
  const re = /\S+/g;
  let lineStart = true;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (text.slice(last, m.index).includes("\n")) lineStart = true;
    last = m.index + m[0].length;
    const marker = lineStart && MARKER.test(m[0]);
    lineStart = false;
    if (marker || !/[\p{L}\p{N}]/u.test(m[0])) continue;
    out.push(last);
  }
  return out;
}

/** The characters of `text` that `n` counted words take: 0 for none, all of it past its last word. */
export function charsOfWords(text: string, n: number): number {
  if (n <= 0) return 0;
  const ends = wordEnds(text);
  if (n > ends.length) return text.length;
  return ends[n - 1];
}

/** The latest `saying` span, outside React state like the voice level. */
export interface SaidWords {
  get(): Span | null;
  set(span: Span | null): void;
  subscribe(fn: (span: Span | null) => void): () => void;
}

export function createSaidWords(): SaidWords {
  let span: Span | null = null;
  const listeners = new Set<(span: Span | null) => void>();
  return {
    get: () => span,
    set(next) {
      span = next;
      for (const fn of listeners) fn(span);
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
  };
}
