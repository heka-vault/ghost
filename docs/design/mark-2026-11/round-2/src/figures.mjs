/**
 * Round 2: Wisp without the tail. Every figure is plain data on Wisp's 24 grid (the head is
 * wisp.ts's own: a circle of 7 about (12, 10)), drawn by `litMark` in round 1's night tile and
 * ring. A figure: body paths, mitts (arms drawn as part of the body and lit with it), the eyes,
 * a pose, and optionally a fade (the lower part dissolving into light) or a clip (the frame
 * cutting it). `t` and the motion inputs are for the live loops; the still mark is t = 0.
 */
import { body as tailBody, ring, streak, tile, grain, offStrike, STATES, NIGHT, INK } from "../../src/marks.mjs";

const n = (v) => (Math.abs(v) < 1e-4 ? 0 : Math.round(v * 1000) / 1000);
const HEAD = "M5 10A7 7 0 0 1 19 10";

/** An ellipse as a path, turned by `rot` degrees: a mitt. */
export function oval(cx, cy, rx, ry, rot = 0) {
  const a = (rot * Math.PI) / 180;
  const px = cx + rx * Math.cos(a);
  const py = cy + rx * Math.sin(a);
  const qx = cx - rx * Math.cos(a);
  const qy = cy - rx * Math.sin(a);
  return `M${n(px)} ${n(py)}A${n(rx)} ${n(ry)} ${n(rot)} 1 0 ${n(qx)} ${n(qy)}A${n(rx)} ${n(ry)} ${n(rot)} 1 0 ${n(px)} ${n(py)}Z`;
}

/**
 * The sheet ghost: Wisp's head, sides falling straight with a slight flare, the bottom closed
 * in `lobes` shallow round lobes. `wave` (0..1) and `ph` ripple the hem as a wave travelling
 * across it: what replaces the tail's swing in the live mascot. `skew` lowers the hem to one
 * side (direction 5's drift).
 */
export function hem({ lobes = 3, depth = 1.7, wave = 0, ph = 0, skew = 0, flare = 0.4, base = 18.6 } = {}) {
  const R = 19 + flare;
  const L = 5 - flare;
  const w = (R - L) / lobes;
  let d = `${HEAD}C19 13.5 ${n(R)} 16 ${n(R)} ${n(base + skew * 0.5)}`;
  for (let i = 0; i < lobes; i++) {
    const a = R - i * w;
    const b = a - w;
    const ya = base + skew * (0.5 - i / lobes);
    const yb = base + skew * (0.5 - (i + 1) / lobes);
    const dep = depth * (1 + wave * Math.sin(ph + i * 2.1));
    const c = 1.33 * dep;
    d += `C${n(a - w * 0.05)} ${n(ya + c)} ${n(b + w * 0.05)} ${n(yb + c)} ${n(b)} ${n(yb)}`;
  }
  return d + `C${n(L)} 16 5 13.5 5 10Z`;
}

/** One soft round bottom: the shape a hem becomes with no lobes. */
export const roundHem = () => `${HEAD}C19 13.6 19.6 17 18.7 19.4C17.6 22.2 14.6 22.6 12 22.6C9.4 22.6 6.4 22.2 5.3 19.4C4.4 17 5 13.6 5 10Z`;

/** The head and straight sides, run long: the lower part is faded or cut away. */
export const dome = (to = 24) => `${HEAD}V${to}H5Z`;

/**
 * The orb: nearly round, a little fuller low on the right and leaning, so it has weight and a
 * direction (a drop of light settling), not a sphere. `m` (0..1) and `ph` move the outline.
 */
export function orb({ m = 0, ph = 0 } = {}) {
  const r = [8.1, 8.5, 7.7, 8.0].map((v, i) => v + m * 0.35 * Math.sin(ph + i * 1.7));
  const [rt, rr, rb, rl] = r;
  const cx = 12;
  const cy = 12.6;
  const k = 0.5523;
  return (
    `M${cx} ${n(cy - rt)}` +
    `C${n(cx + rr * k)} ${n(cy - rt)} ${n(cx + rr)} ${n(cy - rt * k)} ${n(cx + rr)} ${n(cy + 0.3)}` +
    `C${n(cx + rr)} ${n(cy + rb * k + 0.6)} ${n(cx + rr * k + 0.5)} ${n(cy + rb)} ${n(cx + 0.4)} ${n(cy + rb)}` +
    `C${n(cx - rl * k)} ${n(cy + rb)} ${n(cx - rl)} ${n(cy + rb * k)} ${n(cx - rl)} ${n(cy)}` +
    `C${n(cx - rl)} ${n(cy - rt * k)} ${n(cx - rl * k)} ${n(cy - rt)} ${cx} ${n(cy - rt)}Z`
  );
}

/** Mitts: rounded nubs at the shoulders' height, the right raised a little (the wave, at rest). */
export function mitts({ aL = 0, aR = 0, size = 1 } = {}) {
  const rx = 1.9 * size;
  const ry = 1.3 * size;
  // Each mitt hinges at the body's edge; the angle swings it about that point.
  const hinge = (hx, hy, len, deg) => {
    const a = (deg * Math.PI) / 180;
    return [hx + Math.cos(a) * len, hy + Math.sin(a) * len, deg];
  };
  const [lx, ly, lr] = hinge(5.6, 13.6, 1.75 * size, 180 - 32 + aL);
  const [rx_, ry_, rr] = hinge(18.4, 13.2, 1.75 * size, -18 + aR);
  return [oval(lx, ly, rx, ry, lr), oval(rx_, ry_, rx, ry, rr)];
}

const EYES = (y = 9.9, gap = 2.45, rx = 1.2, ry = 2.0) => [
  [12 - gap, y, rx, ry],
  [12 + gap, y, rx, ry],
];

/**
 * Every figure of the round. `pose` turns it about (12, 13); `h` is its height on the tile;
 * `cy` where its middle sits; `fade` [from, to] in grid y; `clip` cuts it at the ring (or the
 * tile) so it rises from the bottom.
 */
export const FIGURES = {
  today: { label: "Today (GHOST_PATH)", body: [tailBody(0)], eyes: EYES(10.3, 2.5, 1.25, 1.65), pose: 0, h: 20.6 },
  todayArms: { label: "Today's icon (arms as lines)", body: [tailBody(0)], lines: true, eyes: EYES(10.3, 2.5, 1.25, 1.65), pose: 0, h: 20.6 },
  round1: { label: "Round 1 still pose", body: [tailBody(0.6)], eyes: EYES(9.75, 2.45, 1.18, 2.05), pose: -7, h: 20.6 },
  hem: { label: "1a · Rounded hem", body: [hem()], eyes: EYES(), pose: 0, h: 20 },
  hemRound: { label: "1b · One round hem", body: [roundHem()], eyes: EYES(), pose: 0, h: 20 },
  hemMitts: { label: "1c · Hem with mitts", body: [hem()], mitts: mitts(), eyes: EYES(), pose: 0, h: 20 },
  tailMitts: { label: "2 · Today's tail with mitts", body: [tailBody(0)], mitts: mitts(), eyes: EYES(10.1), pose: 0, h: 20.6 },
  fade: { label: "3a · No bottom half, centred", body: [dome(24)], eyes: EYES(10.4, 2.5, 1.25, 2.1), pose: 0, h: 18, fade: [12.6, 22.5] },
  peek: { label: "3b · Rising from the frame", body: [dome(30)], eyes: EYES(10.6, 2.5, 1.25, 2.1), pose: 0, h: 19, clip: true },
  orb: { label: "4 · Orb", body: [orb()], eyes: EYES(11.2, 2.6, 1.25, 2.15), pose: -6, h: 18, cy: 12.6 },
  drift: { label: "5 · Drift: hem, mitts, a lean", body: [hem({ skew: 1.1, depth: 1.55 })], mitts: mitts({ size: 0.92 }), eyes: EYES(9.8, 2.45, 1.2, 2.05), pose: -5, h: 20 },
};

/** Live inputs for a figure at time t (the float's moods): hem ripple, mitt angles, outline, the talking mouth. */
export function animate(key, f, o = {}) {
  const out = { ...f };
  const ph = o.ph ?? 0;
  if (key === "hem" || key === "hemMitts") out.body = [hem({ wave: o.wave ?? 0, ph })];
  if (key === "drift") out.body = [hem({ skew: 1.1, depth: 1.55, wave: o.wave ?? 0, ph })];
  if (key === "orb") out.body = [orb({ m: o.wave ?? 0, ph })];
  if (f.mitts) out.mitts = mitts({ aL: o.aL ?? 0, aR: o.aR ?? 0, size: key === "drift" ? 0.92 : 1 });
  if (o.mouth != null) out.mouth = o.mouth;
  if (o.blink != null) out.blink = o.blink;
  return out;
}

/** The figure's height on the 24 grid, top of the head to the lowest point it draws. */
const TOP = 3;

let uid = 0;
const id = (p) => `r2${p}${++uid}`;

/**
 * A figure lit from within, as SVG pieces in tile units: `at` = { cx, cy, h } (h the figure's
 * drawn height in tile units). Over 48px a tight and a wide bloom; under, a halo stroke.
 */
function litFigure(px, tint, f, at, { bloom = 1, clipR = null, inkFill = null } = {}) {
  const k = at.h / f.h;
  const mid = TOP + f.h / 2 + (f.cy ? f.cy - (TOP + f.h / 2) : 0);
  const place = `translate(${n(at.cx)} ${n(at.cy)}) scale(${n(k)}) translate(-12 ${n(-mid)})${f.pose ? ` rotate(${f.pose} 12 13)` : ""}`;
  const hair = 100 / px / k;
  const paths = [...f.body, ...(f.mitts ?? [])];
  const fill = id("f");
  let defs = inkFill
    ? ""
    : `<radialGradient id="${fill}" cx="11.4" cy="${n((f.cy ?? 9.6) - 0.6)}" r="13" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#ffffff"/><stop offset="0.38" stop-color="#f4feff"/><stop offset="0.72" stop-color="${INK}"/><stop offset="1" stop-color="${tint}"/></radialGradient>`;
  const paint = inkFill ?? `url(#${fill})`;
  let under = "";
  if (!inkFill) {
    if (px >= 48 && bloom > 0) {
      const b1 = id("b");
      const b2 = id("b");
      defs += `<filter id="${b1}" x="-60%" y="-60%" width="220%" height="220%"><feGaussianBlur stdDeviation="0.55"/></filter><filter id="${b2}" x="-120%" y="-120%" width="340%" height="340%"><feGaussianBlur stdDeviation="2.4"/></filter>`;
      under = paths.map((d) => `<path d="${d}" fill="${tint}" opacity="${n(0.55 * bloom)}" filter="url(#${b2})"/>`).join("") + paths.map((d) => `<path d="${d}" fill="${tint}" opacity="${n(0.9 * bloom)}" filter="url(#${b1})"/>`).join("");
    } else {
      under = paths.map((d) => `<path d="${d}" fill="none" stroke="${tint}" stroke-opacity="${px >= 24 ? 0.38 : 0.5}" stroke-width="${n(hair * (px >= 24 ? 2.4 : 1.6))}" stroke-linejoin="round"/>`).join("");
    }
  }
  let lines = "";
  if (f.lines) {
    const c = inkFill ?? INK;
    lines = `<line x1="5.8" y1="14.2" x2="2.6" y2="16.4" stroke="${c}" stroke-width="2.3" stroke-linecap="round"/><line x1="18.2" y1="14.2" x2="21" y2="11.6" stroke="${c}" stroke-width="2.3" stroke-linecap="round"/>`;
  }
  const fills = paths.map((d) => `<path d="${d}" fill="${paint}"/>`).join("");
  const eyeFill = inkFill ? "#ffffff" : "#06121a";
  const open = Math.max(0.12, 1 - (f.blink ?? 0));
  let eyes;
  if (px < 32 && !inkFill) {
    // Snapped to whole pixels, both on one row (round 1's rule).
    const u = 100 / px;
    const [w, hh] = px < 24 ? [2, 3] : [2, 4];
    const ty = at.cy + k * (f.eyes[0][1] - mid);
    eyes = f.eyes.map(([gx]) => {
      const tx = at.cx + k * (gx - 12);
      return `<rect x="${n(Math.round(tx / u - w / 2) * u)}" y="${n(Math.round(ty / u - hh / 2) * u)}" width="${n(w * u)}" height="${n(hh * u)}" rx="${n(u * 0.6)}" fill="${eyeFill}"/>`;
    });
    eyes = eyes.join("");
  } else {
    eyes = "";
  }
  const eyesIn = eyes ? "" : f.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${n(y + ry * (1 - open) * 0.7)}" rx="${rx}" ry="${n(ry * open)}" fill="${eyeFill}"/>`).join("");
  const mouth = f.mouth ? `<ellipse cx="12" cy="${n(f.eyes[0][1] + 3.9)}" rx="${n(0.85 + 0.3 * f.mouth)}" ry="${n(0.25 + 1.0 * f.mouth)}" fill="${eyeFill}"/>` : "";
  let group = `<g transform="${place}">${under}${lines}${fills}${eyesIn}${mouth}</g>${eyes}`;
  if (f.fade) {
    const m = id("m");
    const g = id("g");
    defs += `<linearGradient id="${g}" gradientUnits="userSpaceOnUse" x1="0" x2="0" y1="${f.fade[0]}" y2="${f.fade[1]}"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#000"/></linearGradient><mask id="${m}" maskUnits="userSpaceOnUse" x="-20" y="-20" width="140" height="140"><g transform="${place}"><rect x="-10" y="-10" width="44" height="44" fill="url(#${g})"/></g></mask>`;
    group = `<g mask="url(#${m})">${group}</g>`;
    if (!inkFill) {
      // The light it rises out of: a pool where the body dissolves.
      const p = id("p");
      const fy = at.cy + k * (f.fade[0] + 2.4 - mid);
      defs += `<radialGradient id="${p}"><stop offset="0" stop-color="${tint}" stop-opacity="0.85"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></radialGradient>`;
      group = `<ellipse cx="${n(at.cx)}" cy="${n(fy)}" rx="${n(k * 10)}" ry="${n(k * 4)}" fill="url(#${p})"/>` + group;
    }
  }
  if (f.clip) {
    const c = id("c");
    defs += clipR ? `<clipPath id="${c}"><circle cx="50" cy="50" r="${clipR}"/></clipPath>` : `<clipPath id="${c}"><rect width="100" height="100" rx="22.5"/></clipPath>`;
    group = `<g clip-path="url(#${c})">${group}</g>`;
  }
  return { defs, inner: group };
}

const svg = (px, inner, defs = "") => `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="0 0 100 100">${defs ? `<defs>${defs}</defs>` : ""}${inner}</svg>`;

/**
 * The mark: round 1's system (night tile, the state's light and rim, the ring of ticks with its
 * lit arc from 32 px up, the streak at 128 and up), with the figure in the middle.
 */
export function mark(key, px, state = "idle", o = {}) {
  uid = 0;
  const f0 = FIGURES[key];
  const f1 = o.anim ? animate(key, f0, o.anim) : f0;
  // Under 32 px the hands only widen the blob: the hem alone is the mark (round 1's levels of detail).
  const f = px < 32 && f1.mitts ? { ...f1, mitts: undefined } : f1;
  const tint = o.tint ?? STATES[state].tint;
  const off = state === "off";
  const ringed = px >= 32 && o.ring !== false;
  const t = tile(px, tint, { lift: (off ? 0.06 : 0.2) * (o.light ?? 1), cy: 50 });
  const R = px >= 128 ? 35 : px >= 48 ? 36 : 39;
  // The figure's size: inside the ring when there is one, filling the tile when there is none.
  const peek = !!f.clip;
  let h = ringed ? (px >= 128 ? 45 : px >= 48 ? 46 : 50) : px < 24 ? 80 : 70;
  if (key === "orb") h *= 0.95;
  h *= o.scale ?? 1;
  const cy = peek ? (ringed ? 66 : 70) : 51 + (o.dy ?? 0);
  if (peek) h *= ringed ? 1.25 : 1.15;
  const w = litFigure(px, off ? "#9aa3b2" : tint, f, { cx: 50, cy, h }, { bloom: (off ? 0.3 : 1) * (o.light ?? 1), clipR: ringed ? R - 1.4 : null });
  const arcs = { idle: [-162, -102], listening: [-120, -60], reaching: [-40, 40], speaking: [0, 0], waiting: [60, 120], off: [0, 0] };
  const r = !ringed
    ? ""
    : off
      ? ring(px, "#9aa3b2", { R, arc: [0, 0], level: 0 })
      : state === "speaking"
        ? ring(px, tint, { R, full: o.level ?? 0.8, turn: o.turn ?? 0 })
        : ring(px, tint, { R, arc: o.arc ?? arcs[state], level: o.level ?? 1, turn: o.turn ?? 0, reveal: o.reveal ?? 1 });
  const s = streak(px, tint, 51 - h * 0.17, off || peek ? 0 : 0.8 * (o.light ?? 1));
  const gr = grain(px);
  return svg(px, t.inner + s.inner + r + w.inner + (o.extra ?? "") + gr.use + (off ? offStrike(px) : ""), t.defs + s.defs + w.defs + gr.defs);
}

/** The silhouette alone: solid black on white, no light, no ring. The test of this round. */
export function silhouette(key, px = 64, color = "#000") {
  uid = 0;
  const f = px < 32 && FIGURES[key].mitts ? { ...FIGURES[key], mitts: undefined } : FIGURES[key];
  const w = litFigure(px, color, f, { cx: 50, cy: f.clip ? 64 : 51, h: f.clip ? 92 : 86 }, { inkFill: color });
  return svg(px, w.inner, w.defs);
}

/** The agents' family beside a Wisp: the shapes from agentShapes.ts, lit, in their hues. */
export function familyRow(key, shapes, hues, px = 64) {
  const one = (inner, defs) => svg(px, inner, defs);
  uid = 0;
  const out = [];
  {
    const w = litFigure(px, "#5fe3ff", FIGURES[key], { cx: 50, cy: FIGURES[key].clip ? 62 : 51, h: FIGURES[key].clip ? 88 : 80 });
    const clipBox = FIGURES[key].clip ? "" : "";
    out.push(one(clipBox + w.inner, w.defs));
  }
  shapes.forEach((s, i) => {
    uid = 0;
    const g = id("s");
    const c = hues[(i + 1) % 8];
    const defs = `<filter id="${g}" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="0.7"/></filter>`;
    const inner = `<g transform="translate(50 50) scale(3.9) translate(-12 -12)"><path d="${s.d}" fill="${c}" opacity=".7" filter="url(#${g})"/><path d="${s.d}" fill="#f4feff"/>${s.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${y}" rx="${rx}" ry="${ry}" fill="#06121a"/>`).join("")}</g>`;
    out.push(one(inner, defs));
  });
  return out;
}

export { STATES, NIGHT };
