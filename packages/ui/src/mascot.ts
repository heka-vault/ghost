/**
 * The mascot's motion, with no DOM and no React, so the desktop app (SVG in the DOM) and the
 * phone (react-native-svg) drive the same character. A `Character` is a drawing described as
 * data plus pure functions from time to a `Frame` of numbers; `createMotion` is the two-stage machine from the
 * animated-svg-icons skill: Stage 1 plays when hovered or tapped, Stage 2 always settles back to
 * the exact rest pose.
 *
 * Every change of stage starts from the pose on screen, so nothing ever jumps: re-hovering
 * halfway through a settle, or tapping while hovered, blends from wherever the drawing is.
 * Leaving during Stage 1 lets the gesture finish first, then settles (the skill's
 * pendingReturn guard), so a quick pass of the pointer never cuts a hop off mid-air.
 */

/** One pose: every animated quantity of a character, by name. */
export type Frame = Record<string, number>;

export interface Look {
  /** -1 (left) to 1 (right). */
  x: number;
  /** -1 (up) to 1 (down). */
  y: number;
}

export interface Ctx {
  /** Where the character is looking, smoothed. Already clamped to -1..1. */
  look: Look;
}

/** Which drawing a size gets. */
export interface Detail {
  /** Under 24px: the simplified body, like GHOST_PATH_SMALL. */
  simple: boolean;
  /** 40px and up: props (Wisp's arms), in a padded viewBox. */
  props: boolean;
}

export function detailFor(size: number): Detail {
  return { simple: size < 24, props: size >= 40 };
}

/** An element of a character's drawing. Static attributes are React/react-native-svg prop names. */
export interface El {
  tag: "path" | "ellipse" | "circle" | "line" | "g";
  /** Animated elements are addressed by part; `attrs` returns their changing attributes. */
  part?: string;
  props?: Record<string, string | number>;
  children?: El[];
}

export interface Scene {
  viewBox: string;
  /** Cut out of the body (drawn black inside the mask): eyes, grin, smile. */
  holes: El[];
  /** The body, filled with currentColor, under the mask. */
  body: El[];
  /** Drawn on top of the body and moved with it (arms). */
  over: El[];
  /** Drawn on top, not moved with the body. */
  free: El[];
}

/** Changing attributes per part. Names are identical in SVG and React: transform, d, cx, rx... */
export type Attrs = Record<string, Record<string, string | number>>;

export interface Character {
  id: string;
  name: string;
  /** One line: who this character is. */
  personality: string;
  /** One line: what it does when you reach for it. */
  gesture: string;
  rest: Frame;
  /** Stage 1, the gesture: `enter(gestureMs)` must equal `hold(0)`. */
  gestureMs: number;
  enter(t: number, ctx: Ctx): Frame;
  /** Still hovered after the gesture: attentive, with the slow loops that make it alive. */
  hold(t: number, ctx: Ctx): Frame;
  /** Stage 2, back to rest. */
  leaveMs: number;
  /** After a tap (no hover), how long it holds before settling. */
  tapHoldMs: number;
  /** The opt-in idle loop for hero placements. The machine fades it in and out, so it may start anywhere. */
  idle(t: number): Frame;
  /**
   * What the character is doing when nobody is reaching for it, by name (Ghost's voice phases:
   * listening, thinking, speaking...). The machine blends from one to the next.
   */
  moods: Record<string, Mood>;
  scene(d: Detail): Scene;
  /** `clock` is wall time in ms, for loops that must not rewind when a stage changes. */
  attrs(f: Frame, d: Detail, clock: number): Attrs;
}

export interface Mood {
  /** The pose `t` ms into the mood. Frames may leave keys out; the rest pose fills them. */
  pose(t: number): Frame;
  /** False for a still pose: once it is reached, the frame loop may stop. */
  moving: boolean;
  /** The one pose reduced motion shows for this mood. Defaults to `pose(0)`. */
  still?: Frame;
}

// ---- easing and keyframes -------------------------------------------------------------------

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
export const easeInOut = (p: number) => (p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2);
export const easeOut = (p: number) => 1 - Math.pow(1 - p, 3);
export const smooth = (p: number) => p * p * (3 - 2 * p);

/**
 * A value along keyframes `[progress, value]`, eased in and out between each pair. Overshoot is
 * written into the values (a 1.08 before the 1), so every segment can share one curve.
 */
export function kf(p: number, stops: readonly (readonly [number, number])[]): number {
  if (p <= stops[0][0]) return stops[0][1];
  for (let i = 1; i < stops.length; i++) {
    const [p1, v1] = stops[i];
    if (p <= p1) {
      const [p0, v0] = stops[i - 1];
      return v0 + (v1 - v0) * easeInOut((p - p0) / (p1 - p0));
    }
  }
  return stops[stops.length - 1][1];
}

/** A slow loop that is exactly 0 at t = 0, so a hold starts where its gesture ended. */
export const wave = (t: number, periodMs: number, phase = 0) => Math.sin((2 * Math.PI * t) / periodMs + phase) - Math.sin(phase);

/** Eyelid closure (0 open, 1 shut): a 170ms blink every `periodMs`, the first at `firstMs`. */
export function blink(t: number, periodMs: number, firstMs: number): number {
  if (t < firstMs) return 0;
  const u = (t - firstMs) % periodMs;
  if (u >= 170) return 0;
  return u < 60 ? easeOut(u / 60) : 1 - easeInOut((u - 60) / 110);
}

export function lerpFrame(a: Frame, b: Frame, w: number): Frame {
  if (w <= 0) return a;
  if (w >= 1) return b;
  const out: Frame = {};
  for (const k in b) out[k] = (a[k] ?? b[k]) + (b[k] - (a[k] ?? b[k])) * w;
  return out;
}

/** Full rest pose with only the given keys changed. */
export const pose = (rest: Frame, f: Frame): Frame => ({ ...rest, ...f });

/** A number short enough for an attribute, never in exponent notation. */
export const n = (v: number) => (Math.abs(v) < 1e-4 ? 0 : Math.round(v * 1000) / 1000);

/** translate, then rotate about (rx, ry), then scale about (sx0, sy0): a body's whole move. */
export function bodyTransform(x: number, y: number, r: number, rx: number, ry: number, sx: number, sy: number, px: number, py: number): string {
  return `translate(${n(x)} ${n(y)}) rotate(${n(r)} ${n(rx)} ${n(ry)}) translate(${n(px)} ${n(py)}) scale(${n(sx)} ${n(sy)}) translate(${n(-px)} ${n(-py)})`;
}

/** An eye that closes from the top: the lower lid stays put while the upper one comes down. */
export function eye(cx: number, cy: number, rx: number, ry: number, closed: number): Record<string, number> {
  const open = Math.max(0.12, 1 - clamp(closed, 0, 1));
  const h = Math.max(0.2, ry * open);
  return { cx: n(cx), cy: n(cy + (ry - h) * 0.7), rx: n(rx), ry: n(h) };
}

// ---- the machine ----------------------------------------------------------------------------

type Stage = "rest" | "enter" | "hold" | "leave";

/** How long Stage 1 blends in from the pose on screen. */
const BLEND_IN_MS = 140;
/** How long one mood takes to become the next. */
const MOOD_BLEND_MS = 450;
/** Eye tracking time constant: quick enough to feel attentive, slow enough not to twitch. */
const LOOK_TAU_MS = 90;
/** Where a tapped character looks when there is no pointer: up, at you. */
export const LOOK_AT_YOU: Look = { x: 0, y: -0.45 };

export interface Motion {
  hover(on: boolean, now: number): void;
  /** A tap, a click, or the app calling for attention: play once, hold briefly, settle. */
  play(now: number): void;
  setLookTarget(look: Look): void;
  /** Switch to a mood (null: rest), blending from the pose on screen. Unknown names rest. */
  setMood(mood: string | null, now: number): void;
  /** The pose at `now`. Call once per animation frame. */
  frame(now: number, idle: boolean): Frame;
  /** True while a gesture (hover or tap) is playing or settling; false at rest or in a mood. */
  readonly gesturing: boolean;
  /** True when nothing is moving; the caller may stop its frame loop. */
  readonly settled: boolean;
}

export function createMotion(v: Character): Motion {
  let stage: Stage = "rest";
  let t0 = 0;
  let from: Frame = v.rest;
  let last: Frame = v.rest;
  let hovered = false;
  let tapped = false;
  let pendingLeave = false;
  let lastNow = -1;
  let idleT0 = -1;
  // Idle fades in and out instead of switching, so toggling it never jumps.
  let idleW = 0;
  const look: Look = { x: 0, y: 0 };
  let target: Look = { x: 0, y: 0 };
  let mood: Mood | null = null;
  let moodT0 = 0;
  // The base pose being blended away from after a mood change, until the blend completes.
  let moodFrom: Frame | null = null;
  let lastBase: Frame = v.rest;
  // Every frame carries every key, so blends between poses never meet a missing one.
  const full = (f: Frame): Frame => ({ ...v.rest, ...f });

  const begin = (s: Stage, now: number) => {
    stage = s;
    t0 = now;
    from = last;
  };

  return {
    hover(on, now) {
      if (on === hovered) return;
      hovered = on;
      if (on) {
        tapped = false;
        pendingLeave = false;
        if (stage === "rest" || stage === "leave") begin("enter", now);
      } else if (stage === "enter") {
        pendingLeave = true;
      } else if (stage === "hold") {
        begin("leave", now);
      }
    },
    play(now) {
      tapped = !hovered;
      pendingLeave = false;
      begin("enter", now);
    },
    setLookTarget(l) {
      target = { x: clamp(l.x, -1, 1), y: clamp(l.y, -1, 1) };
    },
    setMood(name, now) {
      const next = name ? v.moods[name] ?? null : null;
      if (next === mood) return;
      mood = next;
      moodT0 = now;
      moodFrom = lastBase;
    },
    frame(now, idle) {
      const dt = lastNow < 0 ? 16 : Math.min(64, now - lastNow);
      lastNow = now;
      const k = 1 - Math.exp(-dt / LOOK_TAU_MS);
      const aim = hovered ? target : tapped && stage !== "leave" && stage !== "rest" ? LOOK_AT_YOU : { x: 0, y: 0 };
      look.x += (aim.x - look.x) * k;
      look.y += (aim.y - look.y) * k;
      const ctx: Ctx = { look: { x: look.x, y: look.y } };

      if (stage === "enter" && now - t0 >= v.gestureMs) {
        if (pendingLeave || (!hovered && !tapped)) {
          pendingLeave = false;
          begin("leave", now);
        } else {
          stage = "hold";
          t0 += v.gestureMs;
        }
      }
      if (stage === "hold" && tapped && !hovered && now - t0 >= v.tapHoldMs) {
        tapped = false;
        begin("leave", now);
      }
      if (stage === "leave" && now - t0 >= v.leaveMs) stage = "rest";

      // The idle clock never restarts: a settle lands on the idle pose of that moment and
      // carries on from it.
      if (idleT0 < 0) idleT0 = now;
      idleW += ((idle ? 1 : 0) - idleW) * (1 - Math.exp(-dt / 300));
      if (Math.abs(idleW - (idle ? 1 : 0)) < 1e-3) idleW = idle ? 1 : 0;
      let base = mood ? full(mood.pose(now - moodT0)) : idleW > 0 ? lerpFrame(v.rest, full(v.idle(now - idleT0)), idleW) : v.rest;
      if (moodFrom) {
        const w = clamp((now - moodT0) / MOOD_BLEND_MS, 0, 1);
        if (w >= 1) moodFrom = null;
        else base = lerpFrame(moodFrom, base, smooth(w));
      }
      lastBase = base;
      const t = now - t0;
      let f: Frame;
      switch (stage) {
        case "enter":
          f = lerpFrame(from, full(v.enter(t, ctx)), smooth(clamp(t / BLEND_IN_MS, 0, 1)));
          break;
        case "hold":
          f = full(v.hold(t, ctx));
          break;
        case "leave":
          f = lerpFrame(from, base, easeOut(clamp(t / v.leaveMs, 0, 1)));
          break;
        default:
          f = base;
      }
      last = f;
      return f;
    },
    get settled() {
      return stage === "rest" && idleW === 0 && !moodFrom && !mood?.moving && Math.abs(look.x) < 1e-3 && Math.abs(look.y) < 1e-3;
    },
    get gesturing() {
      return stage !== "rest";
    },
  };
}

/**
 * The one pose reduced motion shows: the attentive face while hovered, otherwise the mood's
 * still pose (or rest), with no movement to get there.
 */
export function stillPose(v: Character, active: boolean, mood?: string | null): Frame {
  if (active) return { ...v.rest, ...v.hold(0, { look: { x: 0, y: 0 } }) };
  const m = mood ? v.moods[mood] : undefined;
  return m ? { ...v.rest, ...(m.still ?? m.pose(0)) } : v.rest;
}
