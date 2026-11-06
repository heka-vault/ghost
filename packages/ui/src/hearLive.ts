/**
 * The Hearing test's recorder as the daemon reports it (`{"type": "hear", hear, level,
 * speaking}`, about 20 a second while it is open, then one `closed`; ADR 0042). The level is
 * the operator's mic, and only while the operator has the recorder open: never passive audio.
 *
 * Kept out of React state on purpose, like the voice level: twenty context updates a second
 * would re-render every screen. A meter reads `level()` from its own animation frame; the
 * screen subscribes to `view()`, which is replaced only when something other than the take's
 * running time changes (a state, a countdown second, a run's progress), so it re-renders a
 * few times a take, not twenty times a second.
 */
import type { HearView } from "@ghost/client-ts";

export interface HearLevel {
  /** The mic's level (0 to 1), whether the VAD hears speech, seconds into the take, and when it arrived (performance.now()). */
  level: number;
  speaking: boolean;
  elapsed: number;
  at: number;
}

export interface HearLive {
  level(): HearLevel;
  /** The recorder's last view, or null before any. */
  view(): HearView | null;
  /** A `hear` event. */
  set(view: HearView, level: number, speaking: boolean): void;
  /** A view the screen read itself (GET /hear, POST /hear/open): no level with it. */
  setView(view: HearView): void;
  /** The stream dropped: the level falls to nothing; the view stays as it was. */
  reset(): void;
  /** Called when the view changes (not on every level). */
  subscribe(fn: () => void): () => void;
  /** Called on every level. */
  subscribeLevel(fn: (l: HearLevel) => void): () => void;
}

/** The parts of a view that matter to a screen: everything but the running time. */
function keyOf(v: HearView): string {
  return JSON.stringify({ ...v, elapsed: 0 });
}

export function createHearLive(): HearLive {
  let view: HearView | null = null;
  let key = "";
  let cur: HearLevel = { level: 0, speaking: false, elapsed: 0, at: Number.NEGATIVE_INFINITY };
  const views = new Set<() => void>();
  const levels = new Set<(l: HearLevel) => void>();
  const stamp = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

  const putView = (v: HearView) => {
    const k = keyOf(v);
    if (k === key) {
      view = v;
      return;
    }
    key = k;
    view = v;
    for (const fn of views) fn();
  };
  const putLevel = (level: number, speaking: boolean, elapsed: number) => {
    cur = { level: Number.isFinite(level) ? Math.max(0, Math.min(1, level)) : 0, speaking, elapsed: Number.isFinite(elapsed) ? elapsed : 0, at: stamp() };
    for (const fn of levels) fn(cur);
  };

  return {
    level: () => cur,
    view: () => view,
    set(v, level, speaking) {
      const open = v.state !== "closed";
      putLevel(open ? level : 0, open && speaking, v.elapsed ?? 0);
      putView(v);
    },
    setView(v) {
      if (v.state === "closed" && cur.level > 0) putLevel(0, false, 0);
      putView(v);
    },
    reset() {
      putLevel(0, false, cur.elapsed);
    },
    subscribe(fn) {
      views.add(fn);
      return () => {
        views.delete(fn);
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
