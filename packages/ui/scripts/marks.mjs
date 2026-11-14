#!/usr/bin/env node
/**
 * Writes Ghost's mark as files, from packages/ui/src/mark.ts (the one drawing; docs/design-system.md,
 * The mark). Node 22.15 or later (type stripping and module hooks), Chrome (the renderer the
 * apps draw the SVG with), ImageMagick (ICO and WebP).
 *
 *   node packages/ui/scripts/marks.mjs desktop      the desktop app's icons, tray icons and favicons (committed)
 *   node packages/ui/scripts/marks.mjs mobile       the phone's launcher icon, adaptive layers, splash, favicon and status-bar icon (committed)
 *   node packages/ui/scripts/marks.mjs launcher <file>  a review sheet: the adaptive icon through Android's masks, themed, the splash and the status bar
 *   node packages/ui/scripts/marks.mjs all <dir>    every variant as SVG and PNG into <dir>, for the phone and the site
 *   node packages/ui/scripts/marks.mjs site         the site's icons and social card, the README's header and the organisation's avatar (committed)
 *   node packages/ui/scripts/marks.mjs dock <file>  a review sheet: the icon at its real pixels on a dock, a tray and a tab
 *
 * `make marks` runs the first two. Everything is rendered at its own pixel size (no downscaling a
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
const MONO = join(REPO, "apps/desktop/node_modules/@fontsource-variable/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2");
const face = (family, file) => `@font-face{font-family:'${family}';font-weight:100 900;src:url(data:font/woff2;base64,${readFileSync(file).toString("base64")}) format('woff2')}`;
const fontFace = () => {
  try {
    return face("Inter Variable", INTER);
  } catch {
    return "";
  }
};
/** Both of the brand's faces, for a drawing with words in it that must not fall back (the card). */
const bothFaces = () => face("Inter Variable", INTER) + face("JetBrains Mono Variable", MONO);
/** A square drawing on the night, full bleed: where the host rounds the corners itself (iOS, GitHub). */
const fullBleed = (px, svg) => svg.replace(/(<svg[^>]*>)/, `$1<rect width="100" height="100" fill="${M.NIGHT}"/>`);

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
} else if (cmd === "mobile") {
  const assets = join(REPO, "apps/mobile/assets");
  const files = [
    // The legacy launcher icon (Android before 8) and Expo's default: the mark, full size.
    { name: "icon.png", px: 1024, svg: M.markSvg(1024) },
    { name: "android-icon-background.png", px: 1024, svg: M.adaptiveSvg("background", "idle", 1024) },
    { name: "android-icon-foreground.png", px: 1024, svg: M.adaptiveSvg("foreground", "idle", 1024) },
    { name: "android-icon-monochrome.png", px: 1024, svg: M.adaptiveSvg("monochrome", "idle", 1024) },
    // The splash: the lit figure inside the same safe circle (Android 12's splash masks its icon to a circle), on the night.
    { name: "splash-icon.png", px: 1024, svg: M.adaptiveSvg("foreground", "idle", 1024) },
    { name: "favicon.png", px: 48, svg: M.markSvg(48) },
  ];
  await rasterize(files, assets, work);
  const drawable = join(REPO, "apps/mobile/modules/ghost-call/android/src/main/res/drawable/ghost_link.xml");
  writeFileSync(drawable, M.notificationVector());
  console.log(`marks: ${files.length} files in ${assets}, and ${drawable}`);
} else if (cmd === "launcher") {
  if (!arg) throw new Error("marks: launcher <file.png>");
  const uri = (svg) => `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;
  const bg = uri(M.adaptiveSvg("background"));
  const fg = uri(M.adaptiveSvg("foreground"));
  const mono = uri(M.adaptiveSvg("monochrome"));
  // Android's masks over the 72dp the launcher shows of the 108dp layers; the 66dp circle is the only safe area.
  const squircle = (() => {
    const pts = [];
    for (let i = 0; i < 96; i++) {
      const t = (i / 96) * 2 * Math.PI;
      const c = Math.cos(t);
      const sn = Math.sin(t);
      pts.push(`${(54 + 36 * Math.sign(c) * Math.abs(c) ** 0.4).toFixed(2)} ${(54 + 36 * Math.sign(sn) * Math.abs(sn) ** 0.4).toFixed(2)}`);
    }
    return `M${pts.join("L")}Z`;
  })();
  const MASKS = {
    circle: '<circle cx="54" cy="54" r="36"/>',
    squircle: `<path d="${squircle}"/>`,
    "rounded square": '<rect x="18" y="18" width="72" height="72" rx="14"/>',
    teardrop: '<path d="M54 18A36 36 0 0 1 90 54V82A8 8 0 0 1 82 90H54A36 36 0 0 1 18 54A36 36 0 0 1 54 18Z"/>',
  };
  const icon = (mask, px, layers, i, safe = false) =>
    `<svg width="${px}" height="${px}" viewBox="18 18 72 72"><defs><clipPath id="m${i}">${MASKS[mask]}</clipPath></defs><g clip-path="url(#m${i})">${layers}</g>${safe ? '<circle cx="54" cy="54" r="33" fill="none" stroke="#ff4d5e" stroke-width="0.4" stroke-dasharray="1.5 1"/>' : ""}</svg>`;
  const full = `<image href="${bg}" width="108" height="108"/><image href="${fg}" width="108" height="108"/>`;
  // Material You's themed icon: the monochrome layer tinted, on a pale or deep wallpaper colour.
  const themed = (back, ink) => `<rect width="108" height="108" fill="${back}"/><image href="${mono}" width="108" height="108" style="filter:url(#tint-${ink.slice(1)})"/>`;
  const tintFilter = (ink) => {
    const n = parseInt(ink.slice(1), 16);
    const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => (v / 255).toFixed(3));
    return `<filter id="tint-${ink.slice(1)}"><feColorMatrix values="0 0 0 0 ${r}  0 0 0 0 ${g}  0 0 0 0 ${b}  0 0 0 1 0"/></filter>`;
  };
  let i = 0;
  const row = (title, cells) => `<div class="t">${title}</div><div class="r">${cells.join("")}</div>`;
  const cell = (inner, label) => `<div class="c">${inner}<span>${label}</span></div>`;
  const masks = Object.keys(MASKS);
  const html =
    `<!doctype html><meta charset="utf-8"><style>body{margin:0;padding:24px;background:#2b2d31;color:#eee;font:13px Inter,sans-serif}.t{margin:18px 0 8px;font-weight:600}.r{display:flex;gap:22px;align-items:flex-end;flex-wrap:wrap}.c{display:flex;flex-direction:column;align-items:center;gap:6px}.wall{display:flex;gap:18px;padding:18px;border-radius:16px;align-items:center}</style>` +
    `<svg width="0" height="0" style="position:absolute">${tintFilter("#0b1c37")}${tintFilter("#d6e3ff")}${tintFilter("#3a2a00")}</svg>` +
    row("The adaptive icon through each mask, 192 px, the 66dp safe circle dashed", masks.map((m) => cell(icon(m, 192, full, i++, true), m))) +
    row("At launcher size, 48 px (a home screen at 1x), on a dark and a light wallpaper", [
      `<div class="wall" style="background:linear-gradient(135deg,#1c2a3a,#3b2f4a)">${masks.map((m) => icon(m, 48, full, i++)).join("")}</div>`,
      `<div class="wall" style="background:linear-gradient(135deg,#e8eef6,#f6e9e3)">${masks.map((m) => icon(m, 48, full, i++)).join("")}</div>`,
    ]) +
    row("Themed icons (Android 13+): the monochrome layer in the wallpaper's colours, 192 and 48 px", [
      cell(icon("circle", 192, themed("#d6e3ff", "#0b1c37"), i++), "light"),
      cell(icon("circle", 192, themed("#0b1c37", "#d6e3ff"), i++), "dark"),
      cell(icon("squircle", 192, themed("#ffdf9e", "#3a2a00"), i++), "light, warm"),
      `<div class="wall" style="background:#eef2fa">${masks.map((m) => icon(m, 48, themed("#d6e3ff", "#0b1c37"), i++)).join("")}</div>`,
      `<div class="wall" style="background:#10141c">${masks.map((m) => icon(m, 48, themed("#0b1c37", "#d6e3ff"), i++)).join("")}</div>`,
    ]) +
    row("The splash (Android 12+: the icon in a 160dp circle, on the night), at 1x", [
      `<div style="width:206px;height:458px;background:${M.NIGHT};border-radius:18px;display:flex;align-items:center;justify-content:center"><img src="${uri(M.adaptiveSvg("foreground"))}" width="120" height="120" style="border-radius:50%"></div>`,
    ]) +
    row("The status bar's icon (24dp) at 1x, 2x and 3x, white as Android paints it, on dark and light shades", [
      `<div class="wall" style="background:#000">${[24, 48, 72].map((px) => `<img width="${px}" height="${px}" src="${uri(M.notificationSvg())}">`).join("")}</div>`,
      `<div class="wall" style="background:#5f6368">${[24, 48, 72].map((px) => `<img width="${px}" height="${px}" src="${uri(M.notificationSvg())}">`).join("")}</div>`,
    ]) +
    `</body>`;
  mkdirSync(work, { recursive: true });
  const page = join(work, "launcher.html");
  writeFileSync(page, html);
  const cdp = await launch();
  try {
    const p = await open(cdp, page, { width: 1100, height: 1600, scale: 2 });
    const h = await p.evaluate("document.body.scrollHeight");
    writeFileSync(arg, await p.shot({ x: 0, y: 0, width: 1100, height: h }));
    await p.close();
  } finally {
    await cdp.close();
  }
  console.log(`marks: ${arg}`);
} else if (cmd === "site") {
  // The site (site/public: the favicon's .ico for browsers that don't take the SVG, the home-screen
  // icon, the social card), the README's header for GitHub's light and dark themes (docs/brand), and
  // the organisation's avatar, uploaded by hand (docs/brand/avatar-512.png).
  const pub = join(REPO, "site/public");
  const brand = join(REPO, "docs/brand");
  const fonts = bothFaces();
  const files = [
    ...[16, 32, 48].map((px) => ({ name: `ico/${px}.png`, px, svg: M.markSvg(px, { id: `i${px}` }) })),
    { name: "apple-touch-icon.png", px: 180, svg: fullBleed(180, M.markSvg(180)) },
    { name: "card.png", px: 1200, w: 1200, h: 630, svg: M.reachCardSvg({ fontFace: fonts }) },
    { name: "avatar-512.png", px: 512, svg: fullBleed(512, M.markSvg(512)) },
  ];
  for (const theme of ["dark", "light"]) {
    // Shown 56 px tall; drawn at twice that, so it is sharp on a 2x screen and the ladder is the 112 px one.
    const svg = M.wordmarkSvg(112, theme, { fontFace: fonts, id: `w${theme[0]}` });
    files.push({ name: `lockup-${theme}.png`, px: 112, w: Number(/width="(\d+)"/.exec(svg)[1]), h: 112, svg });
  }
  await rasterize(files, join(work, "site"), join(work, "site-work"));
  const at = (name) => join(work, "site", name);
  mkdirSync(brand, { recursive: true });
  await magick([...[16, 32, 48].map((px) => at(`ico/${px}.png`)), join(pub, "favicon.ico")]);
  await magick([at("apple-touch-icon.png"), "-strip", join(pub, "apple-touch-icon.png")]);
  await magick([at("card.png"), "-strip", "-quality", "88", "-sampling-factor", "4:4:4", join(pub, "og.jpg")]);
  for (const name of ["avatar-512.png", "lockup-dark.png", "lockup-light.png"]) await magick([at(name), "-strip", join(brand, name)]);
  rmSync(join(work, "site"), { recursive: true, force: true });
  console.log(`marks: favicon.ico, apple-touch-icon.png and og.jpg in ${pub}; avatar-512.png and lockup-{dark,light}.png in ${brand}`);
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
  console.error("marks: desktop | mobile | site | all <dir> | dock <file.png> | launcher <file.png>");
  process.exit(2);
}
