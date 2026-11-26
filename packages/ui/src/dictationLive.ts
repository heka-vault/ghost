/**
 * A dictation as the daemon reports it (`{"type": "dictation", "dictation": …}`, ADR 0077 as
 * amended 2026-11-25): its scene, the microphone's level about fifteen times a second while it
 * listens, the words read so far, and Ghost's own sentence at the end.
 *
 * The one place dictated words reach a page. They are held here in memory for the view that
 * shows them, replaced by the next event and dropped on a reset: never written anywhere, never
 * logged, never put in React state that outlives the dictation. A `level` of null is "no new
 * reading": the last one stands. A `text` of null is "nothing new to read": the words so far
 * stand through writing and the end, and a new dictation (`starting`) begins with none.
 *
 * Kept out of React state, like the voice level: a view reads `get()` or subscribes. Plain
 * TypeScript, safe for React Native.
 */
import type { DictationLiveView } from "@ghost/client-ts";

export interface DictationLive {
  /** The last view, its level the last real reading; null before any or after a reset. */
  get(): DictationLiveView | null;
  /** The last level (0 to 1) and when it arrived (performance.now()); 0 and -Infinity before any. */
  level(): { level: number; at: number };
  /** A `dictation` event. */
  push(v: DictationLiveView): void;
  /** The connection dropped: forget the dictation and its words. */
  reset(): void;
  /** Called when the scene, the words, the sentence or the key change; not on a level alone. */
  subscribe(fn: () => void): () => void;
  /** Called on every new level reading. */
  subscribeLevel(fn: (level: number) => void): () => void;
}

const clamp = (x: number) => (Number.isFinite(x) ? Math.max(0, Math.min(1, x)) : 0);
const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

export function createDictationLive(): DictationLive {
  let view: DictationLiveView | null = null;
  let level = 0;
  let at = Number.NEGATIVE_INFINITY;
  const subs = new Set<() => void>();
  const levels = new Set<(level: number) => void>();
  return {
    get: () => view,
    level: () => ({ level, at }),
    push(v) {
      const fresh = v.scene === "starting";
      const was = fresh ? null : view;
      if (v.level != null || fresh) {
        level = clamp(v.level ?? 0);
        at = now();
      }
      const text = v.text ?? was?.text ?? null;
      view = { ...v, level: v.level != null ? level : (was?.level ?? null), text };
      if (v.level != null) for (const fn of levels) fn(level);
      const changed = !was || was.scene !== v.scene || was.text !== text || was.sentence !== v.sentence || was.key !== v.key;
      if (changed) for (const fn of subs) fn();
    },
    reset() {
      const had = view !== null;
      view = null;
      level = 0;
      at = Number.NEGATIVE_INFINITY;
      if (had) for (const fn of subs) fn();
    },
    subscribe(fn) {
      subs.add(fn);
      return () => {
        subs.delete(fn);
      };
    },
    subscribeLevel(fn) {
      levels.add(fn);
      return () => {
        levels.delete(fn);
      };
    },
  };
}
