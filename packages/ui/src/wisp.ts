/**
 * Wisp, Ghost's mascot: a round spirit with a rounded three-lobed hem and two mitts (the mark of
 * November 2026, "hem with hands"), the eyes cut out, one colour (currentColor) or lit from
 * within (the Mascot's `lit`). Hover or tap and it bounces up, grins and waves, then watches you;
 * leave and it settles back to exactly the brand glyph. The hem ripples where the tail swung.
 *
 * Plain data and math on the brand's 24 grid, driven by the machine in mascot.ts. The body, the
 * mitts and the eyes are figure.ts's, the same functions every still mark and icon is drawn
 * from (mark.ts), so the still mark and the live one are the same drawing.
 */
import { EYES as FIGURE_EYES, hemPath, mittPaths, wispPath } from "./figure";
import { blink, bodyTransform, type Character, clamp, type Detail, type El, eye, type Frame, kf, n, pose, type Scene, smooth, wave } from "./mascot";

const HOLE = { fill: "#000" };
const INK = { fill: "currentColor" };
const LINE = { fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeLinejoin: "round" } as const;
/** The body and mitts: filled, and marked so the Mascot can light them (its `lit`). */
const FLESH = { fill: "currentColor", "data-flesh": 1 } as const;
const CUT = { fill: "none", stroke: "#000", strokeLinecap: "round", strokeLinejoin: "round" } as const;

/**
 * Every animated quantity. Body: x, y, r(otation), sx/sy (squash), op(acity). Eyes: ex/ey (look),
 * es (size), bl (blink), hp (grin arcs), sl (shut, the sleeping arcs). Mouth: sm (smile),
 * mo (talking mouth, how open), mv (talking mouth shown). sw: the hem's ripple (-1..1).
 * aL/aR: the mitts' angles (deg; a positive aL and a negative aR raise them).
 * d1-d3: thinking dots. zo/zy: the "z" of hold, shown and risen.
 */
const REST: Frame = {
  x: 0, y: 0, r: 0, sx: 1, sy: 1, op: 1,
  ex: 0, ey: 0, es: 1, bl: 0, hp: 0, sl: 0,
  sm: 0, mo: 0, mv: 0, sw: 0, aL: 0, aR: 0,
  d1: 0, d2: 0, d3: 0, zo: 0, zy: 0,
};

/** The eyes: centres and radii (figure.ts), for the regular drawing and the simple one used under 24px. */
const eyesOf = (e: (typeof FIGURE_EYES)["regular"], k: number) => ({ lx: e[0][0], rx_: e[1][0], y: e[0][1], rx: e[0][2], ry: e[0][3], k });
const EYES = { regular: eyesOf(FIGURE_EYES.regular, 1), simple: eyesOf(FIGURE_EYES.small, 0.7) };

/** The body: Wisp's head, straight sides and the three-lobed hem; `s` (-1..1) ripples the hem. */
const body = hemPath;

/**
 * Where the drawing's middle sits on the 24 grid (the body runs from y 3 to about 20.3), so the
 * viewBox centres it.
 */
const MID_Y = 11.65;
/**
 * Half the viewBox: the hem ends higher than the tail did (y 20.3, not 23.6), so the box is
 * drawn 10% tighter than the 24 grid to keep Wisp the size it was in every placement. With
 * props, room for the raised mitts, the thinking dots and the hold's "z".
 */
const HALF = 10.8;
const PROPS_HALF = 14.4;

/**
 * Wisp at rest as one path for a single colour, the eyes cut out (nonzero fill rule, never
 * evenodd): the brand glyph. `simple` is the drawing under 32 px: no mitts, larger eyes.
 */
export function wispGlyph(simple: boolean): string {
  return wispPath({ mitts: !simple, small: simple });
}

const happyArc = (cx: number, cy: number) => `M${n(cx - 1.2)} ${n(cy + 0.5)}Q${n(cx)} ${n(cy - 1.3)} ${n(cx + 1.2)} ${n(cy + 0.5)}`;
/** A shut eye, curved down like a sleeper's. */
const shutArc = (cx: number, cy: number) => `M${n(cx - 1.15)} ${n(cy - 0.15)}Q${n(cx)} ${n(cy + 1.15)} ${n(cx + 1.15)} ${n(cy - 0.15)}`;

/** The thinking dots, small to large, rising up and right off the head. */
const DOTS = [
  { cx: 16.7, cy: 3.1, r: 0.62 },
  { cx: 18.8, cy: 1.2, r: 0.85 },
  { cx: 21.3, cy: -0.9, r: 1.08 },
] as const;

/** Talking, as how open the mouth is (0..1): syllables about five a second, in phrases with breaths between. */
function speech(t: number): number {
  const syllable = Math.abs(Math.sin((2 * Math.PI * t) / 400));
  const stress = 0.55 + 0.45 * Math.sin((2 * Math.PI * t) / 1100 + 1);
  const u = t % 2900;
  // A breath at the end of each phrase, eased so the mouth closes rather than snaps shut.
  const phrase = u < 2400 ? 1 : u < 2550 ? 1 - smooth((u - 2400) / 150) : u < 2750 ? 0 : smooth((u - 2750) / 150);
  return clamp(syllable * stress * 1.15, 0, 1) * phrase;
}

/** Small drawings move less, so the hop never leaves a 16px slot. */
const amp = (d: Detail) => (d.props ? 1 : d.simple ? 0.45 : 0.65);

export const WISP: Character = {
  id: "wisp",
  name: "Wisp",
  personality: "A soft, round spirit with a rippling hem and two small mitts.",
  gesture: "Bounces up, squeezes its eyes into a grin and waves hello, then watches you with a small smile.",
  rest: REST,
  gestureMs: 1000,
  leaveMs: 460,
  tapHoldMs: 1000,
  enter(t, { look }) {
    const p = t / 1000;
    const w = kf(p, [[0, 0], [0.75, 0], [1, 1]]);
    return {
      x: 0,
      y: kf(p, [[0, 0], [0.14, 0.4], [0.34, -1.8], [0.7, -1.3], [1, -1]]),
      sy: kf(p, [[0, 1], [0.14, 0.93], [0.34, 1.06], [0.55, 0.99], [1, 1]]),
      sx: kf(p, [[0, 1], [0.14, 1.05], [0.34, 0.96], [0.55, 1.01], [1, 1]]),
      r: kf(p, [[0, 0], [0.36, -4], [0.5, 4], [0.64, -4], [0.78, 3], [1, 0]]) + look.x * 6 * w,
      ex: look.x * 1.2 * w,
      ey: look.y * 0.8 * w,
      bl: 0,
      hp: kf(p, [[0, 0], [0.22, 0], [0.34, 1], [0.8, 1], [0.94, 0], [1, 0]]),
      sm: kf(p, [[0, 0], [0.25, 0], [0.4, 1], [1, 1]]),
      sw: kf(p, [[0, 0], [0.2, -0.7], [0.42, 0.9], [0.62, -0.6], [0.82, 0.35], [1, 0]]),
      // Raised high enough to read as a wave, never so high the hand goes behind the head.
      aR: kf(p, [[0, 0], [0.18, -30], [0.36, -80], [0.5, -52], [0.64, -80], [0.78, -55], [0.92, -25], [1, -12]]),
      aL: kf(p, [[0, 0], [0.3, 14], [1, 6]]),
    };
  },
  hold(t, { look }) {
    return {
      x: 0,
      y: -1 - 0.45 * wave(t, 2400),
      sx: 1,
      sy: 1,
      r: look.x * 6 + 1.6 * wave(t, 3600),
      ex: look.x * 1.2,
      ey: look.y * 0.8,
      bl: blink(t, 3600, 1400),
      hp: 0,
      sm: 1,
      sw: 0.55 * wave(t, 1700),
      aR: -12 - 6 * wave(t, 2400),
      aL: 6 + 4 * wave(t, 2400),
    };
  },
  idle(t) {
    return pose(REST, { y: -0.4 * wave(t, 3000), sw: 0.35 * wave(t, 2600), bl: blink(t, 4500, 2500) });
  },
  // One per voice phase. Each starts from a recognizable pose and loops slowly from there.
  moods: {
    passive: { moving: false, pose: () => REST },
    // Wide-eyed and looking up at you, a slow curious tilt, arms a little open.
    listening: {
      moving: true,
      pose: (t) => ({
        y: -0.6 - 0.35 * wave(t, 2400),
        r: 5 * wave(t, 3400),
        sw: 0.3 * wave(t, 2200),
        es: 1.12,
        ey: -0.3,
        ex: 0.35 * wave(t, 5200),
        bl: blink(t, 3400, 1300),
        sm: 0.7,
        aL: 12 + 3 * wave(t, 2400),
        aR: -12 - 3 * wave(t, 2400),
      }),
    },
    // Eyes up and aside, the near arm raised, thought dots appearing one after another.
    thinking: {
      moving: true,
      pose: (t) => {
        const u = (t % 1800) / 1800;
        return {
          y: -0.4 - 0.3 * wave(t, 2600),
          r: -5 + 2 * wave(t, 3000),
          sw: 0.25 * wave(t, 2600),
          ex: -0.7 + 0.15 * wave(t, 4000),
          ey: -0.9,
          bl: blink(t, 4200, 2200),
          aL: 24 + 4 * wave(t, 3000),
          aR: -4,
          d1: kf(u, [[0, 0], [0.12, 1], [0.8, 1], [0.95, 0], [1, 0]]),
          d2: kf(u, [[0, 0], [0.12, 0], [0.24, 1], [0.8, 1], [0.95, 0], [1, 0]]),
          d3: kf(u, [[0, 0], [0.24, 0], [0.36, 1], [0.8, 1], [0.95, 0], [1, 0]]),
        };
      },
      still: { ex: -0.7, ey: -0.9, r: -5, aL: 24, d1: 1, d2: 1, d3: 1 },
    },
    // Talking: the mouth follows a speech rhythm, the body bobs with it, the arms gesture.
    speaking: {
      moving: true,
      pose: (t) => {
        const m = speech(t);
        return {
          y: -0.5 - 0.25 * m - 0.3 * wave(t, 2400),
          r: 2.5 * wave(t, 2900),
          sw: 0.4 * wave(t, 1900),
          es: 1.05,
          bl: blink(t, 3800, 1600),
          mv: 1,
          mo: m,
          aL: 8 + 8 * (0.5 - 0.5 * Math.cos((2 * Math.PI * t) / 1700)),
          aR: -14 - 16 * (0.5 - 0.5 * Math.cos((2 * Math.PI * t) / 1300)),
        };
      },
      still: { mv: 1, mo: 0.55, es: 1.05 },
    },
    // Checked out (hold): eyes shut, a dozing tilt, a slow breath, a "z" rising now and then.
    hold: {
      moving: true,
      pose: (t) => {
        const u = (t % 3200) / 3200;
        return {
          y: -0.2 - 0.5 * wave(t, 4200),
          sy: 1 + 0.02 * wave(t, 4200),
          r: -7 + 1.2 * wave(t, 4200),
          sw: 0.2 * wave(t, 4200),
          sl: 1,
          sm: 0.35,
          zo: t < 600 ? 0 : kf(u, [[0, 0], [0.15, 1], [0.7, 1], [1, 0]]),
          zy: -2.4 * u,
        };
      },
      still: { r: -7, sl: 1, sm: 0.35, zo: 1, zy: -1 },
    },
    // Waking up while the models load: fading in, eyes drowsy and opening.
    starting: {
      moving: true,
      pose: (t) => {
        const c = 0.5 - 0.5 * Math.cos((2 * Math.PI * t) / 1600);
        return { op: 0.55 + 0.4 * c, bl: 0.75 - 0.6 * c, y: 0.3 - 0.6 * c };
      },
      still: { op: 0.8, bl: 0.4 },
    },
    // Can't reach the desk: slumped and faded, eyes nearly shut, arms down.
    offline: { moving: false, pose: () => ({ y: 0.8, r: 8, sy: 0.97, op: 0.72, bl: 0.85, aL: -10, aR: 10 }) },
  },
  scene(d): Scene {
    return {
      viewBox: d.props ? `${n(12 - PROPS_HALF)} ${n(MID_Y - PROPS_HALF)} ${n(2 * PROPS_HALF)} ${n(2 * PROPS_HALF)}` : `${n(12 - HALF)} ${n(MID_Y - HALF)} ${n(2 * HALF)} ${n(2 * HALF)}`,
      holes: [
        { tag: "ellipse", part: "eyeL", props: HOLE },
        { tag: "ellipse", part: "eyeR", props: HOLE },
        { tag: "path", part: "hapL", props: { ...CUT, strokeWidth: d.simple ? 1.5 : 1.2 } },
        { tag: "path", part: "hapR", props: { ...CUT, strokeWidth: d.simple ? 1.5 : 1.2 } },
        { tag: "path", part: "shutL", props: { ...CUT, strokeWidth: d.simple ? 1.5 : 1.2 } },
        { tag: "path", part: "shutR", props: { ...CUT, strokeWidth: d.simple ? 1.5 : 1.2 } },
        ...(d.simple
          ? []
          : [{ tag: "path", part: "smile", props: { ...CUT, strokeWidth: 1 } } satisfies El, { tag: "ellipse", part: "mouth", props: HOLE } satisfies El]),
      ],
      // The mitts are part of the body: filled with it, under the same mask, lit with it.
      body: [
        { tag: "path", part: "body", props: FLESH },
        ...(d.props ? [{ tag: "path", part: "mittL", props: FLESH } satisfies El, { tag: "path", part: "mittR", props: FLESH } satisfies El] : []),
      ],
      over: d.props
        ? [
            ...DOTS.map((dot, i) => ({ tag: "circle", part: `dot${i + 1}`, props: { ...INK, ...dot } }) satisfies El),
          ]
        : [],
      free: d.props ? [{ tag: "path", part: "z", props: { ...LINE, strokeWidth: 0.8, d: "M18.4 4.4h1.9l-1.9 2.3h1.9" } }] : [],
    };
  },
  attrs(f, d) {
    const a = amp(d);
    const e = d.simple ? EYES.simple : EYES.regular;
    const ex = f.ex * e.k;
    const ey = f.ey * e.k;
    // The eyes close into the grin's arcs: a crisp handover, never both half-drawn.
    const lid = Math.max(f.bl, f.hp, f.sl);
    // Open eyes hand over to the grin's arcs, or to shut ones: crisply, never both half-drawn.
    const closedBy = Math.max(f.hp, f.sl);
    const ovalOpacity = n(clamp((0.62 - closedBy) * 6, 0, 1));
    const arcOpacity = n(clamp((f.hp - 0.45) * 6, 0, 1));
    const shutOpacity = n(clamp((f.sl - 0.45) * 6, 0, 1) * (1 - clamp(f.hp * 2, 0, 1)));
    const out: ReturnType<Character["attrs"]> = {
      root: { transform: bodyTransform(0, f.y * a, f.r * (d.props ? 1 : 0.7), 12, 13, 1 + (f.sx - 1) * a, 1 + (f.sy - 1) * a, 12, 20.3), opacity: n(f.op) },
      body: { d: body(f.sw * (d.props ? 1 : 0.6)) },
      eyeL: { ...eye(e.lx + ex, e.y + ey, e.rx * f.es, e.ry * f.es, lid), opacity: ovalOpacity },
      eyeR: { ...eye(e.rx_ + ex, e.y + ey, e.rx * f.es, e.ry * f.es, lid), opacity: ovalOpacity },
      hapL: { d: happyArc(e.lx + ex, e.y + ey), opacity: arcOpacity },
      hapR: { d: happyArc(e.rx_ + ex, e.y + ey), opacity: arcOpacity },
      shutL: { d: shutArc(e.lx + ex, e.y + ey), opacity: shutOpacity },
      shutR: { d: shutArc(e.rx_ + ex, e.y + ey), opacity: shutOpacity },
    };
    if (!d.simple) {
      // The smile keeps its curve and narrows as it fades, never a flat dash.
      const sx = 12 + ex * 0.6;
      const sy = 13.6 + ey * 0.5;
      const w = 1.1 * (0.45 + 0.55 * f.sm);
      // The talking mouth takes the smile's place while it shows.
      out.smile = { d: `M${n(sx - w)} ${n(sy)}Q${n(sx)} ${n(sy + 1.2)} ${n(sx + w)} ${n(sy)}`, opacity: n(Math.pow(clamp(f.sm, 0, 1), 1.5) * (1 - clamp(f.mv, 0, 1))) };
      out.mouth = { cx: n(sx), cy: n(14.1 + ey * 0.4), rx: n(0.85 + 0.25 * f.mo), ry: n(0.22 + 1.0 * f.mo), opacity: n(clamp(f.mv, 0, 1)) };
    }
    if (d.props) {
      const [mL, mR] = mittPaths(f.aL, f.aR);
      out.mittL = { d: mL };
      out.mittR = { d: mR };
      DOTS.forEach((_, i) => (out[`dot${i + 1}`] = { opacity: n(clamp(f[`d${i + 1}`], 0, 1)) }));
      out.z = { transform: `translate(${n(-f.zy * 0.35)} ${n(f.zy)})`, opacity: n(clamp(f.zo, 0, 1)) };
    }
    return out;
  },
};

/**
 * Wisp saying hello, as one still pose: mid-wave, grinning, a little lifted. A gesture of the
 * live mascot, not the mark: the still mark has no smile (`WISP.rest`, docs/design-system.md).
 */
export function wavePose(): Frame {
  const f = WISP.enter(0.5 * WISP.gestureMs, { look: { x: 0, y: 0 } });
  return { ...WISP.rest, ...f, x: 0, y: -0.6, r: 0, bl: 0 };
}
