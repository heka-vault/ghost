#!/usr/bin/env node
/**
 * Writes Ghost's mark as files, from packages/ui/src/mark.ts (the one drawing; docs/design-system.md,
 * The mark). Node 22.15 or later (type stripping and module hooks), Chrome (the renderer the
 * apps draw the SVG with), ImageMagick (ICO and WebP).
 *
 *   node packages/ui/scripts/marks.mjs desktop      the desktop app's icons, tray icons and favicons (committed)
 *   node packages/ui/scripts/marks.mjs all <dir>    every variant as SVG and PNG into <dir>, for the phone and the site
 *   node packages/ui/scripts/marks.mjs dock <file>  a review sheet: the icon at its real pixels on a dock, a tray and a tab
 *
 * `make marks` runs the first. Everything is rendered at its own pixel size (no downscaling a
 * master), so the ladder's levels of detail land on whole pixels.
 */
import { registerHooks } from "node:module";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// mark.ts imports "./figure" with no extension, as the apps' bundlers want: resolve it to .ts.
registerHooks({
  resolve(spec, ctx, next) {
    try {
      return next(spec, ctx);
    } catch (e) {
      if (spec.startsWith(".")) return next(`${spec}.ts`, ctx);
      throw e;
    }
  },
});

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = join(HERE, "..", "..", "..");
const M = await import(join(HERE, "..", "src", "mark.ts"));
const { launch, open, magick } = await import(join(HERE, "chrome.mjs"));

const INTER = join(REPO, "apps/desktop/node_modules/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2");
const fontFace = () => {
  try {
    return `@font-face{font-family:'Inter Variable';font-weight:100 900;src:url(data:font/woff2;base64,${readFileSync(INTER).toString("base64")}) format('woff2')}`;
  } catch {
    return "";
  }
};

/** Render SVGs to PNGs at their own pixel sizes in one Chrome page. `files`: { name, svg, px, w?, h? }. */
async function rasterize(files, outDir, work) {
  mkdirSync(work, { recursive: true });
  let x = 0;
  let y = 0;
  let rowH = 0;
  const W = 2400;
  const cells = files.map((f) => {
    const w = f.w ?? f.px;
    const h = f.h ?? f.px;
    if (x + w > W) {
      x = 0;
      y += rowH + 4;
      rowH = 0;
    }
    const c = { ...f, w, h, x, y };
    x += w + 4;
    rowH = Math.max(rowH, h);
    return c;
  });
  cells.forEach((c, i) => writeFileSync(join(work, `${i}.svg`), c.svg));
  const page = join(work, "atlas.html");
  writeFileSync(
    page,
    `<!doctype html><meta charset="utf-8"><style>html,body{margin:0;background:transparent}img{position:absolute;display:block}</style>` +
      cells.map((c, i) => `<img src="${i}.svg" width="${c.w}" height="${c.h}" style="left:${c.x}px;top:${c.y}px">`).join(""),
  );
  const cdp = await launch();
  try {
    const p = await open(cdp, page, { width: W, height: y + rowH + 4, transparent: true });
    for (const c of cells) {
      const out = join(outDir, c.name);
      mkdirSync(dirname(out), { recursive: true });
      writeFileSync(out, await p.shot({ x: c.x, y: c.y, width: c.w, height: c.h }));
    }
    await p.close();
  } finally {
    await cdp.close();
  }
}

const work = join(REPO, "target", "marks-work");
const [cmd, arg] = process.argv.slice(2);

if (cmd === "desktop") {
  const icons = join(REPO, "apps/desktop/src-tauri/icons");
  const files = [...M.desktopIconFiles(), ...M.trayIconFiles(32).map((f) => ({ ...f, name: `tray/${f.name.replace(/^tray-/, "")}` }))];
  // The favicon's sizes, for the .ico (the window's page and the stage window).
  const fav = [16, 24, 32, 48, 64].map((px) => ({ name: `ico/${px}.png`, px, svg: M.markSvg(px) }));
  await rasterize([...files, ...fav], icons, work);
  await magick([...[16, 24, 32, 48, 64].map((px) => join(icons, `ico/${px}.png`)), join(icons, "icon.ico")]);
  rmSync(join(icons, "ico"), { recursive: true, force: true });
  const pub = join(REPO, "apps/desktop/public");
  writeFileSync(join(pub, "favicon.svg"), M.markSvg(32, { id: "fav" }) + "\n");
  await magick([join(icons, "icon.ico"), join(pub, "favicon.ico")]);
  console.log(`marks: ${files.length} icons in ${icons}, favicon.svg and favicon.ico in ${pub}`);
} else if (cmd === "all") {
  if (!arg) throw new Error("marks: all <dir>");
  const out = arg;
  const files = [];
  const add = (name, svg, px, w, h) => {
    files.push({ name: `png/${name}.png`, svg, px, w, h });
    mkdirSync(join(out, "svg"), { recursive: true });
    writeFileSync(join(out, "svg", `${name}.svg`), svg);
  };
  for (const px of [16, 20, 24, 32, 36, 40, 48, 64, 96, 128, 180, 192, 256, 512, 1024]) add(`mark-${px}`, M.markSvg(px), px);
  for (const st of Object.keys(M.MARK_STATES)) {
    for (const px of [16, 32, 48, 128, 256]) add(`mark-${st}-${px}`, M.markSvg(px, { state: st }), px);
    add(`chip-${st}-36`, M.markSvg(36, { state: st, ring: "arc" }), 36);
    add(`chip-${st}-72`, M.markSvg(72, { state: st, ring: "arc" }), 72);
  }
  for (const px of [16, 24, 32, 48, 96, 256]) {
    add(`mono-light-${px}`, M.monoSvg(px, M.INK), px);
    add(`mono-dark-${px}`, M.monoSvg(px, M.NIGHT), px);
  }
  for (const s of ["rest", "listening", "reaching", "speaking", "waiting", "off"]) for (const px of [24, 32, 48]) add(`tray-${s}-${px}`, M.traySvg(px, s), px);
  for (const l of ["background", "foreground", "monochrome"]) add(`adaptive-${l}`, M.adaptiveSvg(l), 432);
  const face = fontFace();
  for (const theme of ["dark", "light"]) {
    for (const h of [32, 64, 128]) {
      const svg = M.wordmarkSvg(h, theme, { fontFace: face });
      const w = Number(/width="(\d+)"/.exec(svg)[1]);
      add(`wordmark-${theme}-${h}`, svg, h, w, h);
    }
  }
  await rasterize(files, out, work);
  await magick([...[16, 24, 32, 48, 64].map((px) => join(out, "png", px === 64 ? "mark-64.png" : `mark-${px}.png`)), join(out, "favicon.ico")]);
  console.log(`marks: ${files.length} files as svg/ and png/ in ${out}, and favicon.ico`);
} else if (cmd === "dock") {
  if (!arg) throw new Error("marks: dock <file.png>");
  // A made-up dock (GNOME's, dark and light), a top bar with the tray, and a browser tab, at 1x.
  const others = (px) => ["#2f6fdb", "#e0533b", "#3bb36b", "#8a5cf6"].map((c) => `<div style="width:${px}px;height:${px}px;border-radius:${px * 0.22}px;background:linear-gradient(160deg,${c},#111)"></div>`).join("");
  const img = (svg, px) => `<img width="${px}" height="${px}" src="data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}">`;
  const dock = (px, bg) => `<div style="display:flex;gap:${px * 0.25}px;padding:${px * 0.2}px;background:${bg};border-radius:${px * 0.3}px;width:max-content;align-items:center">${others(px)}${img(M.markSvg(px), px)}</div>`;
  const tray = (bg, fg) => `<div style="display:flex;gap:14px;align-items:center;padding:4px 14px;height:24px;background:${bg};color:${fg};font:600 13px Inter, sans-serif;width:max-content">${["rest", "listening", "speaking", "waiting", "off"].map((s) => img(M.traySvg(32, s), 22)).join("")}<span>Thu 14:32</span></div>`;
  const tab = (bg, fg) => `<div style="display:flex;gap:8px;align-items:center;padding:8px 12px;background:${bg};color:${fg};font:13px Inter, sans-serif;width:220px;border-radius:8px 8px 0 0">${img(M.markSvg(16), 16)}<span>Ghost</span></div>`;
  const html =
    `<!doctype html><meta charset="utf-8"><body style="margin:0;padding:24px;background:#777;display:grid;gap:18px;font:12px sans-serif;color:#fff">` +
    [48, 64, 96, 128].map((px) => `<div>dock ${px}px</div>${dock(px, "rgba(20,20,24,.85)")}${dock(px, "rgba(240,240,244,.9)")}`).join("") +
    `<div>tray (dark and light panels)</div>${tray("#000", "#fff")}${tray("#f0f0f0", "#222")}<div>tab</div>${tab("#202124", "#eee")}${tab("#f1f3f4", "#222")}</body>`;
  mkdirSync(work, { recursive: true });
  const page = join(work, "dock.html");
  writeFileSync(page, html);
  const cdp = await launch();
  try {
    const p = await open(cdp, page, { width: 900, height: 1400 });
    const h = await p.evaluate("document.body.scrollHeight");
    writeFileSync(arg, await p.shot({ x: 0, y: 0, width: 900, height: h }));
    await p.close();
  } finally {
    await cdp.close();
  }
  console.log(`marks: ${arg}`);
} else {
  console.error("marks: desktop | all <dir> | dock <file.png>");
  process.exit(2);
}
