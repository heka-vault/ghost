/**
 * Ghost's voice loudness as the daemon reports it (`{"type": "voice", "level"}`, about 20 a
 * second while Ghost speaks, then one 0). Ghost's own output only: never the microphone.
 *
 * Kept out of React state on purpose: twenty context updates a second would re-render every
 * screen. A drawing reads `get()` from its own animation frame, or `subscribe()`s; nothing
 * else re-renders.
 */
export interface VoiceLevel {
  /** The last level (0 to 1) and when it arrived (performance.now()); 0 and -Infinity before any. */
  get(): { level: number; at: number };
  set(level: number): void;
  subscribe(fn: (level: number) => void): () => void;
}

export function createVoiceLevel(): VoiceLevel {
  let level = 0;
  let at = Number.NEGATIVE_INFINITY;
  const listeners = new Set<(level: number) => void>();
  return {
    get: () => ({ level, at }),
    set(next) {
      level = Number.isFinite(next) ? Math.max(0, Math.min(1, next)) : 0;
      at = typeof performance !== "undefined" ? performance.now() : Date.now();
      for (const fn of listeners) fn(level);
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
  };
}
