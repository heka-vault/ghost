/**
 * The mark concepts, November 2026: every drawing on the sheet, as SVG strings. A concept,
 * not the product: nothing here is imported by an app, and the shipped icons are untouched.
 *
 * Wisp's body is the brand's own curve (`packages/ui/src/wisp.ts`, `body(s)`), copied here so
 * the concept runs with plain Node and no build. Every mark is drawn on a 100-unit tile and
 * told its real pixel size, so hairlines are whole pixels and each size gets its own level of
 * detail (`lod`), not a scaled-down master.
 */

export const STATES = {
  idle: { name: "Ready", tint: "#5fe3ff" },
  listening: { name: "Listening", tint: "#4dffb8" },
  reaching: { name: "Reaching", tint: "#a98bff" },
  speaking: { name: "Speaking", tint: "#5fe3ff" },
  waiting: { name: "Waiting for a yes", tint: "#ffb547" },
  off: { name: "Not listening", tint: "#7d8796" },
};

const NIGHT = "#05060a";
const INK = "#eafcff";

const n = (v) => (Math.abs(v) < 1e-4 ? 0 : Math.round(v * 1000) / 1000);

// ---------------------------------------------------------------- Wisp, the character

/** Wisp's body from wisp.ts: a round head tapering into a tail that curls right; `s` swings the tail. */
export function body(s = 0) {
  const tx = 18.6 + s * 1.7;
  const ty = 22.9 - s * 0.9;
  return (
    `M5 10A7 7 0 0 1 19 10C19 14.5 17.2 17.6 16.2 19.8` +
    `C${n(15.6 + s * 0.4)} 21.2 ${n(16.8 + s * 0.9)} ${n(22.6 - s * 0.5)} ${n(tx)} ${n(ty)}` +
    `C${n(16 + s * 0.6)} ${n(23.6 - s * 0.3)} 12.6 22.6 10.6 21.2C6.8 18.8 5 15 5 10Z`
  );
}

const ellipse = (cx, cy, rx, ry) =>
  `M${n(cx - rx)} ${n(cy)}a${n(rx)} ${n(ry)} 0 1 0 ${n(2 * rx)} 0a${n(rx)} ${n(ry)} 0 1 0 ${n(-2 * rx)} 0Z`;

/** Today's brand glyph (GHOST_PATH): Wisp at rest, upright, eyes cut out. */
export const TODAY = { s: 0, rot: 0, eyes: { lx: 9.5, rx_: 14.5, y: 10.3, rx: 1.25, ry: 1.65 } };
export const TODAY_SMALL = { s: 0, rot: 0, eyes: { lx: 9.5, rx_: 14.5, y: 10.4, rx: 1.4, ry: 1.8 } };

/**
 * The redraw: the still pose that keeps the floating Wisp's presence. The same body, caught
 * mid-drift: the tail flicked out (s 0.6, the mascot's own swing), the whole figure leaning
 * into the drift (-7°), eyes taller and a touch higher, looking slightly up at you. No mouth,
 * no arms: those are gestures, and gestures belong to the live mascot.
 */
export const STILL = { s: 0.6, rot: -7, eyes: { lx: 9.55, rx_: 14.45, y: 9.75, rx: 1.18, ry: 2.05 } };
/** Under 32px: less lean (a vertical edge stays crisp), bigger eyes that still land on pixels. */
export const STILL_SMALL = { s: 0.45, rot: -4, eyes: { lx: 9.35, rx_: 14.65, y: 9.9, rx: 1.55, ry: 2.3 } };

/** The figure as one evenodd path (eyes cut out) and the transform that poses it, on the 24 grid. */
export function figure(p) {
  return {
    d: body(p.s) + ellipse(p.eyes.lx, p.eyes.y, p.eyes.rx, p.eyes.ry) + ellipse(p.eyes.rx_, p.eyes.y, p.eyes.rx, p.eyes.ry),
    body: body(p.s),
    eyes: [
      [p.eyes.lx, p.eyes.y, p.eyes.rx, p.eyes.ry],
      [p.eyes.rx_, p.eyes.y, p.eyes.rx, p.eyes.ry],
    ],
    pose: p.rot ? `rotate(${p.rot} 12 13)` : "",
  };
}

/** Where a 24-grid figure sits on the 100 tile: centred at (cx, cy), `h` tile units tall. */
const place = (cx, cy, h) => {
  const k = h / 20.6; // the body runs y 3 to 23.6
  return `translate(${n(cx)} ${n(cy)}) scale(${n(k)}) translate(-12 -13.3)`;
};

/** A grid point through the pose and the placement, in tile units. */
function toTile(gx, gy, rot, at) {
  const a = (rot * Math.PI) / 180;
  const dx = gx - 12;
  const dy = gy - 13;
  const rx = 12 + dx * Math.cos(a) - dy * Math.sin(a);
  const ry = 13 + dx * Math.sin(a) + dy * Math.cos(a);
  const k = at.h / 20.6;
  return [at.cx + k * (rx - 12), at.cy + k * (ry - 13.3)];
}

// ---------------------------------------------------------------- shared pieces

let uid = 0;
const id = (p) => `${p}${++uid}`;

const svg = (px, inner, defs = "", view = "0 0 100 100") =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="${view}">${defs ? `<defs>${defs}</defs>` : ""}${inner}</svg>`;

/** Film grain over a gradient: breaks the 8-bit bands in a dark glow without being seen. */
function grain(px) {
  if (px < 96) return { defs: "", use: "" };
  const f = id("grain");
  return {
    defs: `<filter id="${f}" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="${n(0.9 * (px / 100))}" numOctaves="2" stitchTiles="stitch"/><feColorMatrix values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 0.55 0"/></filter>`,
    use: `<rect width="100" height="100" rx="22.5" filter="url(#${f})" opacity="0.07" style="mix-blend-mode:overlay"/>`,
  };
}

/** The night tile: near-black, the state's light rising from behind Wisp, a rim the light catches. */
function tile(px, tint, { lift = 0.2, cy = 50 } = {}) {
  const g = id("tg");
  const r = id("tr");
  const hair = 100 / px; // one device pixel in tile units
  const defs =
    `<radialGradient id="${g}" cx="50" cy="${cy}" r="62" gradientUnits="userSpaceOnUse">` +
    `<stop offset="0" stop-color="${tint}" stop-opacity="${lift}"/><stop offset="0.45" stop-color="${tint}" stop-opacity="${n(lift * 0.28)}"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></radialGradient>` +
    `<linearGradient id="${r}" x1="0" y1="0" x2="0" y2="100" gradientUnits="userSpaceOnUse">` +
    `<stop offset="0" stop-color="#ffffff" stop-opacity="0.16"/><stop offset="0.55" stop-color="${tint}" stop-opacity="0.18"/><stop offset="1" stop-color="${tint}" stop-opacity="0.6"/></linearGradient>`;
  const rim = Math.max(hair, 0.9);
  const inner =
    `<rect width="100" height="100" rx="22.5" fill="${NIGHT}"/>` +
    `<rect width="100" height="100" rx="22.5" fill="url(#${g})"/>` +
    `<rect x="${n(rim / 2)}" y="${n(rim / 2)}" width="${n(100 - rim)}" height="${n(100 - rim)}" rx="${n(22.5 - rim / 2)}" fill="none" stroke="url(#${r})" stroke-width="${n(rim)}"/>`;
  return { defs, inner };
}

/**
 * Wisp lit from within: a white-hot core in the head cooling to the state's colour at the
 * edge, a tight bloom and a wide one behind it. Under 48px blur turns to dirt, so the bloom is
 * a soft halo stroke instead, and under 24px a single crisp halo.
 */
function litWisp(px, tint, p, at, { bloom = 1, blink = 0 } = {}) {
  const f = figure(p);
  const k = 20.6 / at.h; // grid units per tile unit, inverse of place()
  const hair = (100 / px) * k; // one device pixel, in grid units
  const fill = id("wf");
  let defs =
    `<radialGradient id="${fill}" cx="11.6" cy="9.4" r="12.5" gradientUnits="userSpaceOnUse">` +
    `<stop offset="0" stop-color="#ffffff"/><stop offset="0.38" stop-color="#f4feff"/><stop offset="0.72" stop-color="${INK}"/><stop offset="1" stop-color="${tint}"/></radialGradient>`;
  let under = "";
  if (px >= 48 && bloom > 0) {
    const b1 = id("b1");
    const b2 = id("b2");
    defs +=
      `<filter id="${b1}" x="-60%" y="-60%" width="220%" height="220%"><feGaussianBlur stdDeviation="${n(0.55)}"/></filter>` +
      `<filter id="${b2}" x="-120%" y="-120%" width="340%" height="340%"><feGaussianBlur stdDeviation="${n(2.4)}"/></filter>`;
    under =
      `<path d="${f.body}" fill="${tint}" opacity="${n(0.55 * bloom)}" filter="url(#${b2})"/>` +
      `<path d="${f.body}" fill="${tint}" opacity="${n(0.9 * bloom)}" filter="url(#${b1})"/>`;
  } else if (px >= 24) {
    under = `<path d="${f.body}" fill="none" stroke="${tint}" stroke-opacity="0.38" stroke-width="${n(hair * 2.4)}" stroke-linejoin="round"/>`;
  } else {
    under = `<path d="${f.body}" fill="none" stroke="${tint}" stroke-opacity="0.5" stroke-width="${n(hair * 1.6)}" stroke-linejoin="round"/>`;
  }
  // The eyes are the night showing through: drawn dark over the body, not cut, so the bloom
  // behind never fills them. Under 32px they snap to whole pixels (2×3 at 16, 2×4 at 24).
  const eyeFill = "#06121a";
  let eyesEl = "";
  if (px < 32) {
    const u = 100 / px;
    const [w, hh] = px < 24 ? [2, 3] : [2, 4];
    // Both eyes on one row of pixels: at this size a lean shows in the body, never as a squint.
    const ys = f.eyes.map(([gx, gy]) => toTile(gx, gy, p.rot, at)[1]);
    const ty = (ys[0] + ys[1]) / 2;
    eyesEl = f.eyes
      .map(([gx, gy]) => {
        const [tx] = toTile(gx, gy, p.rot, at);
        const x0 = Math.round(tx / u - w / 2) * u;
        const y0 = Math.round(ty / u - hh / 2) * u;
        return `<rect x="${n(x0)}" y="${n(y0)}" width="${n(w * u)}" height="${n(hh * u)}" rx="${n(u * 0.6)}" fill="${eyeFill}"/>`;
      })
      .join("");
    const inner = `<g transform="${place(at.cx, at.cy, at.h)}"><g transform="${f.pose}">${under}<path d="${f.body}" fill="url(#${fill})"/></g></g>${eyesEl}`;
    return { defs, inner };
  }
  const open = Math.max(0.12, 1 - blink);
  eyesEl = f.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${n(y + ry * (1 - open) * 0.7)}" rx="${rx}" ry="${n(ry * open)}" fill="${eyeFill}"/>`).join("");
  const inner = `<g transform="${place(at.cx, at.cy, at.h)}"><g transform="${f.pose}">${under}<path d="${f.body}" fill="url(#${fill})"/>${eyesEl}</g></g>`;
  return { defs, inner };
}

/** The anamorphic streak through Wisp's eyes: the floating room's lens that isn't there. */
function streak(px, tint, y, strength = 1) {
  if (px < 128) return { defs: "", inner: "" };
  const g = id("sk");
  const defs = `<linearGradient id="${g}" x1="0" x2="100" y1="0" y2="0" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="${tint}" stop-opacity="0"/><stop offset="0.5" stop-color="#ffffff" stop-opacity="${n(0.55 * strength)}"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></linearGradient>`;
  const h = Math.max(100 / px, 0.35);
  return { defs, inner: `<rect x="2" y="${n(y - h / 2)}" width="96" height="${n(h)}" fill="url(#${g})"/>` };
}

/** A small deterministic noise, so the lit arc's ticks look like a voice and not a ramp. */
const jitter = (i) => {
  const x = Math.sin(i * 12.9898 + 4.1) * 43758.5453;
  return x - Math.floor(x);
};

/**
 * The instrument: the floating room's ring of ticks, every tenth longer, with an arc of them
 * lit in the state's colour (the voice in that direction, or where Ghost is reaching). The
 * count and the drawing depend on the size: 120 ticks at 512 and up, 72 at 128, 36 at 48, a
 * hairline and four ticks at 32, nothing under that.
 */
function ring(px, tint, { cx = 50, cy = 50, R = 37, arc = [-162, -102], level = 1, turn = 0, full = 0, reveal = 1 } = {}) {
  if (px < 32) return "";
  const hair = 100 / px;
  if (px < 48) {
    // A hairline circle and the lit arc as one stroke.
    const a0 = (arc[0] * Math.PI) / 180;
    const a1 = (arc[1] * Math.PI) / 180;
    const p0 = [cx + R * Math.cos(a0), cy + R * Math.sin(a0)];
    const p1 = [cx + R * Math.cos(a1), cy + R * Math.sin(a1)];
    return (
      `<circle cx="${cx}" cy="${cy}" r="${R}" fill="none" stroke="${INK}" stroke-opacity="0.32" stroke-width="${n(hair)}"/>` +
      `<path d="M${n(p0[0])} ${n(p0[1])}A${R} ${R} 0 0 1 ${n(p1[0])} ${n(p1[1])}" fill="none" stroke="${tint}" stroke-width="${n(hair * 2)}" stroke-linecap="round"/>`
    );
  }
  const count = px >= 512 ? 120 : px >= 128 ? 72 : 36;
  const w = px >= 512 ? 0.32 : Math.max(hair, 0.5);
  let out = `<circle cx="${cx}" cy="${cy}" r="${n(R - (px >= 128 ? 1.4 : 1.6))}" fill="none" stroke="${tint}" stroke-opacity="0.22" stroke-width="${n(Math.max(hair, 0.25))}"/>`;
  for (let i = 0; i < count; i++) {
    if ((((i / count) + 0.75) % 1) > reveal) continue;
    const deg = (i / count) * 360 - 180 + turn;
    const a = (deg * Math.PI) / 180;
    let rel = ((deg - arc[0]) % 360 + 360) % 360;
    const span = (arc[1] - arc[0] + 360) % 360;
    const lit = full > 0 ? 1 : span > 0 && rel <= span ? 1 : 0;
    const major = i % 10 === 0 && px >= 128;
    const base = px >= 128 ? (major ? 3.2 : 1.8) : 2.6;
    const amp = lit ? (full > 0 ? full * (0.4 + 0.6 * jitter(i + Math.round(turn))) : Math.sin((rel / span) * Math.PI) * (0.5 + 0.5 * jitter(i))) * level : 0;
    const len = base + amp * (px >= 128 ? 5.5 : 4);
    const x0 = cx + R * Math.cos(a);
    const y0 = cy + R * Math.sin(a);
    const x1 = cx + (R + len) * Math.cos(a);
    const y1 = cy + (R + len) * Math.sin(a);
    const col = lit ? tint : INK;
    const op = lit ? 0.95 : major ? 0.6 : 0.36;
    out += `<line x1="${n(x0)}" y1="${n(y0)}" x2="${n(x1)}" y2="${n(y1)}" stroke="${col}" stroke-opacity="${op}" stroke-width="${n(lit ? w * 1.25 : w)}" stroke-linecap="${px >= 128 ? "round" : "butt"}"/>`;
  }
  if (px >= 128) {
    // The outer dashed hairline of the shader (R + 0.085), only where there is room for it.
    const R2 = R + 9.2;
    const c = 2 * Math.PI * R2;
    out += `<circle cx="${cx}" cy="${cy}" r="${n(R2)}" fill="none" stroke="${INK}" stroke-opacity="0.1" stroke-width="${n(Math.max(hair, 0.22))}" stroke-dasharray="${n(c / 48)} ${n(c / 48)}"/>`;
  }
  return out;
}

// ---------------------------------------------------------------- the three directions

/**
 * I. Lit. Wisp as a light source on a night tile: no slab of indigo, no smile, no arms. The
 * state colour is the light behind it and the rim of the tile.
 */
export function lit(px, state = "idle", o = {}) {
  uid = 0;
  const tint = STATES[state].tint;
  const p = px < 32 ? STILL_SMALL : STILL;
  const h = px < 24 ? 80 : px < 48 ? 74 : 64;
  const t = tile(px, tint, { lift: state === "off" ? 0.08 : 0.24, cy: 46 });
  const w = litWisp(px, state === "off" ? "#9aa3b2" : tint, p, { cx: 51, cy: px < 24 ? 51.5 : 52, h }, { bloom: state === "off" ? 0.3 : 1 });
  const s = streak(px, tint, 52 - h * 0.17, state === "off" ? 0 : 1);
  const gr = grain(px);
  const strike = state === "off" ? offStrike(px) : "";
  const dim = state === "off" ? `<rect width="100" height="100" rx="22.5" fill="${NIGHT}" opacity="0.35"/>` : "";
  return svg(px, t.inner + s.inner + w.inner + dim + gr.use + strike, t.defs + s.defs + w.defs + gr.defs + (o.defs ?? ""));
}

/**
 * II. The instrument. Lit Wisp inside the floating room's ring of ticks: the ring is the
 * device, the lit arc is the state. Under 32px the ring cannot be drawn honestly and the mark
 * becomes Lit's small drawing: one system, two levels of detail.
 */
export function instrument(px, state = "idle", o = {}) {
  if (px < 32) return lit(px, state, o);
  uid = 0;
  const tint = o.tint ?? STATES[state].tint;
  const off = state === "off";
  const t = tile(px, tint, { lift: (off ? 0.06 : 0.2) * (o.light ?? 1), cy: 50 });
  const h = px >= 128 ? 45 : px >= 48 ? 46 : 50;
  const R = px >= 128 ? 35 : px >= 48 ? 36 : 39;
  const wy = 51 + (o.dy ?? 0);
  const w = litWisp(px, off ? "#9aa3b2" : tint, px < 48 ? STILL_SMALL : STILL, { cx: 50.5, cy: wy, h: h * (o.scale ?? 1) }, { bloom: (off ? 0.3 : 1) * (o.light ?? 1), blink: o.blink ?? 0 });
  const arcs = { idle: [-162, -102], listening: [-120, -60], reaching: [-40, 40], speaking: [0, 0], waiting: [60, 120], off: [0, 0] };
  const r = off
    ? ring(px, "#9aa3b2", { R, arc: [0, 0], level: 0, turn: o.turn ?? 0 })
    : state === "speaking"
      ? ring(px, tint, { R, full: o.level ?? 0.8, turn: o.turn ?? 0 })
      : ring(px, tint, { R, arc: o.arc ?? arcs[state], level: o.level ?? 1, turn: o.turn ?? 0, reveal: o.reveal ?? 1 });
  const s = streak(px, tint, wy - h * 0.17, off ? 0 : 0.8 * (o.light ?? 1));
  const gr = grain(px);
  const strike = off ? offStrike(px) : "";
  return svg(px, t.inner + s.inner + r + w.inner + (o.extra ?? "") + gr.use + strike, t.defs + s.defs + w.defs + gr.defs);
}

/**
 * III. Eyes in the dark. Wisp's body is the night itself, found only by a rim of light; the
 * eyes are the light. The most cinematic of the three, and the least like a friend.
 */
export function eyes(px, state = "idle") {
  uid = 0;
  const tint = STATES[state].tint;
  const off = state === "off";
  const p = px < 32 ? STILL_SMALL : STILL;
  const h = px < 24 ? 82 : px < 48 ? 76 : 66;
  const at = { cx: 51, cy: 52, h };
  const f = figure(p);
  const k = 20.6 / h;
  const hair = (100 / px) * k;
  const t = tile(px, tint, { lift: off ? 0.05 : 0.12, cy: 40 });
  const rimG = id("rg");
  const eyeG = id("eg");
  let defs =
    `<linearGradient id="${rimG}" x1="6" y1="4" x2="18" y2="23" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="${tint}" stop-opacity="${off ? 0.35 : 0.95}"/><stop offset="0.55" stop-color="${tint}" stop-opacity="0.25"/><stop offset="1" stop-color="${tint}" stop-opacity="0.05"/></linearGradient>` +
    `<radialGradient id="${eyeG}"><stop offset="0" stop-color="#ffffff"/><stop offset="0.55" stop-color="#ffffff"/><stop offset="1" stop-color="${off ? "#9aa3b2" : tint}"/></radialGradient>`;
  let glow = "";
  if (px >= 48 && !off) {
    const b = id("eb");
    const b2 = id("eb2");
    defs += `<filter id="${b}" x="-300%" y="-300%" width="700%" height="700%"><feGaussianBlur stdDeviation="0.9"/></filter><filter id="${b2}" x="-400%" y="-400%" width="900%" height="900%"><feGaussianBlur stdDeviation="2.6"/></filter>`;
    glow = f.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${y}" rx="${rx * 1.6}" ry="${ry * 1.3}" fill="${tint}" opacity="0.5" filter="url(#${b2})"/><ellipse cx="${x}" cy="${y}" rx="${rx * 1.1}" ry="${ry * 1.05}" fill="${tint}" filter="url(#${b})"/>`).join("");
  }
  const rimW = px < 24 ? hair * 1.4 : px < 48 ? hair * 1.5 : Math.max(hair * 1.5, 0.32);
  const eyeEls = f.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${y}" rx="${n(rx * (px < 24 ? 0.95 : 0.9))}" ry="${n(ry * 0.98)}" fill="url(#${eyeG})"/>`).join("");
  const inner =
    t.inner +
    `<g transform="${place(at.cx, at.cy, at.h)}"><g transform="${f.pose}">` +
    `<path d="${f.body}" fill="#0b1018"/>` +
    `<path d="${f.body}" fill="none" stroke="url(#${rimG})" stroke-width="${n(rimW)}" stroke-linejoin="round"/>` +
    glow +
    eyeEls +
    `</g></g>` +
    (off ? offStrike(px) : "");
  const gr = grain(px);
  return svg(px, inner + gr.use, t.defs + defs + gr.defs);
}

/** Not listening: the tray's existing language (ADR 0057), a red stroke corner to corner, crisp at every size. */
function offStrike(px) {
  const w = Math.max(100 / px * 1.6, 3.2);
  return `<line x1="20" y1="80" x2="80" y2="20" stroke="${NIGHT}" stroke-width="${n(w * 2.2)}" stroke-linecap="round"/><line x1="20" y1="80" x2="80" y2="20" stroke="#ff4d5e" stroke-width="${n(w)}" stroke-linecap="round"/>`;
}

// ---------------------------------------------------------------- one colour

/**
 * One colour, no tile: the tray's symbolic icon, Android's themed icon, print. The silhouette
 * with the eyes cut out; the instrument adds its ring of ticks from 48px, with the lit arc
 * drawn as longer ticks so it survives without colour.
 */
export function mono(dir, px, color = "currentColor") {
  uid = 0;
  const small = px < 32;
  const p = small ? STILL_SMALL : STILL;
  const f = figure(p);
  if (dir === "eyes") {
    const h = small ? 84 : 72;
    const hair = (100 / px) * (20.6 / h);
    const rimW = Math.max(hair * (small ? 1.5 : 1.6), small ? 0 : 0.7);
    return svg(px, `<g transform="${place(51, 52, h)}"><g transform="${f.pose}"><path d="${f.body}" fill="none" stroke="${color}" stroke-width="${n(rimW)}" stroke-linejoin="round"/>${f.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${y}" rx="${rx}" ry="${ry}" fill="${color}"/>`).join("")}</g></g>`);
  }
  if (dir === "instrument" && px >= 48) {
    const R = px >= 128 ? 38 : 39;
    const count = px >= 128 ? 72 : 36;
    const hair = 100 / px;
    let ticks = "";
    for (let i = 0; i < count; i++) {
      const deg = (i / count) * 360 - 180;
      const a = (deg * Math.PI) / 180;
      const rel = ((deg + 162) % 360 + 360) % 360;
      const lit = rel <= 60;
      const len = lit ? 3 + 4.5 * Math.sin((rel / 60) * Math.PI) * (0.5 + 0.5 * jitter(i)) : i % 10 === 0 && px >= 128 ? 3.4 : 2;
      ticks += `<line x1="${n(50 + R * Math.cos(a))}" y1="${n(50 + R * Math.sin(a))}" x2="${n(50 + (R + len) * Math.cos(a))}" y2="${n(50 + (R + len) * Math.sin(a))}" stroke="${color}" stroke-opacity="${lit ? 1 : 0.55}" stroke-width="${n(Math.max(hair * (lit ? 1.4 : 1), 0.6))}"/>`;
    }
    return svg(px, ticks + `<g transform="${place(50.5, 51, 50)}"><path transform="${f.pose}" d="${f.d}" fill-rule="evenodd" fill="${color}"/></g>`);
  }
  const h = small ? 86 : 78;
  return svg(px, `<g transform="${place(51, 51.5, h)}"><path transform="${f.pose}" d="${f.d}" fill-rule="evenodd" fill="${color}"/></g>`);
}

// ---------------------------------------------------------------- the character, drawn for the sheet

/** A bare figure on the 24 grid (no tile), for the character study. */
export function bare(p, { fill = INK, stroke = null, px = 240, view = "0 0 24 24" } = {}) {
  uid = 0;
  const f = figure(p);
  const s = stroke ? ` stroke="${stroke}" stroke-width="0.08" fill-opacity="0"` : "";
  return svg(px, `<g transform="${f.pose}"><path d="${f.d}" fill-rule="evenodd" fill="${fill}"${s}/></g>`, "", view);
}

/** The redraw over today's drawing, outlines only, for the character study. */
export function overlay(px = 360) {
  uid = 0;
  const a = figure(TODAY);
  const b = figure(STILL);
  return svg(
    px,
    `<g><path d="${a.d}" fill-rule="evenodd" fill="none" stroke="#9aa3b2" stroke-width="0.14" stroke-dasharray="0.4 0.3"/></g>` +
      `<g transform="${b.pose}"><path d="${b.d}" fill-rule="evenodd" fill="#5fe3ff" fill-opacity="0.1" stroke="#5fe3ff" stroke-width="0.16"/></g>` +
      `<circle cx="12" cy="13" r="0.18" fill="#ffb547"/>`,
    "",
    "1 1 22 24",
  );
}

/** The lit figure without a tile (rail, splash, lockups on the dark): glow on whatever is behind. */
export function litBare(px, state = "idle") {
  uid = 0;
  const tint = STATES[state].tint;
  const w = litWisp(px, tint, px < 32 ? STILL_SMALL : STILL, { cx: 50, cy: 51, h: px < 32 ? 86 : 74 });
  return svg(px, w.inner, w.defs);
}

// ---------------------------------------------------------------- Android adaptive layers (108dp)

/**
 * The adaptive icon's two layers on Android's 108 canvas: the mask keeps the middle 72 and
 * only the 66 circle is guaranteed. Background: the night and its light. Foreground: Wisp (and
 * for the instrument the ring, which sits inside the safe circle and so reads in every mask).
 */
export function adaptive(dir, layer, state = "idle") {
  uid = 0;
  const tint = STATES[state].tint;
  const px = 432;
  if (layer === "background") {
    const g = id("ab");
    return svg(px, `<rect width="108" height="108" fill="${NIGHT}"/><rect width="108" height="108" fill="url(#${g})"/>`, `<radialGradient id="${g}" cx="54" cy="54" r="50" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="${tint}" stop-opacity="${dir === "eyes" ? 0.12 : 0.24}"/><stop offset="0.5" stop-color="${tint}" stop-opacity="0.06"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></radialGradient>`, "0 0 108 108");
  }
  // Foreground: reuse the tile drawings' insides, scaled into the safe circle.
  const inner = (dir === "instrument" ? instrument(px, state) : dir === "eyes" ? eyes(px, state) : lit(px, state))
    .replace(/^<svg[^>]*>/, "")
    .replace(/<\/svg>$/, "");
  // Drop the tile itself (the first three rects) so only the figure (and ring) remain.
  const fg = inner.replace(/<rect width="100" height="100" rx="22.5" fill="#05060a"\/><rect width="100" height="100" rx="22.5" fill="url\(#[^)]+\)"\/><rect [^>]*stroke="url\(#[^)]+\)"[^>]*\/>/, "").replace(/<rect width="100" height="100" rx="22.5" filter="url\(#grain\d+\)"[^>]*\/>/, "");
  const k = dir === "instrument" ? 0.72 : 0.74;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="0 0 108 108"><g transform="translate(54 54) scale(${k}) translate(-50 -50)">${fg}</g></svg>`;
}
