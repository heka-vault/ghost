/**
 * Owner: design. The concepts' clock: "now" runs in real time from LIVE, the view can be pulled
 * back through the day and lets go back to now, and the scripted sequence ends with a tour back
 * through the day. Plain TypeScript, read every frame by the renderers; React only subscribes
 * to coarse changes.
 */
import { type Answer, buildDay, type Day, DAY_START, H, LIVE, M, type State, stateAt } from "./day";

/** When the scripted tour back through the day starts, seconds after LIVE. */
const TOUR_AT = 44;
const TOUR = { back: 5, hold: 2.5, forward: 7, to: 9 * H + 12 * M + 4 };

export interface Engine {
  day: Day;
  /** Real "now", seconds of the day. */
  live: number;
  /** The moment on screen. Equals live unless pulled back or touring. */
  view: number;
  mode: "live" | "pulled" | "tour";
  paused: boolean;
  answers: Map<string, Answer>;
  /** Bumped on every discrete change (answer, mode, restart) so React can re-read. */
  version: number;
  reduced: boolean;
}

export function createEngine(reduced: boolean) {
  const e: Engine = { day: buildDay(), live: LIVE, view: LIVE, mode: "live", paused: false, answers: new Map(), version: 0, reduced };
  let last = performance.now();
  let tourStart = -1;
  let tourDone = false;
  const subs = new Set<() => void>();
  const bump = () => {
    e.version++;
    subs.forEach((f) => f());
  };

  const ease = (x: number) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);

  let memo: { view: number; version: number; s: State } | null = null;

  return {
    e,
    /** The state on screen, computed once per frame however many parts read it. */
    state(): State {
      if (!memo || memo.view !== e.view || memo.version !== e.version) memo = { view: e.view, version: e.version, s: stateAt(e.day, e.view, e.answers, 10) };
      return memo.s;
    },
    subscribe(f: () => void) {
      subs.add(f);
      return () => subs.delete(f);
    },
    /** Advance the clock; call once per frame. */
    tick(now: number) {
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      if (!e.paused) e.live += dt;
      if (e.mode === "live") e.view = e.live;
      if (!tourDone && e.mode === "live" && e.live >= LIVE + TOUR_AT) {
        e.mode = "tour";
        tourStart = e.live;
        bump();
      }
      if (e.mode === "tour") {
        const k = e.live - tourStart;
        const from = tourStart;
        if (k < TOUR.back) e.view = from + (TOUR.to - from) * ease(k / TOUR.back);
        else if (k < TOUR.back + TOUR.hold) e.view = TOUR.to + (k - TOUR.back) * 0.8;
        else if (k < TOUR.back + TOUR.hold + TOUR.forward) {
          const s = TOUR.to + TOUR.hold * 0.8;
          e.view = s + (e.live - s) * ease((k - TOUR.back - TOUR.hold) / TOUR.forward);
        } else {
          tourDone = true;
          e.mode = "live";
          e.view = e.live;
          bump();
        }
      }
    },
    /** Pull the view by `ds` seconds (negative is back). */
    pull(ds: number) {
      tourDone = true;
      e.view = Math.max(DAY_START, Math.min(e.live, e.view + ds));
      const mode = e.live - e.view < 1.5 ? "live" : "pulled";
      if (mode !== e.mode) {
        e.mode = mode;
        bump();
      }
    },
    /** Put the view at `t` (clamped). */
    seek(t: number) {
      this.pull(t - e.view);
    },
    /** Let go: back to now. */
    toLive() {
      tourDone = true;
      e.mode = "live";
      e.view = e.live;
      bump();
    },
    togglePause() {
      e.paused = !e.paused;
      bump();
    },
    answer(id: string, choice: string, how: "voice" | "desk" = "desk") {
      if (e.mode !== "live" || e.answers.has(id)) return;
      e.answers.set(id, { at: e.live, choice, how });
      bump();
    },
    /** Start the scripted sequence again from the top. */
    restart() {
      e.live = LIVE;
      e.view = LIVE;
      e.mode = "live";
      e.paused = false;
      e.answers = new Map();
      tourDone = false;
      last = performance.now();
      bump();
    },
    /** Jump the live clock to a point in the script, for screenshots: `?at=20`. */
    jump(offset: number) {
      e.live = LIVE + offset;
      e.view = e.live;
      if (offset > TOUR_AT) tourDone = true;
      bump();
    },
  };
}

export type EngineHandle = ReturnType<typeof createEngine>;
