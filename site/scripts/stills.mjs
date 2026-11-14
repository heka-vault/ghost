#!/usr/bin/env node
/**
 * The site's stills, taken from its own float page (the app's floating room on the mock), so the
 * picture a visitor sees first, the one with JavaScript off or reduced motion, is the moving hero
 * caught at one moment: Ghost looking in six places. It is the same Wisp the live float draws, so
 * the still hands over to the live one with no change of body. (The icons and the card a link
 * unfurls into are the mark's, not stills: `node packages/ui/scripts/marks.mjs site`.)
 *
 *   pnpm --dir site site:build && node site/scripts/stills.mjs
 *
 * Needs Chrome or Chromium (CHROME=/path) and ImageMagick for WebP and JPEG. Writes
 * src/assets/float-wide.webp (1680x1000), src/assets/float-tall.webp (780x1040, a phone's top),
 * and src/assets/float-listening.webp (1440x810, the README's desk and crop, beside its
 * stills). Nothing leaves the machine: the pages are served from dist on loopback.
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright-core";

const site = fileURLToPath(new URL("..", import.meta.url));
const dist = join(site, "dist");
if (!existsSync(join(dist, "float", "index.html"))) {
  console.error("stills: build the site first (pnpm --dir site site:build)");
  process.exit(1);
}
const chrome =
  process.env.CHROME ??
  ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"].find((p) => existsSync(p));
if (!chrome) {
  console.error("stills: no Chrome or Chromium (set CHROME=/path/to/it)");
  process.exit(1);
}

const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".woff2": "font/woff2", ".svg": "image/svg+xml", ".webp": "image/webp", ".png": "image/png", ".jpg": "image/jpeg", ".xml": "application/xml", ".txt": "text/plain" };
const server = createServer((req, res) => {
  let path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^(\.\.[/\\])+/, "");
  if (path.endsWith("/")) path += "index.html";
  const file = join(dist, path);
  if (!file.startsWith(dist) || !existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" }).end(readFileSync(file));
}).listen(0, "127.0.0.1");
await new Promise((r) => server.once("listening", r));
const base = `http://127.0.0.1:${server.address().port}`;

const tmp = mkdtempSync(join(tmpdir(), "ghost-stills-"));
const magick = (args) => execFileSync(existsSync("/usr/bin/magick") ? "magick" : "convert", args);
const browser = await chromium.launch({ executablePath: chrome, args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] });

/** The float page held at a moment of the `floating-site` loop, as a PNG at `png`. */
async function float({ width, height, scale = 1, query = "", when, clip }, png) {
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: scale });
  await page.goto(`${base}/float/?latency=0${query}`);
  await page.waitForFunction(when, null, { timeout: 60_000, polling: 50 });
  await page.waitForTimeout(450);
  await page.screenshot({ path: png, clip });
  await page.close();
}

/** Six places out, the first answers on their way back: the hero's moment. */
const reaching = () => document.querySelectorAll(".flt-node").length >= 6 && !!document.querySelector('.flt-root[data-scene="thinking"]');
/** The operator speaking, Ghost listening: the green ring. */
const listening = () => !!document.querySelector('.flt-root[data-scene="you"]');

const shots = [
  // The hero on a wide screen: Ghost beside the work, the left quiet for the words.
  [{ width: 1680, height: 1000, when: reaching }, "src/assets/float-wide.webp", ["-quality", "80"]],
  // The hero on a phone: the top of the screen, Ghost in the middle.
  [{ width: 390, height: 520, scale: 2, query: "&placement=centre", when: reaching }, "src/assets/float-tall.webp", ["-quality", "78"]],
  // Listening, on the README's own desk and crop, to sit beside its stills.
  [{ width: 1280, height: 720, scale: 2, query: "&arrange=readme", when: listening, clip: { x: 560, y: 28, width: 720, height: 405 } }, "src/assets/float-listening.webp", ["-resize", "1440x810", "-quality", "80"]],
];
for (const [opts, out, args] of shots) {
  const png = join(tmp, out.replace(/\W/g, "_") + ".png");
  await float(opts, png);
  magick([png, ...args, join(site, out)]);
  console.log(`stills: ${out}`);
}

await browser.close();
server.close();
rmSync(tmp, { recursive: true, force: true });
