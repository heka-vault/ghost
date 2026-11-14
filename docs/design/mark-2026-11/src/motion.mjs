/**
 * Stage four: the recommended mark alive, as three short WebP loops (motion/). Each frame is
 * the instrument drawn with that moment's inputs (how much of the ring is drawn, the light,
 * the drift, a blink, the arc and its colour), rendered by Chrome at 2x and resized.
 *
 *   node docs/design/mark-2026-11/src/motion.mjs
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { launch, magick, open } from "./chrome.mjs";
import { instrument, STATES } from "./marks.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "motion");
const WORK = join(ROOT, "src", ".work", "motion");
mkdirSync(OUT, { recursive: true });
mkdirSync(WORK, { recursive: true });
const FPS = 20;
const PX = 512;

const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const smooth = (p) => { p = clamp(p, 0, 1); return p * p * (3 - 2 * p); };
const easeOut = (p) => 1 - Math.pow(1 - clamp(p, 0, 1), 3);
const hex = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const mix = (a, b, w) => "#" + hex(a).map((v, i) => Math.round(v + (hex(b)[i] - v) * w).toString(16).padStart(2, "0")).join("");
const lerp = (a, b, w) => a + (b - a) * w;
const blinkAt = (t, at) => { const u = (t - at) * 1000; return u < 0 || u > 170 ? 0 : u < 60 ? easeOut(u / 60) : 1 - smooth((u - 60) / 110); };

/** The arrival: from depth, the room's shockwave, the ticks drawing on clockwise, the light last. */
function arrival(t) {
  const T = 1.6;
  if (t > T - 0.25) t = T - 0.25; // hold the last pose
  const p = t / 1.2;
  const z = easeOut(p / 0.7);
  const shock = clamp((t - 0.25) / 0.6, 0, 1);
  const extra = shock > 0 && shock < 1 ? `<circle cx="50" cy="50" r="${8 + shock * 44}" fill="none" stroke="#bff4ff" stroke-opacity="${(1 - shock) * 0.8}" stroke-width="${0.6 + (1 - shock) * 1.2}"/>` : "";
  return { scale: lerp(0.2, 1, z), light: lerp(2.2, 1, smooth(p)) * smooth(p / 0.35), reveal: smooth((t - 0.3) / 0.7), level: smooth((t - 0.8) / 0.4), extra };
}
/** The idle breath: the light rises and falls, Wisp drifts, one blink; the ring stays still. */
function breath(t) {
  const w = Math.sin((2 * Math.PI * t) / 4);
  return { light: 1 + 0.08 * w, dy: -0.75 * Math.sin((2 * Math.PI * t) / 4 + 0.6), blink: blinkAt(t, 2.6) };
}
/** A state change, each held a moment: the arc travels, the colour blends over 450 ms. */
const SEQ = ["idle", "listening", "reaching", "speaking", "waiting"];
const ARCS = { idle: [-162, -102], listening: [-120, -60], reaching: [-40, 40], waiting: [60, 120] };
function states(t) {
  const per = 1.4;
  const i = Math.floor(t / per) % SEQ.length;
  const j = (i + 1) % SEQ.length;
  const w = smooth(((t % per) - (per - 0.45)) / 0.45);
  const a = SEQ[i];
  const b = SEQ[j];
  const tint = mix(STATES[a].tint, STATES[b].tint, w);
  const cur = w < 0.5 ? a : b;
  if (cur === "speaking") {
    const lvl = 0.45 + 0.4 * Math.abs(Math.sin(t * 7.5)) * (0.6 + 0.4 * Math.sin(t * 2.3));
    return { state: "speaking", tint, level: lvl, turn: Math.round(t * 20) };
  }
  // The arc moves the short way round between two states.
  const A = ARCS[a] ?? [0, 360];
  const B = ARCS[b] ?? [0, 360];
  const arc = a === "speaking" ? B : b === "speaking" ? A : [lerp(A[0], B[0], w), lerp(A[1], B[1], w)];
  return { state: "idle", tint, arc, level: 0.85 + 0.15 * Math.sin(t * 5) };
}

const LOOPS = [
  { name: "arrival", secs: 1.6, f: arrival },
  { name: "breath", secs: 4, f: breath },
  { name: "states", secs: 1.4 * SEQ.length, f: states },
];

const page0 = join(WORK, "frame.html");
writeFileSync(page0, `<!doctype html><style>html,body{margin:0;background:#08090d}#m{width:${PX / 2}px;height:${PX / 2}px}#m svg{width:100%;height:100%;display:block}</style><div id="m"></div>`);
const cdp = await launch();
try {
  const page = await open(cdp, page0, { width: PX / 2, height: PX / 2, scale: 2 });
  for (const L of LOOPS) {
    const files = [];
    const n = Math.round(L.secs * FPS);
    for (let k = 0; k < n; k++) {
      const o = L.f(k / FPS);
      const svg = instrument(PX, o.state ?? "idle", o);
      await page.evaluate(`document.getElementById('m').innerHTML = ${JSON.stringify(svg)}; new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))`);
      const f = join(WORK, `${L.name}-${String(k).padStart(4, "0")}.png`);
      writeFileSync(f, await page.shot({ x: 0, y: 0, width: PX / 2, height: PX / 2 }));
      files.push(f);
    }
    await magick(["-delay", String(100 / FPS), ...files, "-resize", "256x256", "-quality", "82", "-define", "webp:method=6", "-loop", "0", join(OUT, `${L.name}.webp`)]);
    console.log(`motion: motion/${L.name}.webp (${n} frames)`);
  }
  await page.close();
} finally {
  await cdp.close();
}
