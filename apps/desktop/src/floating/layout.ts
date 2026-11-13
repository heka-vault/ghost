/**
 * Where the floating room draws, as a pure function of the monitor and the operator's choice
 * (docs/design-system.md, The floating room: the layout rule). Everything Ghost draws while
 * forward is inside the monitor's safe area and inside a designed share of it:
 *
 * - **Safe area**: 48 px from the top (GNOME's bar), 28 px from the other edges.
 * - **Scale**: the shorter side over 1080, between 0.5 and 1.2 (a 1280×720 laptop draws at
 *   0.67, a 4K monitor at 1.2 of the CSS pixels it has); the corner is 0.64 of that.
 * - **Share** (the bounding box of Wisp's ring, the places and their names, a card, captions)
 *   at most `MAX_SHARE` of the monitor: 45% centre, 36% beside the work, 16% in the corner.
 *   The dim follows the same box, feathered: the operator's work outside it stays as it was.
 *   The tests hold every placement to it on a 1920×1080, a 1280×720 and a portrait monitor.
 * - **Text**: a card is at most 640 px wide at scale 1 (and the safe width) and three lines of
 *   question; captions are two lines. Nothing is sized from the length of what is said.
 *
 * Placements: `beside` (default: a column at the edge away from the work, the places fanned
 * toward the middle), `centre` (the stage as it was), `corner` (small, low on the far side).
 */
export type Placement = "beside" | "centre" | "corner";
export type Side = "left" | "right";

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export const SAFE = { top: 48, right: 28, bottom: 28, left: 28 };
export const MAX_SHARE: Record<Placement, number> = { centre: 0.45, beside: 0.36, corner: 0.16 };
export const PLACEMENTS: Placement[] = ["beside", "centre", "corner"];

export interface LayoutInput {
  w: number;
  h: number;
  placement: Placement;
  /** The side away from the operator's work (the shell's choice); right when not known. */
  side: Side | "centre" | null;
  /** How many places the beams reach (0..6). */
  places: number;
  card: boolean;
  captions: boolean;
}

export interface Layout {
  scale: number;
  /** Wisp's centre. */
  core: [number, number];
  /** Wisp's box side and the ring's outer radius, CSS px. */
  wisp: number;
  ring: number;
  /** Each place's point (the beam's end); its name sits centred on it. */
  places: [number, number][];
  /** A place's name box (for clamping and the share). */
  label: { w: number; h: number };
  card: Rect | null;
  captions: Rect | null;
  /** The stage's bounding box (Wisp, the places, a card): the dim's box. */
  bounds: Rect;
  /** Of the monitor's area, what the stage's box and the captions' band cover together. */
  share: number;
  /** The field's size factor (`uSize`): the ring and glow at this layout's scale. */
  size: number;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

export function scaleOf(w: number, h: number, placement: Placement): number {
  const s = clamp(Math.min(w, h) / 1080, 0.5, 1.2);
  return placement === "corner" ? s * 0.64 : s;
}

export function layoutOf(i: LayoutInput): Layout {
  const { w, h, placement } = i;
  const s = scaleOf(w, h, placement);
  const side: Side = i.side === "left" ? "left" : "right";
  const safe = { x0: SAFE.left, y0: SAFE.top, x1: w - SAFE.right, y1: h - SAFE.bottom };
  const wisp = Math.round(176 * s);
  const ring = Math.round(0.26 * 1080 * s);
  const label = { w: Math.round(170 * s), h: Math.round(50 * s) };
  const capH = i.captions ? Math.round(76 * s) : 0;
  const cardW = Math.min(Math.round(640 * s), safe.x1 - safe.x0);
  const cardH = i.card ? Math.round(250 * s) : 0;

  // The core and the direction the places fan out in: a narrow fan for a few, wider for six.
  const n0 = clamp(i.places, 0, 6);
  let core: [number, number];
  let dir: number;
  let spread: number;
  let reach: number;
  if (placement === "centre") {
    core = [w / 2, clamp(h * 0.36, safe.y0 + ring + label.h, safe.y1 - ring)];
    dir = -Math.PI / 2;
    spread = Math.PI * Math.min(0.8, 0.3 * Math.max(1, n0 - 1));
    reach = ring * 1.55;
  } else if (placement === "beside") {
    const colW = clamp(w * 0.28, 380 * s, 560 * s);
    const x = side === "right" ? safe.x1 - colW / 2 : safe.x0 + colW / 2;
    core = [x, clamp(h * 0.4, safe.y0 + ring + label.h, safe.y1 - ring - cardH)];
    dir = side === "right" ? Math.PI : 0;
    spread = Math.PI * Math.min(0.85, 0.3 * Math.max(1, n0 - 1));
    reach = ring * 1.45;
  } else {
    const r = ring + label.h;
    const x = side === "right" ? safe.x1 - r : safe.x0 + r;
    core = [x, safe.y1 - r - capH];
    dir = side === "right" ? (-Math.PI * 3) / 4 : -Math.PI / 4;
    spread = Math.PI * 0.6;
    reach = ring * 1.5;
  }

  // The places, on an arc, each name kept inside the safe area.
  const n = clamp(i.places, 0, 6);
  const places: [number, number][] = [];
  for (let k = 0; k < n; k++) {
    const a = n === 1 ? dir : dir - spread / 2 + (spread * k) / (n - 1);
    // Alternate a little in and out so neighbouring names don't touch.
    const r = reach * (n > 3 && k % 2 ? 0.82 : 1);
    places.push([
      clamp(core[0] + Math.cos(a) * r, safe.x0 + label.w / 2, safe.x1 - label.w / 2),
      clamp(core[1] + Math.sin(a) * r, safe.y0 + label.h / 2, safe.y1 - label.h / 2),
    ]);
  }

  // The card under Wisp's label (above the core in the corner), inside the safe width.
  let card: Rect | null = null;
  if (i.card) {
    const cx = clamp(core[0], safe.x0 + cardW / 2, safe.x1 - cardW / 2);
    // Under Wisp's label and its two buttons (Floating.tsx's `.flt-under`).
    const below = core[1] + wisp * 0.62 + 76 * s;
    const y = placement === "corner" ? Math.max(safe.y0, core[1] - wisp / 2 - 16 * s - cardH) : Math.min(below, safe.y1 - cardH - capH);
    card = { x: cx - cardW / 2, y, w: cardW, h: cardH };
  }

  let captions: Rect | null = null;
  if (i.captions) {
    const cw = Math.min(Math.round((placement === "centre" ? 960 : 620) * s), safe.x1 - safe.x0);
    const cx = placement === "centre" ? w / 2 : clamp(core[0], safe.x0 + cw / 2, safe.x1 - cw / 2);
    captions = { x: cx - cw / 2, y: safe.y1 - capH, w: cw, h: capH };
  }

  // The stage's box: Wisp's ring and label, the places' names, the card. Captions are their own
  // band at the bottom; the share counts both.
  const boxes: Rect[] = [{ x: core[0] - ring, y: core[1] - ring, w: ring * 2, h: ring * 2 + label.h }];
  for (const p of places) boxes.push({ x: p[0] - label.w / 2, y: p[1] - label.h / 2, w: label.w, h: label.h });
  if (card) boxes.push(card);
  const x0 = Math.max(safe.x0, Math.min(...boxes.map((b) => b.x)));
  const y0 = Math.max(safe.y0, Math.min(...boxes.map((b) => b.y)));
  const x1 = Math.min(safe.x1, Math.max(...boxes.map((b) => b.x + b.w)));
  const y1 = Math.min(safe.y1, Math.max(...boxes.map((b) => b.y + b.h)));
  const bounds = { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };

  const share = (bounds.w * bounds.h + (captions ? captions.w * captions.h : 0)) / (w * h);
  return { scale: s, core, wisp, ring, places, label, card, captions, bounds, share, size: (ring / (0.26 * 1080)) * (1080 / Math.min(w, h)) };
}

/** True when `r` is wholly inside the monitor's safe area. */
export function insideSafe(r: Rect, w: number, h: number): boolean {
  return r.x >= SAFE.left - 0.5 && r.y >= SAFE.top - 0.5 && r.x + r.w <= w - SAFE.right + 0.5 && r.y + r.h <= h - SAFE.bottom + 0.5;
}
