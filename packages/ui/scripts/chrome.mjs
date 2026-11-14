/**
 * Headless Chrome over the DevTools protocol, the way scripts/floating-shots.mjs drives it: no
 * Playwright, no Puppeteer, Node 22+ (its own WebSocket). Opens a file and captures exact clips.
 * Used by marks.mjs to turn the mark's SVG into pixels with the renderer the apps draw it with.
 */
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";

export async function launch() {
  const chrome = process.env.CHROME ?? "google-chrome";
  const profile = join(tmpdir(), `ghost-mark-${process.pid}`);
  mkdirSync(profile, { recursive: true });
  const proc = spawn(
    chrome,
    ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--hide-scrollbars", "--allow-file-access-from-files", "--font-render-hinting=none", "about:blank"],
    { stdio: "ignore" },
  );
  const portFile = join(profile, "DevToolsActivePort");
  for (let i = 0; i < 100 && !existsSync(portFile); i++) await sleep(100);
  const [port, path] = readFileSync(portFile, "utf8").trim().split("\n");
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
      if (msg.error) fail(new Error(msg.error.message));
      else ok(msg.result);
    } else for (const h of handlers) h(msg);
  };
  const send = (method, params = {}, sessionId) =>
    new Promise((ok, fail) => {
      const id = next++;
      waiting.set(id, { ok, fail });
      ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  return {
    send,
    handlers,
    async close() {
      ws.close();
      const gone = new Promise((ok) => proc.once("exit", ok));
      proc.kill();
      await gone;
      rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    },
  };
}

/** Open a file at a viewport; returns helpers bound to that page. */
export async function open(cdp, file, { width = 1200, height = 800, scale = 1, transparent = false } = {}) {
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  const s = (m, p) => cdp.send(m, p, sessionId);
  await s("Page.enable");
  await s("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: scale, mobile: false });
  if (transparent) await s("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
  const loaded = new Promise((ok) => {
    const h = (m) => {
      if (m.sessionId === sessionId && m.method === "Page.loadEventFired") {
        cdp.handlers.delete(h);
        ok();
      }
    };
    cdp.handlers.add(h);
  });
  await s("Page.navigate", { url: pathToFileURL(file).href });
  await loaded;
  await s("Runtime.evaluate", { expression: "document.fonts.ready.then(() => true)", awaitPromise: true });
  await sleep(150);
  const evaluate = async (expression) => (await s("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result.value;
  return {
    evaluate,
    async shot(clip) {
      const { data } = await s("Page.captureScreenshot", { format: "png", captureBeyondViewport: true, ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
      return Buffer.from(data, "base64");
    },
    /** Rectangles of every element matching a selector, by its data-name. */
    rects: (sel) => evaluate(`[...document.querySelectorAll(${JSON.stringify(sel)})].map(e => { const r = e.getBoundingClientRect(); return { name: e.dataset.name, x: r.x + scrollX, y: r.y + scrollY, width: r.width, height: r.height }; })`),
    resize: (w, h) => s("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: scale, mobile: false }),
    close: () => cdp.send("Target.closeTarget", { targetId }),
  };
}

export function magick(args) {
  return new Promise((ok, fail) => {
    const p = spawn(process.env.MAGICK ?? "magick", args, { stdio: ["ignore", "ignore", "inherit"] });
    p.on("exit", (c) => (c === 0 ? ok() : fail(new Error(`magick ${args.join(" ")} exited ${c}`))));
  });
}
