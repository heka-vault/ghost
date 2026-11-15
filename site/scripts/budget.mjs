/**
 * The hero's budget (docs/site.md, What the hero costs), after `site:build`:
 *
 *   pnpm --dir site site:budget [--bytes-only]
 *
 * Deterministic, and failing past its ceiling:
 * - the front page's first paint, gzip: the HTML, its stylesheets, its scripts and what they
 *   import, the preloaded font and the larger of the hero's two stills (as served);
 * - the float frame's module graph, gzip: /float/'s HTML, stylesheets, scripts, the island and
 *   its renderer, and everything they import, static or dynamic.
 *
 * In Chrome (the system's, through playwright-core) against `astro preview` of site/dist,
 * failing when wrong:
 * - the float draws at most 31 times a second and writes Wisp's SVG under a ceiling;
 * - no audio is fetched before the press;
 * - the scene stops by itself after two turns, and then nothing is drawn;
 * - three seconds after the hero scrolls away the frame is gone;
 * - with reduced motion, or Save-Data, there is no frame at all.
 *
 * Timed, and only warned about (a shared runner is noisy; a Lighthouse score is not used, it
 * came out bimodal on this page): with the CPU slowed 4x on a phone's screen, when Ghost comes
 * to life and how much of the main thread it keeps while it plays.
 */
import { spawn } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const site = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(site, "dist");

/** Ceilings. Bytes are gzip at level 9 except images and fonts, which are counted as they are. */
const LIMITS = {
  firstPaintKB: 200,
  floatGraphKB: 115,
  drawsPerSec: 31,
  svgWritesPerSec: 400,
};
/** Warnings only: the review's targets (4x CPU, a phone's screen). */
const WARN = { liveMs: 4500, mainThreadPct: 25 };

let failed = 0;
const check = (ok, what) => {
  console.log(`  ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failed++;
};
const warn = (ok, what) => {
  console.log(`  ${ok ? "ok  " : "warn"} ${what}`);
  if (!ok && process.env.GITHUB_ACTIONS) console.log(`::warning title=site budget::${what}`);
};

// ---------------------------------------------------------------- bytes
const kb = (n) => (n / 1024).toFixed(1);
const file = (url) => join(dist, url.replace(/^\//, "").replace(/(^|\/)$/, "$1index.html"));
const compressed = /\.(webp|png|jpe?g|woff2?|ico|webm|m4a)$/;
const size = (url) => {
  const f = file(url);
  if (!existsSync(f)) throw new Error(`missing in dist: ${url}`);
  return compressed.test(f) ? statSync(f).size : gzipSync(readFileSync(f), { level: 9 }).length;
};
const attrs = (html, tag, attr) => [...html.matchAll(new RegExp(`<${tag}\\b[^>]*\\b${attr}="([^"]+)"`, "g"))].map((m) => m[1]);
/** A script and every module it reaches, static or dynamic (Astro's chunks import each other by relative name). */
function graph(entries) {
  const seen = new Set();
  const walk = (url) => {
    if (seen.has(url)) return;
    seen.add(url);
    const text = readFileSync(file(url), "utf8");
    for (const m of text.matchAll(/["'`](?:\.\/|\/_astro\/|_astro\/)([\w.-]+\.js)["'`]/g)) walk(`/_astro/${m[1]}`);
  };
  entries.forEach(walk);
  return [...seen];
}
function page(url, { stills = [] } = {}) {
  const html = readFileSync(file(url), "utf8");
  const css = attrs(html, "link", "href").filter((h) => h.endsWith(".css"));
  const fonts = [...html.matchAll(/<link\b[^>]*rel="preload"[^>]*href="([^"]+\.woff2)"/g)].map((m) => m[1]);
  const scripts = graph([...attrs(html, "script", "src"), ...attrs(html, "astro-island", "component-url"), ...attrs(html, "astro-island", "renderer-url")]);
  const items = [url, ...css, ...fonts, ...scripts, ...stills].map((u) => ({ url: u, bytes: size(u) }));
  return { total: items.reduce((a, i) => a + i.bytes, 0), items };
}

console.log("\nbytes (gzip; images and fonts as served)");
const index = readFileSync(file("/"), "utf8");
// A visitor loads one still: the wide one, or the tall one on a phone. The larger counts.
const stills = [...new Set([...index.matchAll(/(?:src|srcset)="(\/_astro\/float-(?:wide|tall)\.[\w-]+\.webp)"/g)].map((m) => m[1]))];
const still = stills.sort((a, b) => size(b) - size(a))[0];
const front = page("/", { stills: still ? [still] : [] });
const float = page("/float/");
for (const [name, p, limit] of [
  ["the front page's first paint", front, LIMITS.firstPaintKB],
  ["the float frame's module graph", float, LIMITS.floatGraphKB],
]) {
  const js = p.items.filter((i) => i.url.endsWith(".js")).reduce((a, i) => a + i.bytes, 0);
  check(p.total <= limit * 1024, `${name}: ${kb(p.total)} KB, of it ${kb(js)} KB of script (ceiling ${limit} KB)`);
  for (const i of p.items.sort((a, b) => b.bytes - a.bytes).slice(0, 5)) console.log(`         ${kb(i.bytes).padStart(6)} KB  ${i.url}`);
}
if (process.argv.includes("--bytes-only")) process.exit(failed ? 1 : 0);

// ---------------------------------------------------------------- in Chrome
const { chromium } = await import("playwright-core");
const turn = JSON.parse(readFileSync(join(site, "../apps/desktop/src/design/mock/fixtures/floating-site-turn.json"), "utf8"));
const PORT = 4341;
const URL_ = `http://localhost:${PORT}/`;
const server = spawn("pnpm", ["exec", "astro", "preview", "--port", String(PORT)], { cwd: site, stdio: "ignore", detached: true });
for (let i = 0; i < 150; i++) {
  try {
    if ((await fetch(URL_)).ok) break;
  } catch {}
  await new Promise((r) => setTimeout(r, 100));
}

/** In every document: the field's draws and Wisp's SVG attribute writes, counted. */
const COUNT = () => {
  const m = (window.__budget = { gl: 0, svg: 0 });
  const W = window.WebGL2RenderingContext?.prototype;
  if (W) {
    const d = W.drawArrays;
    W.drawArrays = function (...a) {
      m.gl++;
      return d.apply(this, a);
    };
  }
  const sa = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (k, v) {
    if (this instanceof SVGElement) m.svg++;
    return sa.call(this, k, v);
  };
};
const floatFrame = (page) => page.frames().find((f) => f.url().includes("/float/"));
/** Draws and SVG writes a second in the float frame over `ms`. */
async function rates(page, ms) {
  const f = floatFrame(page);
  if (!f) return { gl: 0, svg: 0, frame: false };
  await f.evaluate(() => Object.assign(window.__budget, { gl: 0, svg: 0 }));
  const t = Date.now();
  await page.waitForTimeout(ms);
  const c = await f.evaluate(() => window.__budget).catch(() => ({ gl: 0, svg: 0 }));
  const s = (Date.now() - t) / 1000;
  return { gl: c.gl / s, svg: c.svg / s, frame: true };
}
const hasFrame = (page) => page.evaluate(() => !!document.querySelector(".hero__frame"));
const live = (page, timeout) => page.waitForFunction(() => document.querySelector("[data-hero][data-live]"), null, { timeout });

const browser = await chromium.launch({ executablePath: process.env.CHROME ?? "/usr/bin/google-chrome" });
try {
  console.log("\nthe float, playing and at rest (1440x900)");
  {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await ctx.addInitScript(COUNT);
    const requests = [];
    ctx.on("request", (r) => requests.push(r.url()));
    const page = await ctx.newPage();
    await page.goto(URL_);
    await live(page, 20_000);
    const liveAt = Date.now();
    await page.waitForTimeout(1500);
    const r = await rates(page, 10_000);
    check(r.frame, "the float is on");
    check(r.gl <= LIMITS.drawsPerSec, `the field draws ${r.gl.toFixed(1)} times a second (at most ${LIMITS.drawsPerSec})`);
    check(r.svg < LIMITS.svgWritesPerSec, `Wisp's SVG takes ${r.svg.toFixed(0)} attribute writes a second (under ${LIMITS.svgWritesPerSec})`);
    check(!requests.some((u) => /\/audio\//.test(u)), "no audio fetched before the press");
    // Two turns from the frame's start, then it settles by itself.
    const left = turn.loop_ms * 2 + 6000 - (Date.now() - liveAt);
    const rested = await page.waitForFunction(() => document.querySelector("[data-hero][data-rest]"), null, { timeout: Math.max(1000, left) }).then(
      () => true,
      () => false,
    );
    check(rested, `the scene stops by itself after two turns (${((Date.now() - liveAt) / 1000).toFixed(1)} s after it came to life)`);
    check(await page.evaluate(() => !document.querySelector("[data-play]").hidden), "Play is offered at rest");
    await page.waitForTimeout(1200);
    const still = await rates(page, 4000);
    check(still.frame && still.gl === 0 && still.svg === 0, `at rest nothing is drawn (${still.gl.toFixed(1)} draws, ${still.svg.toFixed(0)} SVG writes a second)`);
    // A visitor pointing at the hero starts it again.
    await page.mouse.move(300, 300);
    await page.mouse.move(320, 320);
    const again = await page.waitForFunction(() => !document.querySelector("[data-hero][data-rest]"), null, { timeout: 3000 }).then(
      () => true,
      () => false,
    );
    await page.waitForTimeout(2500);
    const back = await rates(page, 2000);
    check(again && back.gl > 0, "pointing at the hero starts it again");
    // Scrolled away: the frame is emptied within three seconds.
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await page.waitForTimeout(3000);
    check(!(await hasFrame(page)), "three seconds after the hero scrolls away the frame is gone");
    await ctx.close();
  }

  console.log("\nno frame unless asked");
  for (const [name, opts, init] of [
    ["reduced motion", { reducedMotion: "reduce" }, null],
    ["Save-Data", {}, () => Object.defineProperty(Navigator.prototype, "connection", { get: () => ({ saveData: true, effectiveType: "4g" }) })],
  ]) {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, ...opts });
    if (init) await ctx.addInitScript(init);
    const page = await ctx.newPage();
    await page.goto(URL_);
    await page.waitForTimeout(4000);
    check(!(await hasFrame(page)) && (await page.evaluate(() => !document.querySelector("[data-play]").hidden)), `${name}: the still and Play, no frame`);
    await ctx.close();
  }

  console.log("\ntimed, warnings only (CPU 4x slower, a phone's screen)");
  {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
    const page = await ctx.newPage();
    const cdp = await ctx.newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
    // The review's slow 4G. Headless Chrome draws WebGL in software (SwiftShader) where there is
    // no GPU, as on a runner, so the main-thread share here reads far over a real phone's.
    await cdp.send("Network.enable");
    await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 150, downloadThroughput: (1.6 * 1024 * 1024) / 8, uploadThroughput: (750 * 1024) / 8 });
    await cdp.send("Performance.enable");
    await page.goto(URL_);
    const ok = await live(page, 30_000).then(
      () => true,
      () => false,
    );
    const liveMs = ok ? await page.evaluate(() => performance.now()) : Infinity;
    warn(liveMs <= WARN.liveMs, `Ghost comes to life ${(liveMs / 1000).toFixed(2)} s after navigation (ceiling ${WARN.liveMs / 1000} s)`);
    const get = async () => Object.fromEntries((await cdp.send("Performance.getMetrics")).metrics.map((x) => [x.name, x.value]));
    await page.waitForTimeout(1500);
    const a = await get();
    const t = Date.now();
    await page.waitForTimeout(10_000);
    const b = await get();
    const running = ((b.TaskDuration - a.TaskDuration) / ((Date.now() - t) / 1000)) * 100;
    warn(running <= WARN.mainThreadPct, `while it plays the main thread is busy ${running.toFixed(1)}% of the time (ceiling ${WARN.mainThreadPct}%)`);
    await ctx.close();
  }
} finally {
  await browser.close();
  try {
    process.kill(-server.pid);
  } catch {
    server.kill();
  }
}
console.log(failed ? `\n${failed} over budget` : "\nwithin budget");
process.exit(failed ? 1 : 0);
