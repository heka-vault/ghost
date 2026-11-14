/**
 * Round 2, stage one: every figure as SVG (svg/) and as PNG at its own pixel size (png/, not
 * committed): the marks at 1024 to 16, the black silhouettes at 64 and 16, the family rows.
 *
 *   node docs/design/mark-2026-11/round-2/src/export.mjs
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { launch, open } from "../../src/chrome.mjs";
import { FIGURES, familyRow, mark, silhouette, STATES } from "./figures.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = join(ROOT, "..", "..", "..", "..");
const SVG = join(ROOT, "svg");
const PNG = join(ROOT, "png");
mkdirSync(SVG, { recursive: true });
mkdirSync(PNG, { recursive: true });

export const KEYS = Object.keys(FIGURES);
export const DIRECTIONS = ["hem", "hemRound", "hemMitts", "tailMitts", "fade", "peek", "orb", "drift"];

const shapesSrc = readFileSync(join(REPO, "packages/ui/src/agentShapes.ts"), "utf8");
export const SHAPES = [...shapesSrc.matchAll(/^\s+(\w+): \{ body: "([^"]+)", eyes: EYES\(([^)]*)\) \},?$/gm)].map(([, name, d, args]) => {
  const [y, gap = 2.6, rx = 1.35, ry = 1.95] = args.split(",").map(Number);
  return { name, d, eyes: [[12 - gap, y, rx, ry], [12 + gap, y, rx, ry]] };
});
const HUES = ["#9db0ff", "#c4a8ff", "#ff9ccf", "#ffad7a", "#f5cc5c", "#6ee39a", "#5fd4e6", "#7cc9ff"];

const jobs = new Map();
const add = (name, text, px) => jobs.set(name, [text, px]);
for (const k of KEYS) {
  add(`sil-${k}-64`, silhouette(k, 64), 64);
  add(`sil-${k}-16`, silhouette(k, 16), 16);
  add(`sil-${k}-256`, silhouette(k, 256), 256);
}
for (const k of DIRECTIONS) {
  for (const px of [1024, 256, 128, 48, 36, 32, 16]) add(`${k}-${px}`, mark(k, px), px);
  for (const st of Object.keys(STATES)) add(`${k}-${st}-128`, mark(k, 128, st), 128);
  add(`${k}-listening-16`, mark(k, 16, "listening"), 16);
  add(`${k}-listening-36`, mark(k, 36, "listening"), 36);
  add(`${k}-mono-16`, silhouette(k, 16, "#eafcff"), 16);
  add(`${k}-monodark-16`, silhouette(k, 16, "#1d2027"), 16);
  add(`${k}-fg`, mark(k, 432).replace(/<rect width="100" height="100" rx="22.5" fill="#05060a"\/><rect width="100" height="100" rx="22.5" fill="url\(#[^)]+\)"\/><rect [^>]*stroke="url\(#[^)]+\)"[^>]*\/>/, "").replace('viewBox="0 0 100 100"', 'viewBox="-21 -21 142 142"'), 432);
  familyRow(k, SHAPES, HUES, 64).forEach((s, i) => add(`fam-${k}-${i}`, s, 64));
}
for (const [name, [text]] of jobs) writeFileSync(join(SVG, `${name}.svg`), text);

let x = 0;
let y = 0;
let rowH = 0;
const W = 2400;
const cells = [];
for (const [name, [, px]] of jobs) {
  if (x + px > W) {
    x = 0;
    y += rowH + 4;
    rowH = 0;
  }
  cells.push({ name, px, x, y });
  x += px + 4;
  rowH = Math.max(rowH, px);
}
const atlas = join(ROOT, "src", ".atlas.html");
writeFileSync(atlas, `<!doctype html><meta charset="utf-8"><style>html,body{margin:0;background:transparent}img{position:absolute;display:block}</style>${cells.map((c) => `<img src="../svg/${c.name}.svg" width="${c.px}" height="${c.px}" style="left:${c.x}px;top:${c.y}px">`).join("")}`);
const cdp = await launch();
try {
  const page = await open(cdp, atlas, { width: W, height: y + rowH + 4, transparent: true });
  for (const c of cells) writeFileSync(join(PNG, `${c.name}.png`), await page.shot({ x: c.x, y: c.y, width: c.px, height: c.px }));
  await page.close();
} finally {
  await cdp.close();
}
console.log(`round-2 export: ${jobs.size} files`);
