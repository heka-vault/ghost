#!/usr/bin/env node
/**
 * The phone's screens from the web build on the mock desk, for reviewing a visual change: the
 * pairing screens, Home, a session in each phase, the ask stage, the agents on Home and the
 * brand workbench (`app/brand.tsx`). No phone, no desk, no network: Expo's web export served
 * from this process and driven with headless Chrome (packages/ui/scripts/chrome.mjs).
 *
 *   node apps/mobile/scripts/shots.mjs <out-dir> [web-dir]
 *
 * Without `web-dir` it runs `expo export --platform web` first. Each shot is 412x915 at 2x, the
 * dark theme unless the name ends in -light. Needs Chrome and Node 22+.
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";

const HERE = dirname(fileURLToPath(import.meta.url));
const MOBILE = join(HERE, "..");
const { launch } = await import(join(MOBILE, "..", "..", "packages", "ui", "scripts", "chrome.mjs"));

const [out, given] = process.argv.slice(2);
if (!out) {
  console.error("shots: <out-dir> [web-dir]");
  process.exit(2);
}
mkdirSync(out, { recursive: true });
const web = given ?? join(tmpdir(), `ghost-mobile-web-${process.pid}`);
if (!given) execFileSync("npx", ["expo", "export", "--platform", "web", "--output-dir", web], { cwd: MOBILE, stdio: "inherit" });

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".png": "image/png", ".ttf": "font/ttf", ".json": "application/json", ".ico": "image/x-icon", ".svg": "image/svg+xml" };
const server = createServer((req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^(\.\.[/\\])+/, "");
  let file = join(web, path);
  if (!file.startsWith(web) || !existsSync(file) || !extname(file)) file = join(web, "index.html");
  res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
  res.end(readFileSync(file));
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const base = `http://127.0.0.1:${server.address().port}`;

const W = 412;
const H = 915;
const cdp = await launch();
try {
  for (const scheme of ["dark", "light"]) {
    const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    const s = (m, p) => cdp.send(m, p, sessionId);
    await s("Page.enable");
    await s("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 2, mobile: true });
    await s("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }, { name: "prefers-reduced-motion", value: "no-preference" }] });
    const evaluate = async (expression) => (await s("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result.value;
    const go = async (path) => {
      await s("Page.navigate", { url: base + path });
      await sleep(2500);
    };
    const shot = async (name, full = false) => {
      const h = full ? await evaluate("Math.max(document.body.scrollHeight, ...[...document.querySelectorAll('div')].map(d => d.scrollHeight))") : H;
      if (full) await s("Emulation.setDeviceMetricsOverride", { width: W, height: h, deviceScaleFactor: 2, mobile: true });
      const { data } = await s("Page.captureScreenshot", { format: "png" });
      const file = join(out, `${name}${scheme === "light" ? "-light" : ""}.png`);
      writeFileSync(file, Buffer.from(data, "base64"));
      if (full) await s("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 2, mobile: true });
      console.log(`shots: ${file}`);
    };
    /** Press the element whose label or own text is `text`, as a finger would (pointer events). */
    const press = async (text) => {
      const at = await evaluate(`(() => {
        const t = ${JSON.stringify(text)};
        const all = [...document.querySelectorAll('[aria-label], div, span')].filter(e => e.offsetParent !== null || e.getClientRects().length);
        const el = all.find(e => e.getAttribute('aria-label') === t) ?? all.reverse().find(e => e.textContent.trim() === t);
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return [r.x + r.width / 2, r.y + r.height / 2];
      })()`);
      if (!at) throw new Error(`shots: nothing says "${text}"`);
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await s("Input.dispatchMouseEvent", { type, x: at[0], y: at[1], button: "left", clickCount: 1 });
    };

    await go("/pair");
    await shot("pair");
    if (scheme === "light") {
      await cdp.send("Target.closeTarget", { targetId });
      continue;
    }
    await press("Camera. Mock: tap to pair with the mock desk");
    await sleep(1800);
    await shot("pair-done");
    await press("Start using Ghost");
    await sleep(2500);
    await shot("home");
    await shot("home-full", true);
    await press("Start session");
    await sleep(900);
    await shot("session-speaking");
    await sleep(3200);
    await shot("session-listening");
    await press("Mock desk controls");
    await sleep(600);
    await press("Ask Ghost something");
    await sleep(500);
    await shot("session-thinking");
    await sleep(1600);
    await shot("session-after-talk");
    await sleep(4000);
    await press("Mock desk controls");
    await sleep(600);
    await press("Ghost asks to close infra#77");
    await sleep(1400);
    await shot("session-ask");
    await cdp.send("Target.closeTarget", { targetId });
  }
  // The workbench, in both themes, at full length.
  for (const scheme of ["dark", "light"]) {
    const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    const s = (m, p) => cdp.send(m, p, sessionId);
    await s("Page.enable");
    await s("Emulation.setDeviceMetricsOverride", { width: 700, height: 2600, deviceScaleFactor: 1, mobile: true });
    await s("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
    await s("Page.navigate", { url: `${base}/brand` });
    await sleep(3500);
    const { data } = await s("Page.captureScreenshot", { format: "png" });
    const file = join(out, `brand${scheme === "light" ? "-light" : ""}.png`);
    writeFileSync(file, Buffer.from(data, "base64"));
    console.log(`shots: ${file}`);
    await cdp.send("Target.closeTarget", { targetId });
  }
} finally {
  await cdp.close();
  server.close();
}
