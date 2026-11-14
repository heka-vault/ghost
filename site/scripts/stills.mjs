#!/usr/bin/env node
/**
 * The site's stills, taken from its own float page (the app's floating room on the mock), so the
 * picture a visitor sees first, the one with JavaScript off or reduced motion, and the card a
 * link unfurls into are the moving hero caught at one moment: Ghost looking in three places.
 *
 *   pnpm --dir site site:build && node site/scripts/stills.mjs
 *
 * Needs Chrome or Chromium (CHROME=/path) and ImageMagick for WebP and JPEG. Writes
 * src/assets/float-wide.webp (1600x1000), src/assets/float-square.webp (1100x1100),
 * public/og.jpg (1200x630) and public/apple-touch-icon.png (180x180). Nothing leaves the
 * machine: the pages are served from dist on loopback.
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

/** The float page at this size, held at the moment Ghost is looking in three places. */
async function float(width, height, png) {
  const page = await browser.newPage({ viewport: { width, height } });
  await page.goto(`${base}/float/`);
  await page.waitForFunction(() => document.querySelectorAll(".flt-node").length >= 3 && document.querySelector('.flt-root[data-scene="thinking"]'), null, { timeout: 60_000, polling: 100 });
  await page.waitForTimeout(650);
  await page.screenshot({ path: png });
  await page.close();
}

const shots = [
  [1600, 1000, "src/assets/float-wide.webp", ["-quality", "78"]],
  [1100, 1100, "src/assets/float-square.webp", ["-quality", "78"]],
  [1200, 630, "public/og.jpg", ["-quality", "86"]],
];
for (const [w, h, out, opts] of shots) {
  const png = join(tmp, `${w}x${h}.png`);
  await float(w, h, png);
  magick([png, ...opts, join(site, out)]);
  console.log(`stills: ${out}`);
}

// The home-screen icon: the favicon's Wisp, full bleed (the phone rounds the corners itself).
const icon = await browser.newPage({ viewport: { width: 180, height: 180 } });
const svg = readFileSync(join(dist, "favicon.svg"), "utf8").replace('rx="8" ', "");
await icon.setContent(`<style>html,body{margin:0;background:#05060a}svg{display:block;width:180px;height:180px}</style>${svg}`);
await icon.screenshot({ path: join(site, "public", "apple-touch-icon.png") });
console.log("stills: public/apple-touch-icon.png");

await browser.close();
server.close();
rmSync(tmp, { recursive: true, force: true });
