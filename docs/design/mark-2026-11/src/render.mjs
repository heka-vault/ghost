/**
 * Stage three: the sheet rendered to images, one per section, at twice the
 * pixels (the small marks are pixelated, so each of their pixels is a clean 2×2 block).
 *
 *   node docs/design/mark-2026-11/src/render.mjs
 */
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { launch, magick, open } from "./chrome.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
// Round 2 renders its own sheet the same way: pass its folder.
const BASE = process.argv[2] ? join(process.cwd(), process.argv[2]) : ROOT;
const OUT = join(BASE, "sheet");
const WORK = join(BASE, "src", ".work");
mkdirSync(WORK, { recursive: true });

const cdp = await launch();
try {
  const page = await open(cdp, join(OUT, "index.html"), { width: 1440, height: 1000, scale: 2 });
  const h = await page.evaluate("document.documentElement.scrollHeight");
  await page.resize(1440, h);
  await page.evaluate("new Promise(r => setTimeout(r, 400))");
  for (const r of await page.rects("section[data-name]")) {
    const f = join(WORK, `${r.name}.png`);
    writeFileSync(f, await page.shot({ x: r.x, y: r.y, width: r.width, height: r.height }));
    await magick([f, "-quality", "88", "-define", "webp:method=6", join(OUT, `${r.name}.webp`)]);
    console.log(`render: sheet/${r.name}.webp`);
  }


  await page.close();
} finally {
  await cdp.close();
}
