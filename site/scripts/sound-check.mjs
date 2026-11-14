/**
 * The hero's sound in real browsers, headless, with nothing played aloud (docs/site.md):
 *
 *   pnpm --dir site site:build && pnpm --dir site site:sound-check [--video out.mp4]
 *
 * Against `astro preview` of site/dist, in Chrome (the system's) and Playwright's Firefox
 * (`pnpm --dir site exec playwright-core install firefox`). A browser Playwright drives plays
 * without a gesture (it mutes Chrome, and its Firefox allows autoplay whatever the prefs say),
 * so the browsers' own rule is checked apart, in the system's Chrome and Firefox started by hand
 * with fresh profiles, headless, on a page that tries to play with no gesture and reports back:
 *
 * - stock Chrome and Firefox refuse to play without a gesture (NotAllowedError);
 * - before the press, no clip is fetched (the timeline is: it is the silent float's clock);
 * - "Hear it" plays one clip (WebM/Opus where the browser takes it), the float starts its turn
 *   again, and each scene of the float is logged against the clip's own clock: the request's
 *   start, the reply, the yes, the goodbye, the rest, within 150 ms of the timeline;
 * - captions follow; pressing again stops it; with reduced motion the sound plays over the still.
 *
 * `--video out.mp4`: Chrome's screen capture of one turn with the clip muxed in at the moment it
 * started, for a reviewer to watch and hear.
 */
import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium, firefox } from "playwright-core";

const site = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const turn = JSON.parse(readFileSync(join(site, "../apps/desktop/src/design/mock/fixtures/floating-site-turn.json"), "utf8"));
const at = (id) => turn.lines.find((l) => l.id === id);
const PORT = 4339;
const URL_ = `http://localhost:${PORT}/`;
const videoOut = process.argv.includes("--video") ? resolve(process.argv[process.argv.indexOf("--video") + 1]) : null;

// Every media element the page plays, so the check can read the clip's own clock.
const HOOK = () => {
  window.__audios = [];
  const play = HTMLMediaElement.prototype.play;
  HTMLMediaElement.prototype.play = function () {
    if (!window.__audios.includes(this)) window.__audios.push(this);
    return play.call(this);
  };
};

const server = spawn("pnpm", ["exec", "astro", "preview", "--port", String(PORT)], { cwd: site, stdio: "ignore" });
for (let i = 0; i < 100; i++) {
  try {
    if ((await fetch(URL_)).ok) break;
  } catch {}
  await new Promise((r) => setTimeout(r, 100));
}

let failed = 0;
const check = (ok, what) => {
  console.log(`  ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failed++;
};

// The browsers' own rule: the system's Chrome and Firefox, fresh profiles, no Playwright, try to
// play the clip with no gesture and say what happened to a little server here.
async function stockRule() {
  const results = {};
  const clip = readFileSync(join(site, "public/audio/hero-turn.m4a"));
  const probe = createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    if (url.pathname === "/clip") return res.writeHead(200, { "content-type": "audio/mp4" }), res.end(clip);
    if (url.pathname === "/result") results[url.searchParams.get("b")] = url.searchParams.get("r");
    res.writeHead(200, { "content-type": "text/html" });
    res.end(`<!doctype html><script>new Audio("/clip").play().then(()=>"played",e=>e.name).then(r=>fetch("/result?b="+new URLSearchParams(location.search).get("b")+"&r="+r))</script>`);
  });
  await new Promise((r) => probe.listen(PORT + 1, "127.0.0.1", r));
  const page = (b) => `http://127.0.0.1:${PORT + 1}/?b=${b}`;
  // Run until the page has answered (or the time is up); the server must keep serving meanwhile.
  const run = (bin, args, name, ms) =>
    new Promise((done) => {
      const child = spawn(bin, args, { stdio: "ignore" });
      const end = () => (clearInterval(poll), clearTimeout(cap), child.kill(), done());
      const poll = setInterval(() => results[name] && end(), 100);
      const cap = setTimeout(end, ms);
      child.on("exit", () => (clearInterval(poll), clearTimeout(cap), done()));
    });
  const chromeProfile = mkdtempSync(join(tmpdir(), "ghost-rule-"));
  await run("/usr/bin/google-chrome", ["--headless=new", `--user-data-dir=${chromeProfile}`, "--no-first-run", page("chrome")], "chrome", 15_000);
  rmSync(chromeProfile, { recursive: true, force: true });
  if (existsSync("/usr/bin/firefox")) {
    // The snap's Firefox reads profiles only under its own folder.
    const ffProfile = mkdtempSync(join(existsSync(join(homedir(), "snap/firefox/common")) ? join(homedir(), "snap/firefox/common") : tmpdir(), "ghost-rule-"));
    await run("/usr/bin/firefox", ["--headless", "--no-remote", "--profile", ffProfile, page("firefox")], "firefox", 25_000);
    await new Promise((r) => setTimeout(r, 1000)); // let it let go of the profile
    rmSync(ffProfile, { recursive: true, force: true });
  }
  probe.close();
  return results;
}
console.log("\nthe browsers' own autoplay rule (stock, fresh profiles)");
const rule = await stockRule();
for (const b of ["chrome", "firefox"]) check(rule[b] === "NotAllowedError", `${b} refuses to play without a gesture: ${rule[b] ?? "no answer"}`);

const browsers = [
  {
    name: "chrome",
    launch: () => chromium.launch({ executablePath: "/usr/bin/google-chrome" }),
  },
  {
    name: "firefox",
    launch: () => firefox.launch(),
  },
];

try {
  for (const b of browsers) {
    console.log(`\n${b.name}`);
    const browser = await b.launch();
    const videoDir = videoOut && b.name === "chrome" ? mkdtempSync(join(tmpdir(), "ghost-sound-")) : null;
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, ...(videoDir ? { recordVideo: { dir: videoDir, size: { width: 1440, height: 900 } } } : {}) });
    await ctx.addInitScript(HOOK);
    const requests = [];
    ctx.on("request", (r) => requests.push(r.url()));
    const page = await ctx.newPage();
    const videoT0 = Date.now();

    await page.goto(URL_);
    await page.waitForFunction(() => document.querySelector("[data-hero][data-live]"), null, { timeout: 15_000 });
    await page.waitForTimeout(3000);
    // The timeline (floating-site-turn) is the silent float's clock too, so the float loads it; the clip waits.
    check(!requests.some((u) => /\/audio\//.test(u)), `no clip fetched before the press (${requests.length} requests)`);
    check(await page.evaluate(() => window.__audios.length === 0), "no media element played before the press");

    // The press.
    await page.click("[data-hear]");
    const pressedAt = Date.now();
    await page.waitForFunction(() => window.__audios[0] && !window.__audios[0].paused && window.__audios[0].currentTime > 0, null, { timeout: 10_000 });
    const src = await page.evaluate(() => window.__audios[0].currentSrc);
    check(/hero-turn\.(webm|m4a)$/.test(src), `playing ${src.replace(/.*\//, "")}`);
    const clips = new Set(requests.filter((u) => /\/audio\//.test(u)));
    check(clips.size === 1 && clips.has(src), `only the one clip the browser takes is fetched (${[...clips].map((u) => u.replace(/.*\//, "")).join(", ")})`);
    check(await page.evaluate(() => document.querySelector("[data-hear]").getAttribute("aria-pressed") === "true"), "the control says it is on");

    // Each scene the float shows, on the clip's clock.
    const frame = page.frames().find((f) => f.url().includes("/float/"));
    await frame.evaluate(() => {
      const audio = parent.__audios[0];
      window.__scenes = [];
      let last = "";
      const look = () => {
        const s = document.querySelector(".flt-root")?.getAttribute("data-scene") ?? "";
        if (s !== last) window.__scenes.push({ scene: s, clip: Math.round(audio.currentTime * 1000), wall: Date.now() });
        last = s;
      };
      setInterval(look, 20); // a timer, not frames: a headless frame's rAF may be throttled
    });
    const captions = [];
    const until = Date.now() + turn.loop_ms + 1500;
    let loopStartWall = null;
    while (Date.now() < until) {
      const c = await page.evaluate(() => {
        const el = document.querySelector("[data-caption]");
        const a = window.__audios[0];
        return { text: el.hidden ? "" : el.textContent, clip: Math.round(a.currentTime * 1000), wall: Date.now() };
      });
      if (c.clip < 400 && loopStartWall === null) loopStartWall = c.wall - c.clip;
      if (c.text && captions.at(-1)?.text !== c.text) captions.push(c);
      await page.waitForTimeout(100);
    }
    const scenes = await frame.evaluate(() => window.__scenes);
    // The turn as the clip has it, and when the float showed it (first sighting after a fresh start).
    const want = [
      ["you", at("request").at],
      ["thinking", at("request").at + at("request").ms],
      ["speaking", at("reply").at],
      ["okay", at("ask").at],
      ["ending", at("bye").at],
      ["line", turn.sleep],
    ];
    // The first whole turn: from the press to where the clip wraps for the next one.
    const wrap = scenes.findIndex((s, i) => i > 0 && s.clip < scenes[i - 1].clip - 1000);
    const run = wrap > 0 ? scenes.slice(0, wrap) : scenes;
    console.log("  scene      clip says   float shows   off");
    for (const [scene, ms] of want) {
      const seen = run.filter((s) => s.scene === scene).sort((x, y) => Math.abs(x.clip - ms) - Math.abs(y.clip - ms))[0];
      const off = seen ? seen.clip - ms : NaN;
      console.log(`  ${scene.padEnd(9)} ${String(ms).padStart(7)} ms  ${String(seen?.clip ?? "-").padStart(8)} ms  ${Number.isNaN(off) ? "-" : `${off > 0 ? "+" : ""}${off} ms`}`);
      check(Math.abs(off) <= 150, `${scene} within 150 ms of the clip`);
    }
    console.log(`  scenes seen: ${run.map((s) => `${s.scene}@${s.clip}`).join(" ")}`);
    console.log(`  captions: ${captions.map((c) => `${c.clip}ms "${c.text}"`).join(" | ")}`);
    check(captions.some((c) => /Ghost.*The review is at four/.test(c.text)), "the reply is captioned");
    check(captions.some((c) => /You.*Hey Ghost/.test(c.text)), "the wake is captioned");

    await page.click("[data-hear]");
    await page.waitForTimeout(300);
    check(await page.evaluate(() => window.__audios[0].paused && document.querySelector("[data-caption]").hidden), "pressed again, the sound stops and the captions go");

    await page.close();
    await ctx.close();
    if (videoDir && loopStartWall !== null) {
      const webm = join(videoDir, readdirSync(videoDir).find((f) => f.endsWith(".webm")));
      const offset = (loopStartWall - videoT0) / 1000;
      const clip = join(site, "public/audio/hero-turn.m4a");
      // The capture from just before the press, through one whole turn, with the clip where it played.
      const from = Math.max(0, (pressedAt - videoT0) / 1000 - 1.5);
      execFileSync("ffmpeg", [
        "-hide_banner", "-loglevel", "error", "-y",
        "-ss", String(from), "-i", webm,
        "-itsoffset", String(offset - from), "-i", clip,
        "-map", "0:v", "-map", "1:a", "-t", String(turn.loop_ms / 1000 + 2),
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "26", "-preset", "veryfast", "-c:a", "aac", "-b:a", "96k",
        videoOut,
      ]);
      console.log(`  video: ${videoOut} (clip placed ${offset.toFixed(2)} s into the capture)`);
    }

    // Reduced motion: the still stays, and the sound still plays over it.
    const still = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: "reduce" });
    await still.addInitScript(HOOK);
    const p2 = await still.newPage();
    await p2.goto(URL_);
    await p2.waitForTimeout(2500);
    await p2.click("[data-hear]");
    await p2.waitForFunction(() => window.__audios[0] && !window.__audios[0].paused && window.__audios[0].currentTime > 0.5, null, { timeout: 10_000 });
    check(await p2.evaluate(() => !document.querySelector(".hero__frame")), "reduced motion: the sound plays over the still, no float loaded");
    await still.close();
    await browser.close();
  }
} finally {
  server.kill();
  // Astro's preview server outlives the command that started it: stop it by name.
  spawn("pnpm", ["exec", "astro", "preview", "stop"], { cwd: site, stdio: "ignore" });
}
if (!existsSync(join(site, "dist"))) console.log("build the site first");
console.log(failed ? `\n${failed} failed` : "\nall ok");
process.exit(failed ? 1 : 0);
