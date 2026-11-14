#!/usr/bin/env node
// The README's pictures of the floating room (ADR 0059), from the mock daemon: no daemon, no
// data of anyone's, no network. Headless Chrome opens apps/desktop/shots/desktop.html (a
// made-up desktop) with the real floating page laid over it in a transparent iframe, playing
// `?mock=floating-tour` on its fixed clock, and takes each picture at its moment; the loop is
// a screencast of the whole turn, resampled to a steady frame rate. Run by scripts/screenshots.sh
// (make screenshots) with the UI served; Node 22 or later (its own WebSocket), ImageMagick.
//
//   node scripts/floating-shots.mjs <url> <out-dir> <work-dir> [name ...]
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const [url, out, work, ...want] = process.argv.slice(2);
const chrome = process.env.CHROME;
if (!url || !out || !work || !chrome) {
  console.error("floating-shots: usage: CHROME=… floating-shots.mjs <url> <out-dir> <work-dir> [name ...]");
  process.exit(2);
}

/** The desktop is 1280×720 CSS pixels (a laptop's screen; Ghost draws at 0.67 of its full size there). */
const W = 1280;
const H = 720;

// name | the floating page's query | when, in ms from load | crop (x, y, w, h in CSS px) or none
const STILLS = [
  // Six places, the beams out and the first answers on their way back.
  { name: "floating-hero", query: "mock=floating-tour", at: 4500 },
  // Ghost speaking: the voice moves Wisp.
  { name: "floating-speaking", query: "mock=floating-tour", at: 7700, crop: [560, 28, 720, 405] },
  // A change waiting for a yes, in the corner.
  { name: "floating-yes", query: "mock=floating-tour&placement=corner", at: 17600, crop: [640, 360, 640, 360] },
  // An agent's question, beside the work.
  { name: "floating-question", query: "mock=floating-tour", at: 25500, crop: [560, 120, 720, 405] },
];
/** The loop: one whole turn, the wake to the line again. */
const LOOP = { name: "floating-loop", query: "mock=floating-tour", from: 300, to: 12_300, fps: 12 };

const selected = (name) => want.length === 0 || want.includes(name);

// ---------------------------------------------------------------- a minimal CDP client
async function launch() {
  const profile = join(work, "profile");
  mkdirSync(profile, { recursive: true });
  const proc = spawn(
    chrome,
    [
      "--headless=new",
      "--remote-debugging-port=0",
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--hide-scrollbars",
      "--force-dark-mode",
      "--enable-unsafe-swiftshader",
      "--use-angle=swiftshader",
      "--autoplay-policy=no-user-gesture-required",
      `--window-size=${W},${H}`,
      "about:blank",
    ],
    { stdio: "ignore" },
  );
  const portFile = join(profile, "DevToolsActivePort");
  for (let i = 0; i < 100 && !existsSync(portFile); i++) await sleep(100);
  const [port, path] = readFileSync(portFile, "utf8").trim().split("\n");
  // Chrome writes the port a moment before it answers on it: try again for a few seconds.
  let ws;
  for (let i = 0; ; i++) {
    ws = new WebSocket(`ws://127.0.0.1:${port}${path}`);
    const open = await new Promise((ok) => {
      ws.onopen = () => ok(true);
      ws.onerror = () => ok(false);
    });
    if (open) break;
    if (i === 30) throw new Error("Chrome's DevTools never answered");
    await sleep(200);
  }
  let next = 1;
  const waiting = new Map();
  const handlers = new Set();
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && waiting.has(msg.id)) {
      const { ok, fail } = waiting.get(msg.id);
      waiting.delete(msg.id);
      if (msg.error) fail(new Error(`${msg.error.message}`));
      else ok(msg.result);
    } else for (const h of handlers) h(msg);
  };
  const send = (method, params = {}, sessionId) =>
    new Promise((ok, fail) => {
      const id = next++;
      waiting.set(id, { ok, fail });
      ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  return { proc, ws, send, handlers };
}

async function openPage(cdp, query, scale) {
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  const s = (method, params) => cdp.send(method, params, sessionId);
  await s("Page.enable");
  await s("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: scale, mobile: false });
  await s("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
  const loaded = new Promise((ok) => {
    const h = (m) => {
      if (m.sessionId === sessionId && m.method === "Page.loadEventFired") {
        cdp.handlers.delete(h);
        ok();
      }
    };
    cdp.handlers.add(h);
  });
  await s("Page.navigate", { url: `${url}/shots/desktop.html?float=${encodeURIComponent(query)}` });
  await loaded;
  // The tour's clock starts when the floating page first subscribes to the mock daemon.
  const expr = "document.getElementById('ghost').contentWindow.ghostTourStart ?? null";
  for (let i = 0; i < 200; i++) {
    const { result } = await s("Runtime.evaluate", { expression: expr, returnByValue: true });
    if (typeof result.value === "number") return { s, sessionId, targetId, t0: result.value };
    await sleep(25);
  }
  throw new Error("the floating page never started the tour (is the UI served from this checkout?)");
}

const close = (cdp, page) => cdp.send("Target.closeTarget", { targetId: page.targetId });

function magick(args) {
  return new Promise((ok, fail) => {
    const bin = process.env.MAGICK ?? "magick";
    const p = spawn(bin, args, { stdio: ["ignore", "ignore", "inherit"] });
    p.on("exit", (code) => (code === 0 ? ok() : fail(new Error(`${bin} ${args.join(" ")} exited ${code}`))));
  });
}

// ---------------------------------------------------------------- the stills
async function still(cdp, shot) {
  const page = await openPage(cdp, shot.query, 2);
  const wait = shot.at - (Date.now() - page.t0);
  if (wait > 0) await sleep(wait);
  const clip = shot.crop ? { x: shot.crop[0], y: shot.crop[1], width: shot.crop[2], height: shot.crop[3], scale: 1 } : undefined;
  const { data } = await page.s("Page.captureScreenshot", { format: "png", ...(clip ? { clip } : {}) });
  const late = Date.now() - page.t0 - shot.at;
  await close(cdp, page);
  const png = join(work, `${shot.name}.png`);
  writeFileSync(png, Buffer.from(data, "base64"));
  // 1660 px wide is twice the README's column: sharp on a high-density screen, small enough.
  const webp = join(out, `${shot.name}.webp`);
  await magick([png, "-resize", "1660x>", "-quality", "86", "-define", "webp:method=6", webp]);
  console.log(`floating-shots: ${webp} (taken ${late} ms after its moment)`);
}

// ---------------------------------------------------------------- the loop
async function loop(cdp) {
  const page = await openPage(cdp, LOOP.query, 1);
  const frames = [];
  cdp.handlers.add((m) => {
    if (m.sessionId !== page.sessionId || m.method !== "Page.screencastFrame") return;
    frames.push({ t: Date.now() - page.t0, data: m.params.data });
    void page.s("Page.screencastFrameAck", { sessionId: m.params.sessionId });
  });
  await page.s("Page.startScreencast", { format: "png", maxWidth: W, maxHeight: H, everyNthFrame: 1 });
  await sleep(LOOP.to - (Date.now() - page.t0) + 300);
  await page.s("Page.stopScreencast");
  await close(cdp, page);
  if (frames.length === 0) throw new Error("the screencast sent no frames");
  // A steady rate: for each tick, the newest frame at or before it.
  const dir = join(work, "loop");
  mkdirSync(dir, { recursive: true });
  const step = 1000 / LOOP.fps;
  const files = [];
  let j = 0;
  for (let t = LOOP.from, i = 0; t <= LOOP.to; t += step, i++) {
    while (j + 1 < frames.length && frames[j + 1].t <= t) j++;
    const f = join(dir, `${String(i).padStart(4, "0")}.png`);
    writeFileSync(f, Buffer.from(frames[j].data, "base64"));
    files.push(f);
  }
  const gaps = frames.slice(1).map((f, i) => f.t - frames[i].t).filter((g) => g > 0);
  console.log(`floating-shots: ${frames.length} screencast frames, median gap ${gaps.sort((a, b) => a - b)[gaps.length >> 1] ?? "?"} ms`);
  const webp = join(out, `${LOOP.name}.webp`);
  await magick([
    "-delay", String(Math.round(100 / LOOP.fps)), ...files,
    "-quality", "70", "-define", "webp:method=6", "-loop", "0", webp,
  ]);
  console.log(`floating-shots: ${webp} (${files.length} frames at ${LOOP.fps} a second)`);
}

const cdp = await launch();
try {
  for (const shot of STILLS) if (selected(shot.name)) await still(cdp, shot);
  if (selected(LOOP.name)) await loop(cdp);
} finally {
  cdp.ws.close();
  cdp.proc.kill();
}
