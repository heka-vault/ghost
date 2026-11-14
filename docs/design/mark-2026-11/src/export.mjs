/**
 * Stage one: every mark as an SVG source (svg/) and as a PNG rendered at its own pixel size
 * (png/), so the small sizes on the sheet are what a dock, a tab or a tray would really show,
 * not a big drawing scaled down by the browser.
 *
 *   node docs/design/mark-2026-11/src/export.mjs
 */
import { mkdirSync, writeFileSync, copyFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

import { launch, open } from "./chrome.mjs";
import { adaptive, bare, eyes, instrument, lit, litBare, mono, overlay, STATES, STILL, STILL_SMALL, TODAY } from "./marks.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = join(ROOT, "..", "..", "..");
const SVG = join(ROOT, "svg");
const PNG = join(ROOT, "png");
mkdirSync(SVG, { recursive: true });
mkdirSync(PNG, { recursive: true });

export const SIZES = [1024, 512, 256, 128, 48, 32, 24, 16];
export const DIRS = { lit, instrument, eyes };

/** name -> [svg, px] */
const jobs = new Map();
const add = (name, svgText, px) => jobs.set(name, [svgText, px]);

for (const [d, fn] of Object.entries(DIRS)) {
  for (const px of SIZES) add(`${d}-${px}`, fn(px, "idle"), px);
  for (const px of [64, 40, 36, 20]) add(`${d}-${px}`, fn(px, "idle"), px);
  for (const st of Object.keys(STATES)) for (const px of [256, 128, 48, 36, 32, 16]) add(`${d}-${st}-${px}`, fn(px, st), px);
  for (const px of [256, 128, 48, 32, 24, 20, 16]) {
    add(`${d}-mono-${px}`, mono(d, px, "#0b0d12"), px);
    add(`${d}-monolight-${px}`, mono(d, px, "#eafcff"), px);
  }
  add(`${d}-adaptive-fg`, adaptive(d, "foreground"), 432);
  add(`${d}-adaptive-fg-reaching`, adaptive(d, "foreground", "reaching"), 432);
  add(`${d}-adaptive-bg`, adaptive(d, "background"), 432);
  add(`${d}-adaptive-mono`, mono(d, 432, "#ffffff").replace('viewBox="0 0 100 100"', 'viewBox="-26 -26 152 152"'), 432);
}
for (const px of [512, 128, 64, 48, 36, 32, 24, 16]) for (const st of Object.keys(STATES)) add(`bare-${st}-${px}`, litBare(px, st), px);
add("character-today", bare(TODAY, { px: 480 }), 480);
add("character-still", bare(STILL, { px: 480 }), 480);
add("character-still-small", bare(STILL_SMALL, { px: 480 }), 480);
add("character-overlay", overlay(560), 560);

for (const [name, [text]] of jobs) writeFileSync(join(SVG, `${name}.svg`), text);

// One page with every mark at its own size, then a clip per mark at device scale 1.
let x = 0;
let y = 0;
let rowH = 0;
const W = 2200;
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
const H = y + rowH + 4;
const html = `<!doctype html><meta charset="utf-8"><style>html,body{margin:0;background:transparent}img{position:absolute;display:block}</style>${cells.map((c) => `<img src="../svg/${c.name}.svg" width="${c.px}" height="${c.px}" style="left:${c.x}px;top:${c.y}px">`).join("")}`;
const atlas = join(ROOT, "src", ".atlas.html");
writeFileSync(atlas, html);

const cdp = await launch();
try {
  const page = await open(cdp, atlas, { width: W, height: H, transparent: true });
  for (const c of cells) writeFileSync(join(PNG, `${c.name}.png`), await page.shot({ x: c.x, y: c.y, width: c.px, height: c.px }));
  await page.close();
} finally {
  await cdp.close();
}

// Today's marks, from the shipped files (read only), resized the way a desktop would.
const T = (p) => join(REPO, p);
for (const px of [...SIZES, 64, 40, 36, 20]) execFileSync("magick", [T("apps/desktop/src-tauri/icons/icon.png"), "-filter", "Lanczos", "-resize", `${px}x${px}`, join(PNG, `today-${px}.png`)]);
execFileSync("magick", [T("apps/desktop/src-tauri/icons/32x32.png"), join(PNG, "today-32.png")]);
execFileSync("magick", [T("apps/desktop/src-tauri/icons/32x32.png"), "-filter", "Lanczos", "-resize", "16x16", join(PNG, "today-16.png")]);
execFileSync("magick", [T("apps/desktop/src-tauri/icons/32x32.png"), "-filter", "Lanczos", "-resize", "24x24", join(PNG, "today-24.png")]);
copyFileSync(T("apps/desktop/public/favicon.svg"), join(SVG, "today-favicon.svg"));
copyFileSync(T("apps/mobile/assets/android-icon-foreground.png"), join(PNG, "today-adaptive-fg.png"));
copyFileSync(T("apps/mobile/assets/android-icon-background.png"), join(PNG, "today-adaptive-bg.png"));
copyFileSync(T("apps/mobile/assets/android-icon-monochrome.png"), join(PNG, "today-adaptive-mono.png"));
for (const px of [64, 48, 40, 36, 32, 24, 20, 16]) execFileSync("magick", [T("apps/mobile/assets/android-icon-foreground.png"), "-trim", "-resize", `${px}x${px}`, "-gravity", "center", "-background", "none", "-extent", `${px}x${px}`, join(PNG, `today-bare-${px}.png`)]);
for (const px of [32, 16]) execFileSync("magick", ["-background", "none", "-density", String(96 * (px / 32)), join(SVG, "today-favicon.svg"), "-resize", `${px}x${px}`, join(PNG, `today-favicon-${px}.png`)]);

console.log(`export: ${jobs.size} marks to svg/ and png/`);
