/**
 * Ghost's mark (November 2026, "hem with hands"; docs/design-system.md, The mark): every still
 * drawing of the brand as an SVG string, from figure.ts's body, so the icons, the tiles, the
 * favicon, the tray and the live mascot are one drawing.
 *
 * The rules, in code:
 * - The mark is Wisp lit from within (a white-hot core cooling to the state's colour at the
 *   edge) on the near-black night tile. On a light surface it is a dark chip; never a bare white
 *   ghost on a light page, never on an indigo slab (indigo is the app's accent for actions).
 * - The state is a colour: cyan at rest, green listening, violet reaching, amber for a yes, grey
 *   with a red stroke when not listening. With a ring, one arc of it is lit in that colour (the
 *   whole ring while speaking).
 * - The size ladder (`ladder`): the ring of ticks from 128 px (under that it is mush in a dock;
 *   a larger lit Wisp fills the tile instead), a blurred bloom from 48, the mitts from 32,
 *   eyes snapped to whole pixels under 32, larger eyes under 24.
 * - One-colour drawings (`monoSvg`, `traySvg`, the Android monochrome layer) for the tray at
 *   rest, forced colours and print. Nothing still has a smile or stick arms.
 *
 * Every drawing is on a 100-unit tile and told its real pixel size, so hairlines are whole
 * pixels. Plain TypeScript with only figure.ts imported: `packages/ui/scripts/marks.mjs`
 * writes the PNG, ICO and WebP files from it with Node and Chrome.
 */
import { EYES, type Eye, hemPath, MITTS_FROM, mittPaths, n, SMALL_EYES_UNDER, wispPath } from "./figure";

export type MarkState = "idle" | "listening" | "reaching" | "speaking" | "waiting" | "off";

/** Each state's name and light. `idle` is Ghost at rest; `off` is Not listening (ADR 0057). */
export const MARK_STATES: Record<MarkState, { name: string; tint: string }> = {
  idle: { name: "Ready", tint: "#5fe3ff" },
  listening: { name: "Listening", tint: "#4dffb8" },
  reaching: { name: "Reaching", tint: "#a98bff" },
  speaking: { name: "Speaking", tint: "#5fe3ff" },
  waiting: { name: "Waiting for a yes", tint: "#ffb547" },
  off: { name: "Not listening", tint: "#7d8796" },
};

/** The night tile's colour, and the light's ink (the body's cool white). */
export const NIGHT = "#05060a";
export const INK = "#eafcff";
/** The eyes: the night showing through. */
export const EYE_INK = "#06121a";
/** Not listening's stroke (the tray's language since ADR 0057). */
export const OFF_RED = "#ff4d5e";
const OFF_LIGHT = "#9aa3b2";

/** Where each state's arc sits on the ring, in degrees (0 is right, -90 up). */
export const ARCS: Record<MarkState, [number, number]> = {
  idle: [-162, -102],
  listening: [-120, -60],
  reaching: [-40, 40],
  speaking: [0, 0],
  waiting: [60, 120],
  off: [0, 0],
};

export interface Ladder {
  /** The ring of ticks around Wisp (the app icon at 128 px and up). */
  ticks: boolean;
  /** A blurred bloom behind the body (else a halo stroke). */
  bloom: boolean;
  mitts: boolean;
  /** Eyes snapped to whole pixels. */
  pixelEyes: boolean;
  /** The small drawing's larger eyes. */
  smallEyes: boolean;
  /** Film grain over the tile's light, the streak through the eyes. */
  grain: boolean;
  streak: boolean;
}

/** The levels of detail by the pixel size the mark is drawn at (docs/design-system.md, The size ladder). */
export function ladder(px: number): Ladder {
  return {
    ticks: px >= 128,
    bloom: px >= 48,
    mitts: px >= MITTS_FROM,
    pixelEyes: px < 32,
    smallEyes: px < SMALL_EYES_UNDER,
    grain: px >= 96,
    streak: px >= 128,
  };
}

/** The ring: `ticks` (from 128), `arc` (a hairline and the lit arc: in-app chips, 32 to 127), `none`. */
export type RingMode = "auto" | "ticks" | "arc" | "none";

export interface MarkOptions {
  state?: MarkState;
  ring?: RingMode;
  /** The tile: true (the mark), false (the lit figure alone, on whatever is behind). */
  tile?: boolean;
  /** Override the light's colour. */
  tint?: string;
  /** The speaking ring's loudness 0..1, and the ring's turn in degrees. */
  level?: number;
  turn?: number;
  /** Override the arc, in degrees. */
  arc?: [number, number];
  /** Space round the tile, in tile units (an app icon's margin). */
  inset?: number;
  /** A prefix for the SVG's ids, unique per page when several marks are inlined in one document. */
  id?: string;
}

let uid = 0;
let prefix = "gm";
const id = (p: string) => `${prefix}${p}${++uid}`;

const svg = (px: number, inner: string, defs = "", view = "0 0 100 100") =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="${view}">${defs ? `<defs>${defs}</defs>` : ""}${inner}</svg>`;

/** A small deterministic noise, so the lit arc's ticks look like a voice and not a ramp. */
const jitter = (i: number) => {
  const x = Math.sin(i * 12.9898 + 4.1) * 43758.5453;
  return x - Math.floor(x);
};

/** The night tile: near-black, the state's light rising from behind Wisp, a rim the light catches. */
function tile(px: number, tint: string, lift: number): { defs: string; inner: string } {
  const g = id("tg");
  const r = id("tr");
  const hair = 100 / px;
  const defs =
    `<radialGradient id="${g}" cx="50" cy="50" r="62" gradientUnits="userSpaceOnUse">` +
    `<stop offset="0" stop-color="${tint}" stop-opacity="${n(lift)}"/><stop offset="0.45" stop-color="${tint}" stop-opacity="${n(lift * 0.28)}"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></radialGradient>` +
    `<linearGradient id="${r}" x1="0" y1="0" x2="0" y2="100" gradientUnits="userSpaceOnUse">` +
    `<stop offset="0" stop-color="#ffffff" stop-opacity="0.16"/><stop offset="0.55" stop-color="${tint}" stop-opacity="0.18"/><stop offset="1" stop-color="${tint}" stop-opacity="0.6"/></linearGradient>`;
  const rim = Math.max(hair, 0.9);
  const inner =
    `<rect width="100" height="100" rx="22.5" fill="${NIGHT}"/>` +
    `<rect width="100" height="100" rx="22.5" fill="url(#${g})"/>` +
    `<rect x="${n(rim / 2)}" y="${n(rim / 2)}" width="${n(100 - rim)}" height="${n(100 - rim)}" rx="${n(22.5 - rim / 2)}" fill="none" stroke="url(#${r})" stroke-width="${n(rim)}"/>`;
  return { defs, inner };
}

/** Film grain over the tile's light: breaks the 8-bit bands in a dark glow without being seen. */
function grain(px: number): { defs: string; use: string } {
  const f = id("gr");
  return {
    defs: `<filter id="${f}" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="${n(0.9 * (px / 100))}" numOctaves="2" stitchTiles="stitch"/><feColorMatrix values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 0.55 0"/><feComposite in2="SourceGraphic" operator="in"/></filter>`,
    use: `<rect width="100" height="100" rx="22.5" filter="url(#${f})" opacity="0.07" style="mix-blend-mode:overlay"/>`,
  };
}

/** The anamorphic streak through Wisp's eyes: the floating room's lens that isn't there. */
function streak(px: number, tint: string, y: number, strength: number): { defs: string; inner: string } {
  if (strength <= 0) return { defs: "", inner: "" };
  const g = id("sk");
  const h = Math.max(100 / px, 0.35);
  return {
    defs: `<linearGradient id="${g}" x1="0" x2="100" y1="0" y2="0" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="${tint}" stop-opacity="0"/><stop offset="0.5" stop-color="#ffffff" stop-opacity="${n(0.55 * strength)}"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></linearGradient>`,
    inner: `<rect x="2" y="${n(y - h / 2)}" width="96" height="${n(h)}" fill="url(#${g})"/>`,
  };
}

/**
 * The ring of ticks (the floating room's instrument): every tenth longer, an arc of them lit in
 * the state's colour. `arc` mode is the hairline circle and the lit arc as one stroke.
 */
function ring(px: number, tint: string, mode: "ticks" | "arc", o: { R: number; arc: [number, number]; level?: number; turn?: number; full?: number }): string {
  const cx = 50;
  const cy = 50;
  const { R, arc } = o;
  const hair = 100 / px;
  if (mode === "arc") {
    const a0 = (arc[0] * Math.PI) / 180;
    const a1 = (arc[1] * Math.PI) / 180;
    const base = `<circle cx="${cx}" cy="${cy}" r="${R}" fill="none" stroke="${INK}" stroke-opacity="0.3" stroke-width="${n(hair)}"/>`;
    if (o.full) return base + `<circle cx="${cx}" cy="${cy}" r="${R}" fill="none" stroke="${tint}" stroke-width="${n(hair * 2)}" stroke-opacity="${n(0.5 + 0.5 * o.full)}"/>`;
    if (arc[0] === arc[1]) return base;
    const p0 = [cx + R * Math.cos(a0), cy + R * Math.sin(a0)];
    const p1 = [cx + R * Math.cos(a1), cy + R * Math.sin(a1)];
    return base + `<path d="M${n(p0[0])} ${n(p0[1])}A${R} ${R} 0 0 1 ${n(p1[0])} ${n(p1[1])}" fill="none" stroke="${tint}" stroke-width="${n(hair * 2)}" stroke-linecap="round"/>`;
  }
  const turn = o.turn ?? 0;
  const level = o.level ?? 1;
  const full = o.full ?? 0;
  const count = px >= 512 ? 120 : 72;
  const w = px >= 512 ? 0.32 : Math.max(hair, 0.5);
  let out = `<circle cx="${cx}" cy="${cy}" r="${n(R - 1.4)}" fill="none" stroke="${tint}" stroke-opacity="0.22" stroke-width="${n(Math.max(hair, 0.25))}"/>`;
  const span = (arc[1] - arc[0] + 360) % 360;
  for (let i = 0; i < count; i++) {
    const deg = (i / count) * 360 - 180 + turn;
    const a = (deg * Math.PI) / 180;
    const rel = (((deg - arc[0]) % 360) + 360) % 360;
    const lit = full > 0 ? 1 : span > 0 && rel <= span ? 1 : 0;
    const major = i % 10 === 0;
    const base = major ? 3.2 : 1.8;
    const amp = lit ? (full > 0 ? full * (0.4 + 0.6 * jitter(i + Math.round(turn))) : Math.sin((rel / span) * Math.PI) * (0.5 + 0.5 * jitter(i))) * level : 0;
    const len = base + amp * 5.5;
    const col = lit ? tint : INK;
    const op = lit ? 0.95 : major ? 0.6 : 0.36;
    out += `<line x1="${n(cx + R * Math.cos(a))}" y1="${n(cy + R * Math.sin(a))}" x2="${n(cx + (R + len) * Math.cos(a))}" y2="${n(cy + (R + len) * Math.sin(a))}" stroke="${col}" stroke-opacity="${op}" stroke-width="${n(lit ? w * 1.25 : w)}" stroke-linecap="round"/>`;
  }
  // The outer dashed hairline of the floating room's shader.
  const R2 = R + 9.2;
  const c = 2 * Math.PI * R2;
  out += `<circle cx="${cx}" cy="${cy}" r="${n(R2)}" fill="none" stroke="${INK}" stroke-opacity="0.1" stroke-width="${n(Math.max(hair, 0.22))}" stroke-dasharray="${n(c / 48)} ${n(c / 48)}"/>`;
  return out;
}

/** Not listening: a red stroke corner to corner over a dark edge, crisp at every size. */
export function offStrike(px: number): string {
  const w = Math.max((100 / px) * 1.6, 3.2);
  return `<line x1="20" y1="80" x2="80" y2="20" stroke="${NIGHT}" stroke-width="${n(w * 2.2)}" stroke-linecap="round"/><line x1="20" y1="80" x2="80" y2="20" stroke="${OFF_RED}" stroke-width="${n(w)}" stroke-linecap="round"/>`;
}

/** The middle of the drawing on the 24 grid that a placement centres (round 2's: y 13). */
const MID = 13;
/** The drawing's height on the 24 grid that a placement's `h` measures. */
const FIG_H = 20;

/** Wisp's paths at rest for a size: the body, and the mitts from 32 px. */
export function figurePaths(px: number): string[] {
  return [hemPath(0), ...(ladder(px).mitts ? mittPaths() : [])];
}

/**
 * Wisp lit from within, in tile units: centred at (cx, cy), `h` tile units for the figure's
 * 20 grid units. A white-hot core in the head cooling to the state's colour at the edge; from
 * 48 px a tight and a wide bloom behind it, under that a halo stroke (blur turns to dirt).
 */
function litFigure(px: number, tint: string, at: { cx: number; cy: number; h: number }, bloom = 1): { defs: string; inner: string } {
  const L = ladder(px);
  const k = at.h / FIG_H;
  const place = `translate(${n(at.cx)} ${n(at.cy)}) scale(${n(k)}) translate(-12 ${-MID})`;
  const hair = 100 / px / k;
  const paths = figurePaths(px);
  const fill = id("wf");
  let defs = `<radialGradient id="${fill}" cx="11.4" cy="9" r="13" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#ffffff"/><stop offset="0.38" stop-color="#f4feff"/><stop offset="0.72" stop-color="${INK}"/><stop offset="1" stop-color="${tint}"/></radialGradient>`;
  let under = "";
  if (L.bloom && bloom > 0) {
    const b1 = id("b");
    const b2 = id("b");
    defs += `<filter id="${b1}" x="-60%" y="-60%" width="220%" height="220%"><feGaussianBlur stdDeviation="0.55"/></filter><filter id="${b2}" x="-120%" y="-120%" width="340%" height="340%"><feGaussianBlur stdDeviation="2.4"/></filter>`;
    under =
      paths.map((d) => `<path d="${d}" fill="${tint}" opacity="${n(0.55 * bloom)}" filter="url(#${b2})"/>`).join("") +
      paths.map((d) => `<path d="${d}" fill="${tint}" opacity="${n(0.9 * bloom)}" filter="url(#${b1})"/>`).join("");
  } else {
    under = paths.map((d) => `<path d="${d}" fill="none" stroke="${tint}" stroke-opacity="${px >= 24 ? 0.38 : 0.5}" stroke-width="${n(hair * (px >= 24 ? 2.4 : 1.6))}" stroke-linejoin="round"/>`).join("");
  }
  const fills = paths.map((d) => `<path d="${d}" fill="url(#${fill})"/>`).join("");
  const eyes: readonly Eye[] = L.smallEyes ? EYES.small : EYES.regular;
  let pixelEyes = "";
  let eyesIn = "";
  if (L.pixelEyes) {
    // Snapped to whole pixels, both on one row: 2×3 at 16, 2×4 at 24.
    const u = 100 / px;
    const [w, hh] = px < 24 ? [2, 3] : [2, 4];
    const ty = at.cy + k * (eyes[0][1] - MID);
    pixelEyes = eyes
      .map(([gx]) => {
        const tx = at.cx + k * (gx - 12);
        return `<rect x="${n(Math.round(tx / u - w / 2) * u)}" y="${n(Math.round(ty / u - hh / 2) * u)}" width="${n(w * u)}" height="${n(hh * u)}" rx="${n(u * 0.6)}" fill="${EYE_INK}"/>`;
      })
      .join("");
  } else {
    eyesIn = eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${y}" rx="${rx}" ry="${ry}" fill="${EYE_INK}"/>`).join("");
  }
  return { defs, inner: `<g transform="${place}">${under}${fills}${eyesIn}</g>${pixelEyes}` };
}

/** Where the figure goes and how big, by ring mode and size. */
function placement(px: number, mode: "ticks" | "arc" | "none"): { h: number; R: number } {
  if (mode === "ticks") return { h: 45, R: 35 };
  if (mode === "arc") return { h: 48, R: 39 };
  return { h: px < 24 ? 80 : 72, R: 0 };
}

const resolveRing = (px: number, ring: RingMode = "auto"): "ticks" | "arc" | "none" => (ring === "auto" ? (ladder(px).ticks ? "ticks" : "none") : ring);

/**
 * The mark: Wisp lit on the night tile, in a state, at a pixel size. `ring: "auto"` follows the
 * ladder (the ring of ticks from 128 px); in-app chips that show the state pass `"arc"`.
 */
export function markSvg(px: number, o: MarkOptions = {}): string {
  uid = 0;
  prefix = o.id ?? "gm";
  const state = o.state ?? "idle";
  const off = state === "off";
  const tint = o.tint ?? MARK_STATES[state].tint;
  const light = off ? OFF_LIGHT : tint;
  const mode = resolveRing(px, o.ring);
  const { h, R } = placement(px, mode);
  const cy = 51;
  const withTile = o.tile !== false;
  const t = withTile ? tile(px, tint, off ? 0.06 : 0.2) : { defs: "", inner: "" };
  const w = litFigure(px, light, { cx: 50, cy, h }, off ? 0.3 : 1);
  let r = "";
  if (mode !== "none") {
    const arc = o.arc ?? ARCS[state];
    r = off
      ? ring(px, OFF_LIGHT, mode, { R, arc: [0, 0], level: 0 })
      : state === "speaking"
        ? ring(px, tint, mode, { R, arc, full: o.level ?? 0.8, turn: o.turn })
        : ring(px, tint, mode, { R, arc, level: o.level ?? 1, turn: o.turn });
  }
  const L = ladder(px);
  const s = withTile && L.streak ? streak(px, tint, cy - h * 0.17, off ? 0 : 0.8) : { defs: "", inner: "" };
  const gr = withTile && L.grain ? grain(px) : { defs: "", use: "" };
  // Not listening: the light dimmed under the stroke.
  const dim = off && withTile ? `<rect width="100" height="100" rx="22.5" fill="${NIGHT}" opacity="0.35"/>` : "";
  const strike = off ? offStrike(px) : "";
  const inner = t.inner + s.inner + r + w.inner + dim + gr.use + strike;
  const defs = t.defs + s.defs + w.defs + gr.defs;
  const inset = o.inset ?? 0;
  if (!inset) return svg(px, inner, defs);
  const k = (100 - 2 * inset) / 100;
  return svg(px, `<g transform="translate(${inset} ${inset}) scale(${n(k)})">${inner}</g>`, defs);
}

/**
 * One colour, no tile: the silhouette with the eyes cut out, the mitts from 32 px. For forced
 * colours, print, a themed icon. `color` may be `currentColor`.
 */
export function monoSvg(px: number, color = "currentColor"): string {
  const L = ladder(px);
  const h = px < 24 ? 86 : 80;
  const k = h / FIG_H;
  return svg(px, `<g transform="translate(50 51) scale(${n(k)}) translate(-12 ${-MID})"><path d="${wispPath({ mitts: L.mitts, small: L.smallEyes })}" fill="${color}"/></g>`);
}

/**
 * The tray: one colour at rest (the cool ink, with a thin dark edge so it holds on a light
 * panel as on GNOME's dark one), the state's colour in a session, grey with the red stroke when
 * not listening. No tile and no light: a tray icon is a glyph among glyphs.
 */
export function traySvg(px: number, state: MarkState | "rest" = "rest"): string {
  const L = ladder(px);
  const fill = state === "rest" ? INK : state === "off" ? OFF_LIGHT : MARK_STATES[state].tint;
  const h = 84;
  const k = h / FIG_H;
  const edge = n(((100 / px) * 2.2) / k);
  const d = wispPath({ mitts: L.mitts, small: L.smallEyes });
  const inner =
    `<g transform="translate(50 51) scale(${n(k)}) translate(-12 ${-MID})">` +
    `<path d="${d}" fill="none" stroke="${NIGHT}" stroke-opacity="0.9" stroke-width="${edge}" stroke-linejoin="round"/>` +
    `<path d="${d}" fill="${fill}" opacity="${state === "off" ? 0.85 : 1}"/></g>` +
    (state === "off" ? offStrike(px) : "");
  return svg(px, inner);
}

/**
 * Android's adaptive icon on its 108 canvas (the mask keeps the middle 72; only the 66 circle
 * is safe): `background` is the night and its light, `foreground` the lit Wisp inside the safe
 * circle, `monochrome` the one-colour drawing for themed icons.
 */
export function adaptiveSvg(layer: "background" | "foreground" | "monochrome", state: MarkState = "idle", px = 432): string {
  uid = 0;
  prefix = "ga";
  const tint = MARK_STATES[state].tint;
  if (layer === "background") {
    const g = id("ab");
    return svg(
      px,
      `<rect width="108" height="108" fill="${NIGHT}"/><rect width="108" height="108" fill="url(#${g})"/>`,
      `<radialGradient id="${g}" cx="54" cy="54" r="50" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="${tint}" stop-opacity="0.24"/><stop offset="0.5" stop-color="${tint}" stop-opacity="0.06"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></radialGradient>`,
      "0 0 108 108",
    );
  }
  const k = 46 / FIG_H;
  const place = `translate(54 55) scale(${n(k)}) translate(-12 ${-MID})`;
  if (layer === "monochrome") return svg(px, `<g transform="${place}"><path d="${wispPath()}" fill="#ffffff"/></g>`, "", "0 0 108 108");
  // The foreground: litFigure's drawing at 108 scale, no tile.
  const w = litFigure(px, tint, { cx: 54, cy: 55, h: 46 });
  return svg(px, w.inner, w.defs, "0 0 108 108");
}

/**
 * The wordmark: Inter 800 at −0.035 em, beside the mark at the text's cap height and a bit.
 * `theme` is the surface it sits on: on dark the word is the cool ink, on light the night. The
 * mark stays the dark tile on both. Text needs Inter where it is drawn (the apps load it; the
 * PNG script embeds it).
 */
export function wordmarkSvg(height = 64, theme: "dark" | "light" = "dark", o: { fontFace?: string; id?: string } = {}): string {
  const tileH = height;
  const fs = height * 0.62;
  const gap = height * 0.28;
  const textW = fs * 2.62;
  const W = Math.ceil(tileH + gap + textW);
  const inner = markSvg(Math.round(height), { id: o.id ?? "gw" })
    .replace(/^<svg[^>]*>/, `<svg x="0" y="0" width="${tileH}" height="${tileH}" viewBox="0 0 100 100">`);
  const color = theme === "dark" ? INK : NIGHT;
  const style = o.fontFace ? `<style>${o.fontFace}</style>` : "";
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${Math.ceil(tileH)}" viewBox="0 0 ${W} ${n(tileH)}">${style}` +
    inner +
    `<text x="${n(tileH + gap)}" y="${n(tileH / 2 + fs * 0.36)}" font-family="'Inter Variable', Inter, system-ui, sans-serif" font-weight="800" font-size="${n(fs)}" letter-spacing="${n(-0.035 * fs)}" fill="${color}">Ghost</text></svg>`
  );
}

/** Every file the apps ship, by name, with its SVG and pixel size: what the script writes. */
export interface MarkFile {
  name: string;
  svg: string;
  px: number;
}

/** The desktop app's icon files (Tauri's bundle, the window, the .deb's hicolor sizes). */
export function desktopIconFiles(): MarkFile[] {
  // Full bleed: GNOME's dock draws the tile at the size its neighbours are (checked on a dock mock).
  const at = (name: string, px: number): MarkFile => ({ name, px, svg: markSvg(px) });
  return [
    at("16x16.png", 16),
    at("24x24.png", 24),
    at("32x32.png", 32),
    at("48x48.png", 48),
    at("64x64.png", 64),
    at("128x128.png", 128),
    at("128x128@2x.png", 256),
    at("256x256.png", 256),
    at("icon.png", 512),
  ];
}

/** The tray's icons, by state (shell's tray.rs embeds them). */
export function trayIconFiles(px = 32): MarkFile[] {
  const states: (MarkState | "rest")[] = ["rest", "listening", "reaching", "speaking", "waiting", "off"];
  return states.map((s) => ({ name: `tray-${s}.png`, px, svg: traySvg(px, s) }));
}

/**
 * Android's status-bar icon (a notification's small icon): one colour, transparent, on the 24dp
 * grid, the plain hem and the larger eyes (the drawing under 24 px: the mitts only widen the
 * blob there), centred in the 22dp live area. Android paints it white or the system's tint, so
 * it is the silhouette alone. The nonzero rule, as everywhere the body is drawn.
 */
const STATUS_PATH = () => wispPath({ mitts: false, small: true });
/** The drawing sits from y 3 to 20.3 on the 24 grid: this centres it. */
const STATUS_DY = 12 - (3 + 20.3) / 2;

/** The status-bar icon as an SVG, for review sheets. */
export function notificationSvg(): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path transform="translate(0 ${n(STATUS_DY)})" d="${STATUS_PATH()}" fill="#ffffff"/></svg>`;
}

/** The status-bar icon as an Android vector drawable (`res/drawable/ghost_link.xml`). */
export function notificationVector(): string {
  return `<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by packages/ui/scripts/marks.mjs mobile from mark.ts (notificationVector); do not edit.
     Wisp at rest, the plain hem and the larger eyes (the drawing under 24 px), one colour: the
     status-bar icon for the connection and for what Ghost puts in the shade. Nonzero fill: the
     eyes wind the other way, so they are cut out. -->
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="24"
    android:viewportHeight="24">
  <group android:translateY="${n(STATUS_DY)}">
    <path
        android:fillColor="@android:color/white"
        android:fillType="nonZero"
        android:pathData="${STATUS_PATH()}" />
  </group>
</vector>
`;
}
