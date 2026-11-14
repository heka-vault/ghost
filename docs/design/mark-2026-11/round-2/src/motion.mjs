/**
 * Round 2, the two strongest directions alive in the ring: the arrival and speaking. Speaking
 * is the float's mood, so it has the live mascot's talking mouth; the still mark has none.
 *
 *   node docs/design/mark-2026-11/round-2/src/motion.mjs
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { launch, magick, open } from "../../src/chrome.mjs";
import { mark } from "./figures.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "motion");
const WORK = join(ROOT, "src", ".work");
mkdirSync(OUT, { recursive: true });
mkdirSync(WORK, { recursive: true });
const FPS = 20;
const PX = 512;
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const smooth = (p) => { p = clamp(p, 0, 1); return p * p * (3 - 2 * p); };
const easeOut = (p) => 1 - Math.pow(1 - clamp(p, 0, 1), 3);
const lerp = (a, b, w) => a + (b - a) * w;

/** The arrival (round 1's): from depth, a shockwave, the ticks drawing on; the hem settles last. */
function arrival(t) {
  t = Math.min(t, 1.35);
  const p = t / 1.2;
  const z = easeOut(p / 0.7);
  const shock = clamp((t - 0.25) / 0.6, 0, 1);
  const extra = shock > 0 && shock < 1 ? `<circle cx="50" cy="50" r="${8 + shock * 44}" fill="none" stroke="#bff4ff" stroke-opacity="${(1 - shock) * 0.8}" stroke-width="${0.6 + (1 - shock) * 1.2}"/>` : "";
  const settle = 1 - smooth((t - 0.5) / 0.8);
  return { state: "idle", scale: lerp(0.2, 1, z), light: lerp(2.2, 1, smooth(p)) * smooth(p / 0.35), reveal: smooth((t - 0.3) / 0.7), level: smooth((t - 0.8) / 0.4), extra, anim: { wave: 0.6 * settle, ph: t * 9, aR: -40 * settle * Math.sin(t * 10) } };
}

/** Speaking: the ring driven by the voice, the hem rippling where the tail used to swing, the mitts gesturing, the mouth. */
function speaking(t) {
  const syl = Math.abs(Math.sin((2 * Math.PI * t) / 0.4));
  const stress = 0.55 + 0.45 * Math.sin((2 * Math.PI * t) / 1.1 + 1);
  const u = t % 2.9;
  const phrase = u < 2.4 ? 1 : u < 2.55 ? 1 - smooth((u - 2.4) / 0.15) : u < 2.75 ? 0 : smooth((u - 2.75) / 0.15);
  const m = clamp(syl * stress * 1.15, 0, 1) * phrase;
  return {
    state: "speaking",
    level: 0.35 + 0.6 * m,
    turn: Math.round(t * 6),
    dy: -0.8 * m - 0.6 * Math.sin((2 * Math.PI * t) / 2.4),
    anim: {
      wave: 0.45,
      ph: t * 5,
      aL: 10 * (0.5 - 0.5 * Math.cos((2 * Math.PI * t) / 1.7)),
      aR: -18 * (0.5 - 0.5 * Math.cos((2 * Math.PI * t) / 1.3)),
      mouth: 0.15 + 0.75 * m,
      blink: (() => { const v = ((t - 1.6) * 1000) % 3800; return v < 0 || v > 170 ? 0 : v < 60 ? v / 60 : 1 - (v - 60) / 110; })(),
    },
  };
}

const LOOPS = [
  { key: "hemMitts", name: "hem-mitts-arrival", secs: 1.6, f: arrival },
  { key: "hemMitts", name: "hem-mitts-speaking", secs: 2.9, f: speaking },
  { key: "fade", name: "fade-arrival", secs: 1.6, f: arrival },
  { key: "fade", name: "fade-speaking", secs: 2.9, f: speaking },
];

const page0 = join(WORK, "frame.html");
writeFileSync(page0, `<!doctype html><style>html,body{margin:0;background:#08090d}#m{width:${PX / 2}px;height:${PX / 2}px}#m svg{width:100%;height:100%;display:block}</style><div id="m"></div>`);
const cdp = await launch();
try {
  const page = await open(cdp, page0, { width: PX / 2, height: PX / 2, scale: 2 });
  for (const L of LOOPS) {
    const files = [];
    const count = Math.round(L.secs * FPS);
    for (let k = 0; k < count; k++) {
      const o = L.f(k / FPS);
      await page.evaluate(`document.getElementById('m').innerHTML = ${JSON.stringify(mark(L.key, PX, o.state, o))}; new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))`);
      const f = join(WORK, `${L.name}-${String(k).padStart(4, "0")}.png`);
      writeFileSync(f, await page.shot({ x: 0, y: 0, width: PX / 2, height: PX / 2 }));
      files.push(f);
    }
    await magick(["-delay", String(100 / FPS), ...files, "-resize", "256x256", "-quality", "82", "-define", "webp:method=6", "-loop", "0", join(OUT, `${L.name}.webp`)]);
    console.log(`round-2 motion: motion/${L.name}.webp (${count} frames)`);
  }
  await page.close();
} finally {
  await cdp.close();
}
