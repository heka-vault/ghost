/**
 * Wisp's body, the one drawing (November 2026, "hem with hands": docs/design-system.md, The mark).
 * The live mascot (wisp.ts), the brand glyph (brand.ts) and every still mark, icon and tile
 * (mark.ts) are built from these functions, so they cannot drift apart.
 *
 * On the brand's 24 grid: Wisp's head (a circle of 7 about (12, 10)), the sides falling straight
 * with a slight flare, the bottom closed in three shallow round lobes (the hem), and two mitts
 * that are part of the body, the right one a little raised. No tail, no smile, no stick arms.
 *
 * Winding matters: the body and the mitts are drawn clockwise and the eyes counter-clockwise,
 * so one path with the default (nonzero) fill rule fills where the mitts overlap the body and
 * still cuts the eyes out. Never draw the glyph with `fill-rule="evenodd"`: the overlaps of the
 * mitts would turn into holes.
 *
 * Plain TypeScript with no imports, so a script can load it with Node alone.
 */

/** A number short enough for an attribute, never in exponent notation. */
export const n = (v: number) => (Math.abs(v) < 1e-4 ? 0 : Math.round(v * 1000) / 1000);

/** The hem's shape at rest. */
export const HEM = { lobes: 3, depth: 1.7, flare: 0.4, base: 18.6 } as const;

/** How far the hem's lobes deepen and shallow as `s` swings (the ripple's strength). */
const RIPPLE = 0.42;
/** How far the hem sways sideways at full swing, in grid units. */
const SWAY = 0.35;

/** One segment of the outline: a cubic from the previous point. */
export interface Cubic {
  c1: [number, number];
  c2: [number, number];
  to: [number, number];
}

/**
 * The body below the head as cubics, from (19, 10) round the hem to (5, 10). `s` (-1..1, the
 * live mascot's `sw`) ripples the hem: each lobe deepens and shallows a little out of step with
 * the next, so a swing of `s` reads as a wave running across the hem, and the hem sways a touch
 * with it. At `s = 0` it is the still mark exactly, symmetric about x = 12.
 */
export function hemCurves(s = 0): Cubic[] {
  const { lobes, depth, flare, base } = HEM;
  const R = 19 + flare;
  const L = 5 - flare;
  const w = (R - L) / lobes;
  const sway = s * SWAY;
  const out: Cubic[] = [{ c1: [19, 13.5], c2: [R, 16], to: [R + sway * 0.5, base] }];
  for (let i = 0; i < lobes; i++) {
    const a = R - i * w + sway * (i === 0 ? 0.5 : 1);
    const b = R - (i + 1) * w + sway * (i === lobes - 1 ? 0.5 : 1);
    // The rest's own unevenness is subtracted, so s = 0 is the plain, even hem.
    const dep = depth * (1 + RIPPLE * (Math.sin(s * Math.PI + i * 2.1) - Math.sin(i * 2.1)));
    const c = 1.33 * dep;
    out.push({ c1: [a - w * 0.05, base + c], c2: [b + w * 0.05, base + c], to: [b, base] });
  }
  out.push({ c1: [L, 16], c2: [5, 13.5], to: [5, 10] });
  return out;
}

/** The head: a half circle from (5, 10) over the top to (19, 10), clockwise. */
export const HEAD = "M5 10A7 7 0 0 1 19 10";

/** The body's outline (head, sides, hem), clockwise. */
export function hemPath(s = 0): string {
  return HEAD + hemCurves(s).map((k) => `C${n(k.c1[0])} ${n(k.c1[1])} ${n(k.c2[0])} ${n(k.c2[1])} ${n(k.to[0])} ${n(k.to[1])}`).join("") + "Z";
}

/** An ellipse turned by `rot` degrees, as a closed path. `clockwise` sets its winding. */
export function oval(cx: number, cy: number, rx: number, ry: number, rot = 0, clockwise = true): string {
  const a = (rot * Math.PI) / 180;
  const px = cx + rx * Math.cos(a);
  const py = cy + rx * Math.sin(a);
  const qx = cx - rx * Math.cos(a);
  const qy = cy - rx * Math.sin(a);
  const sw = clockwise ? 1 : 0;
  return `M${n(px)} ${n(py)}A${n(rx)} ${n(ry)} ${n(rot)} 1 ${sw} ${n(qx)} ${n(qy)}A${n(rx)} ${n(ry)} ${n(rot)} 1 ${sw} ${n(px)} ${n(py)}Z`;
}

/** Where each mitt hinges on the body's edge. */
export const HINGE = { left: [5.6, 13.6], right: [18.4, 13.2] } as const;

/**
 * The mitts' centres and angles. `aL`/`aR` are the live mascot's arm angles in degrees (the
 * same convention the arm lines had: a positive `aL` and a negative `aR` raise them); at 0 both
 * are at rest, the right a little raised.
 */
export function mittPlaces(aL = 0, aR = 0, size = 1): { cx: number; cy: number; rot: number }[] {
  const at = (h: readonly [number, number], deg: number) => {
    const a = (deg * Math.PI) / 180;
    return { cx: h[0] + Math.cos(a) * 1.75 * size, cy: h[1] + Math.sin(a) * 1.75 * size, rot: deg };
  };
  return [at(HINGE.left, 180 - 32 + aL), at(HINGE.right, -18 + aR)];
}

/** The mitts as two clockwise ovals, `[left, right]`. */
export function mittPaths(aL = 0, aR = 0, size = 1): [string, string] {
  const [l, r] = mittPlaces(aL, aR, size);
  return [oval(l.cx, l.cy, 1.9 * size, 1.3 * size, l.rot), oval(r.cx, r.cy, 1.9 * size, 1.3 * size, r.rot)];
}

/** An eye: centre and radii. */
export type Eye = readonly [cx: number, cy: number, rx: number, ry: number];

/** The eyes of the regular drawing, and of the small one (under 24 px: larger, so they read). */
export const EYES: { regular: readonly [Eye, Eye]; small: readonly [Eye, Eye] } = {
  regular: [
    [9.55, 9.9, 1.2, 2.0],
    [14.45, 9.9, 1.2, 2.0],
  ],
  small: [
    [9.35, 10.1, 1.5, 2.15],
    [14.65, 10.1, 1.5, 2.15],
  ],
};

/** An eye as a counter-clockwise ellipse: a hole in a clockwise body under the nonzero rule. */
export const eyeHole = ([cx, cy, rx, ry]: Eye) =>
  `M${n(cx - rx)} ${n(cy)}a${n(rx)} ${n(ry)} 0 1 0 ${n(2 * rx)} 0a${n(rx)} ${n(ry)} 0 1 0 ${n(-2 * rx)} 0Z`;

/**
 * The still mark's levels of detail, by the pixel size it is drawn at. Under 32 px the mitts
 * only widen the blob, so the plain hem is the mark; under 24 the eyes are larger.
 */
export const MITTS_FROM = 32;
export const SMALL_EYES_UNDER = 24;

/**
 * Wisp at rest as one path for a single colour, the eyes cut out (nonzero fill rule): the brand
 * glyph. `mitts` adds the hands (drawn at 32 px and up).
 */
export function wispPath({ mitts = true, small = false }: { mitts?: boolean; small?: boolean } = {}): string {
  const eyes = small ? EYES.small : EYES.regular;
  return hemPath(0) + (mitts ? mittPaths().join("") : "") + eyes.map(eyeHole).join("");
}

/** The drawing's box at rest with the mitts, on the 24 grid: [x0, y0, x1, y1]. */
export const WISP_BOX = [2.36, 3.0, 21.92, 20.3] as const;
