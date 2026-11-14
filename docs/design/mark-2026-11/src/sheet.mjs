/**
 * Stage two: the concept sheet as one static page (sheet/index.html), every mark from png/ at
 * its own pixel size (pixelated, so a 2x render shows the real pixels) and from svg/ where the
 * size is large. Every context is drawn here in CSS and SVG: no screenshots of anyone's
 * desktop, no stock art, the design system's two fonts only.
 *
 *   node docs/design/mark-2026-11/src/sheet.mjs
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { STATES, TODAY, STILL, STILL_SMALL, body, figure } from "./marks.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = join(ROOT, "..", "..", "..");
mkdirSync(join(ROOT, "sheet"), { recursive: true });

// ---------------------------------------------------------------- the agents' shapes, read from the source

const shapesSrc = readFileSync(join(REPO, "packages/ui/src/agentShapes.ts"), "utf8");
const SHAPES = [...shapesSrc.matchAll(/^\s+(\w+): \{ body: "([^"]+)", eyes: EYES\(([^)]*)\) \},?$/gm)].map(([, name, d, args]) => {
  const [y, gap = 2.6, rx = 1.35, ry = 1.95] = args.split(",").map(Number);
  return { name, d, eyes: [[12 - gap, y, rx, ry], [12 + gap, y, rx, ry]] };
});
const HUES = ["#9db0ff", "#c4a8ff", "#ff9ccf", "#ffad7a", "#f5cc5c", "#6ee39a", "#5fd4e6", "#7cc9ff"];

// ---------------------------------------------------------------- helpers

const COLS = [
  { key: "today", label: "Today", note: "indigo tile, smile, stick arms" },
  { key: "lit", label: "I · Lit", note: "Wisp as the light source" },
  { key: "instrument", label: "II · Instrument", note: "lit Wisp in the ring of ticks", rec: true },
  { key: "eyes", label: "III · Eyes in the dark", note: "the body is the night; the eyes are the light" },
];
const png = (name) => `../png/${name}.png`;
const svgf = (name) => `../svg/${name}.svg`;
const img = (src, px, cls = "", alt = "") => `<img class="px ${cls}" src="${src}" width="${px}" height="${px}" alt="${alt}">`;
/** The app icon of a column at a size. */
const app = (k, px) => img(png(k === "today" ? `today-${px}` : `${k}-${px}`), px);
const appState = (k, st, px) => img(png(k === "today" ? `today-${px}` : `${k}-${st}-${px}`), px);
/** A mark shown at a display size from its SVG master (large sizes only). */
const big = (k, master, show) => (k === "today" ? `<img src="${png(`today-${master}`)}" width="${show}" height="${show}" alt="">` : `<img src="${svgf(`${k}-${master}`)}" width="${show}" height="${show}" alt="">`);
const mono = (k, px, light) => (k === "today" ? todayMono(px, light) : img(png(`${k}-${light ? "monolight" : "mono"}-${px}`), px));
/** Today has no one-colour mark of its own: the phone's themed layer is the closest, a white ghost with arms. */
const todayMono = (px, light) => `<span class="tmono" style="width:${px}px;height:${px}px;-webkit-mask-image:url(${png(`today-bare-${px <= 16 ? 16 : px <= 20 ? 20 : px <= 24 ? 24 : px <= 32 ? 32 : 48}`)});background:${light ? "#eafcff" : "#0b0d12"}"></span>`;

const tag = (k) => {
  const c = COLS.find((c) => c.key === k);
  return `<div class="ctag ${c.rec ? "rec" : ""}">${c.label}${c.rec ? ' <b>recommended</b>' : ""}</div>`;
};

/** A row of the four columns, each cell drawn by `cell(k)`. */
const row = (title, sub, cell, cls = "") => `
<div class="ctx ${cls}">
  <div class="ctx__head"><h3>${title}</h3><p>${sub}</p></div>
  <div class="ctx__grid">${COLS.map((c) => `<div class="ctx__cell">${tag(c.key)}${cell(c.key)}</div>`).join("")}</div>
</div>`;

// Generic neighbours for docks, tabs and trays: plain shapes, no one's brand.
const GEN = {
  files: `<svg viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#e8a33d"/><path d="M10 16h11l3 3h14v17H10z" fill="#fff4dc"/></svg>`,
  term: `<svg viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#2b2f37"/><path d="M12 18l7 6-7 6" stroke="#e6e9ef" stroke-width="3" fill="none" stroke-linecap="round" stroke-linejoin="round"/><path d="M23 31h12" stroke="#e6e9ef" stroke-width="3" stroke-linecap="round"/></svg>`,
  web: `<svg viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#2f6fe4"/><circle cx="24" cy="24" r="12" fill="none" stroke="#fff" stroke-width="2.6"/><path d="M12 24h24M24 12c-5 6-5 18 0 24M24 12c5 6 5 18 0 24" stroke="#fff" stroke-width="2.2" fill="none"/></svg>`,
  edit: `<svg viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#7a4fe0"/><path d="M19 15l-7 9 7 9M29 15l7 9-7 9" stroke="#fff" stroke-width="3" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  mail: `<svg viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#e4e7ec"/><rect x="11" y="15" width="26" height="18" rx="2" fill="none" stroke="#4b5563" stroke-width="2.6"/><path d="M11 17l13 9 13-9" stroke="#4b5563" stroke-width="2.6" fill="none"/></svg>`,
  chat: `<svg viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#1f9d6b"/><path d="M13 15h22v14H22l-6 5v-5h-3z" fill="#fff"/></svg>`,
  music: `<svg viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#e2445c"/><path d="M20 31V15l14-3v16" stroke="#fff" stroke-width="2.6" fill="none"/><circle cx="17" cy="31" r="4" fill="#fff"/><circle cx="31" cy="28" r="4" fill="#fff"/></svg>`,
};
const sym = {
  wifi: (c) => `<svg width="16" height="16" viewBox="0 0 16 16"><path d="M1.5 6.2a9.5 9.5 0 0 1 13 0M3.8 8.6a6.2 6.2 0 0 1 8.4 0M6.1 11a3 3 0 0 1 3.8 0" stroke="${c}" stroke-width="1.5" fill="none" stroke-linecap="round"/><circle cx="8" cy="13.3" r="1.1" fill="${c}"/></svg>`,
  vol: (c) => `<svg width="16" height="16" viewBox="0 0 16 16"><path d="M2 6h3l4-3v10L5 10H2z" fill="${c}"/><path d="M11 5.5a3.5 3.5 0 0 1 0 5M12.6 3.6a6 6 0 0 1 0 8.8" stroke="${c}" stroke-width="1.4" fill="none" stroke-linecap="round"/></svg>`,
  bat: (c) => `<svg width="16" height="16" viewBox="0 0 16 16"><rect x="1.5" y="4.5" width="11.5" height="7" rx="1.5" stroke="${c}" stroke-width="1.3" fill="none"/><rect x="3" y="6" width="6.5" height="4" fill="${c}"/><rect x="13.6" y="6.6" width="1.4" height="2.8" rx=".5" fill="${c}"/></svg>`,
  cloud: (c) => `<svg width="16" height="16" viewBox="0 0 16 16"><path d="M4.5 12.5a3 3 0 0 1-.3-6 4 4 0 0 1 7.6-.6 3.3 3.3 0 0 1 .2 6.6z" fill="none" stroke="${c}" stroke-width="1.4"/></svg>`,
  mic: (c) => `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="${c}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="2" width="6" height="12" rx="3"/><path d="M19 10v1a7 7 0 0 1-14 0v-1M12 18v4"/></svg>`,
  micoff: (c) => `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="${c}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 2l20 20M18.9 13.5A7 7 0 0 0 19 12v-2M5 10v2a7 7 0 0 0 12 5M15 9.3V5a3 3 0 0 0-5.7-1.3M9 9v3a3 3 0 0 0 5.1 2.1M12 19v3"/></svg>`,
};

// ---------------------------------------------------------------- contexts

/** GNOME's dash over a wallpaper. */
const dock = (k, wall) => `
<div class="wall ${wall}"><div class="dock">${GEN.files}${GEN.web}<span class="dock__me">${app(k, 48)}<i></i></span>${GEN.term}${GEN.edit}</div></div>`;

/** GNOME's top bar with the tray, dark and light panel. */
const trayIcons = (k, ink, light) => {
  if (k === "today") return `<span class="tray__me">${app("today", 16)}</span>`;
  // Recommended rule: symbolic (one colour, the panel's ink) at rest; the lit glyph in the state's colour during a session.
  return `<span class="tray__me">${mono(k, 16, !light)}</span><span class="tray__me">${img(png(light ? `${k}-listening-16` : `bare-listening-16`), 16)}</span>`;
};
const tray = (k) => `
<div class="bars">
  <div class="bar dark"><span class="bar__l">Thu 14:32</span><span class="bar__r">${trayIcons(k, "#eef0f3", false)}${sym.wifi("#eef0f3")}${sym.vol("#eef0f3")}${sym.bat("#eef0f3")}</span></div>
  <div class="bar light"><span class="bar__l">Thu 14:32</span><span class="bar__r">${trayIcons(k, "#1d2027", true)}${sym.wifi("#1d2027")}${sym.vol("#1d2027")}${sym.bat("#1d2027")}</span></div>
  ${k === "today" ? "" : `<p class="mini">at rest, one colour · in a session, lit in the state's colour (green: listening; on a light panel, the chip)</p>`}
</div>`;

/** The desktop app's rail: the brand at its head, the microphone switch at its foot. */
const rail = (k, theme) => {
  const brand =
    k === "today"
      ? `<span class="rail__ring">${img(png("today-bare-36"), 36)}</span>`
      : `<span class="rail__chip">${img(png(`${k}-listening-36`), 36)}</span>`;
  const off = k === "today" ? `<span class="rail__ring off">${img(png("today-bare-36"), 36, "dim")}</span>` : `<span class="rail__chip">${img(png(`${k}-off-36`), 36)}</span>`;
  return `
<div class="railbox ${theme}">
  <div class="rail"><div class="rail__brand">${brand}<span>Listening</span></div><div class="rail__nav"><i></i><i></i><i></i></div><div class="rail__mic">${sym.mic(theme === "light" ? "#3b4150" : "#c9ced8")}</div></div>
  <div class="rail"><div class="rail__brand">${off}<span>Not listening</span></div><div class="rail__nav"><i></i><i></i><i></i></div><div class="rail__mic off">${sym.micoff("#ff4d5e")}</div></div>
</div>`;
};

/** A browser's tab strip, light and dark. */
const tabs = (k) => {
  const fav = k === "today" ? img(png("today-favicon-16"), 16) : img(png(`${k}-16`), 16);
  const t = (icon, name, on) => `<span class="tab ${on ? "on" : ""}">${icon}<span>${name}</span></span>`;
  const g = (c) => `<span class="gfav" style="background:${c}"></span>`;
  const strip = (theme) => `<div class="tabs ${theme}">${t(g("#2f6fe4"), "Pull requests", false)}${t(fav, "Ghost — your desk", true)}${t(g("#e2445c"), "Inbox (3)", false)}${t(g("#1f9d6b"), "Docs", false)}</div>`;
  return strip("light") + strip("dark");
};

/** An Android home screen: the adaptive icon through four masks, and the themed icon. */
const MASKS = { circle: "50%", squircle: "36%", rounded: "18%", teardrop: "50% 50% 18% 50%" };
const adaptiveIcon = (k, mask, size = 56) => {
  const fg = k === "today" ? png("today-adaptive-fg") : png(`${k}-adaptive-fg`);
  const bg = k === "today" ? png("today-adaptive-bg") : png(`${k}-adaptive-bg`);
  const layer = size * 1.5; // the 108 canvas, of which the mask shows the middle 72
  const off = -(layer - size) / 2;
  const bgcol = k === "today" ? "#5560E8" : "#05060a";
  return `<span class="ad" style="width:${size}px;height:${size}px;border-radius:${MASKS[mask]};background:${bgcol}"><img src="${bg}" style="width:${layer}px;height:${layer}px;left:${off}px;top:${off}px"><img src="${fg}" style="width:${layer}px;height:${layer}px;left:${off}px;top:${off}px"></span>`;
};
const themed = (k, size = 56) => {
  const m = k === "today" ? png("today-adaptive-mono") : png(`${k}-adaptive-mono`);
  const layer = size * 1.5;
  return `<span class="ad themed" style="width:${size}px;height:${size}px;border-radius:50%"><span style="width:${layer}px;height:${layer}px;left:${-(layer - size) / 2}px;top:${-(layer - size) / 2}px;-webkit-mask-image:url(${m})"></span></span>`;
};
const android = (k) => `
<div class="phone">
  <div class="phone__grid">
    ${Object.keys(MASKS).map((m) => `<span class="phone__app">${adaptiveIcon(k, m)}<span>${m}</span></span>`).join("")}
  </div>
  <div class="phone__grid themedrow">
    <span class="phone__app">${themed(k)}<span>themed</span></span>
    <span class="phone__app"><span class="ad themed gen"><span class="genmono"></span></span><span>Clock</span></span>
    <span class="phone__app"><span class="ad themed gen"><span class="genmono b"></span></span><span>Files</span></span>
    <span class="phone__app"><span class="ad themed gen"><span class="genmono c"></span></span><span>Maps</span></span>
  </div>
</div>`;

/** An Android notification: the status bar's small icon (one colour) and the card. */
const notification = (k) => {
  const small = k === "today" ? todayMono(16, true) : mono(k, 16, true);
  const smallCard = k === "today" ? todayMono(16, true) : img(png(`bare-waiting-16`), 16);
  const large = k === "today" ? app("today", 40) : app(k, 40);
  return `
<div class="noti">
  <div class="noti__bar"><span>9:41</span><span class="noti__icons">${small}${sym.wifi("#e8eaed")}${sym.bat("#e8eaed")}</span></div>
  <div class="noti__card">
    <div class="noti__app">${smallCard}<span>Ghost · now</span></div>
    <div class="noti__body"><div><b>Waiting for your yes</b><p>Close issue #214 in heka-vault/site?</p></div>${large}</div>
  </div>
</div>`;
};

/** GitHub: the organisation's avatar on its page and small in a list. */
const github = (k) => {
  const av = k === "today" ? png("today-128") : png(`${k}-128`);
  const s20 = k === "today" ? png("today-20") : png(`${k}-20`);
  return `
<div class="gh">
  <div class="gh__org"><img src="${av}" width="80" height="80" alt=""><div><b>Ghost</b><p>A voice on your own machine</p></div></div>
  <div class="gh__list"><span><img class="px" src="${s20}" width="20" height="20" alt=""> ghost / ghost</span><span class="gh__gen"></span><span class="gh__gen b"></span></div>
</div>`;
};

/** The README's header, on GitHub's light and dark. */
const lockup = (k, theme, px = 40) => {
  if (k === "today") return `<span class="lock ${theme}"><svg width="${px * 0.8}" height="${px * 0.8}" viewBox="0 0 24 24"><path fill="currentColor" fill-rule="evenodd" d="${figure(TODAY).d}"/></svg><span class="lock__w today">Ghost</span></span>`;
  return `<span class="lock ${theme}">${img(png(`${k}-${px}`), px)}<span class="lock__w">Ghost</span></span>`;
};
const readme = (k) => `
<div class="rm light">${lockup(k, "light")}<p>Ghost floats over your work when you call it.</p></div>
<div class="rm dark">${lockup(k, "dark")}<p>Ghost floats over your work when you call it.</p></div>`;

/** The site's nav. */
const nav = (k) => `
<div class="nav">${k === "today" ? `<span class="nav__brand"><svg width="22" height="22" viewBox="0 0 24 24"><path fill="currentColor" fill-rule="evenodd" d="${figure(TODAY).d}"/></svg>Ghost</span>` : `<span class="nav__brand">${img(png(`${k}-24`), 24)}Ghost</span>`}<span class="nav__links"><span>Docs</span><span class="nav__cta">Download</span></span></div>`;

// ---------------------------------------------------------------- the reach (C): big surfaces only

const PLACES = [
  { name: "GitHub", word: "pull requests", a: -150, r: 1.0 },
  { name: "Linear", word: "issues", a: -112, r: 0.92 },
  { name: "Calendar", word: "today", a: -40, r: 0.96 },
  { name: "Agents", word: "Ledger", a: 14, r: 0.9 },
  { name: "Memory", word: "", a: 150, r: 0.9 },
];
/** Beams from a centre to named places, in the floating room's language: lit lines, a node of light, the name in heavy type. */
function beams(cx, cy, R, tint, places = PLACES, scale = 1) {
  let out = `<defs><filter id="bm" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="${3 * scale}"/></filter><radialGradient id="nd"><stop offset="0" stop-color="#fff"/><stop offset=".3" stop-color="${tint}"/><stop offset="1" stop-color="${tint}" stop-opacity="0"/></radialGradient></defs>`;
  for (const p of places) {
    const a = (p.a * Math.PI) / 180;
    const x = cx + Math.cos(a) * R * p.r;
    const y = cy + Math.sin(a) * R * p.r;
    const x0 = cx + Math.cos(a) * 70 * scale;
    const y0 = cy + Math.sin(a) * 70 * scale;
    out += `<line x1="${x0}" y1="${y0}" x2="${x}" y2="${y}" stroke="${tint}" stroke-width="${5 * scale}" opacity=".35" filter="url(#bm)"/><line x1="${x0}" y1="${y0}" x2="${x}" y2="${y}" stroke="${tint}" stroke-width="${1.2 * scale}" opacity=".85"/>`;
    const mx = x0 + (x - x0) * 0.55;
    const my = y0 + (y - y0) * 0.55;
    out += `<circle cx="${mx}" cy="${my}" r="${7 * scale}" fill="url(#nd)"/><circle cx="${x}" cy="${y}" r="${11 * scale}" fill="url(#nd)"/>`;
    const below = Math.sin(a) > 0.3;
    const ty = y + (below ? 30 : -22) * scale;
    out += `<text x="${x}" y="${ty}" text-anchor="middle" font-family="Inter Variable" font-weight="800" font-size="${19 * scale}" fill="#eafcff" style="paint-order:stroke" stroke="#05060a" stroke-width="${4 * scale}">${p.name}</text>`;
    if (p.word) out += `<text x="${x}" y="${ty + 16 * scale}" text-anchor="middle" font-family="JetBrains Mono Variable" font-weight="600" font-size="${10 * scale}" letter-spacing="${2 * scale}" fill="${tint}">${p.word.toUpperCase()}</text>`;
  }
  return out;
}

const social = (k) => {
  if (k === "today") return `<img src="../../../../site/public/og.jpg" width="600" height="315" alt="">`;
  return `
<div class="og">
  <svg class="og__field" viewBox="0 0 1200 630" width="600" height="315">
    <defs><radialGradient id="ogl" cx="840" cy="300" r="520" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#a98bff" stop-opacity=".28"/><stop offset=".45" stop-color="#a98bff" stop-opacity=".06"/><stop offset="1" stop-color="#a98bff" stop-opacity="0"/></radialGradient></defs>
    <rect width="1200" height="630" fill="#05060a"/><rect width="1200" height="630" fill="url(#ogl)"/>
    ${beams(840, 300, 280, "#a98bff", PLACES, 1.4)}
    <image href="${svgf(`${k}-adaptive-fg-reaching`)}" x="660" y="120" width="360" height="360"/>
    <text x="80" y="250" font-family="Inter Variable" font-weight="800" font-size="92" letter-spacing="-3" fill="#eafcff">Ghost</text>
    <text x="84" y="310" font-family="Inter Variable" font-weight="500" font-size="28" fill="#eafcff" opacity=".72">Floats over your work</text>
    <text x="84" y="346" font-family="Inter Variable" font-weight="500" font-size="28" fill="#eafcff" opacity=".72">when you call it.</text>
    <text x="84" y="430" font-family="JetBrains Mono Variable" font-weight="600" font-size="16" letter-spacing="5" fill="#a98bff">SAY “GHOST”</text>
  </svg>
</div>`;
};

const splash = (k) => {
  if (k === "today") return `<div class="splash today"><img src="../../../../apps/mobile/assets/splash-icon.png" width="150" height="150" alt=""></div>`;
  return `<div class="splash"><img src="${svgf(`${k}-adaptive-fg`)}" width="180" height="180" alt="" style="margin:-30px 0"><span class="splash__w">Ghost</span><span class="splash__s">SAY “GHOST”</span></div>`;
};

// ---------------------------------------------------------------- the sections

const ladder = (k) => `
<div class="ladder">
  <figure>${big(k, 1024, 256)}<figcaption>1024 <i>shown at 256</i></figcaption></figure>
  <figure>${big(k, 512, 160)}<figcaption>512 <i>at 160</i></figcaption></figure>
  <figure>${app(k, 128)}<figcaption>128</figcaption></figure>
  <figure>${app(k, 48)}<figcaption>48</figcaption></figure>
  <figure>${app(k, 32)}<figcaption>32</figcaption></figure>
  <figure>${app(k, 24)}<figcaption>24</figcaption></figure>
  <figure>${app(k, 16)}<figcaption>16</figcaption></figure>
</div>
<div class="zoom">
  ${[32, 24, 16].map((px) => `<figure><img class="px" src="${png(k === "today" ? `today-${px}` : `${k}-${px}`)}" width="${px * (px === 16 ? 8 : px === 24 ? 16 / 3 * 2 : 4)}" height="${px * (px === 16 ? 8 : px === 24 ? 16 / 3 * 2 : 4)}" alt=""><figcaption>${px} px, every pixel</figcaption></figure>`).join("")}
  <figure class="lightbg"><div class="ondark-light"><span>${app(k, 16)}${app(k, 24)}${app(k, 32)}</span><span class="wl">${app(k, 16)}${app(k, 24)}${app(k, 32)}</span><span class="wb">${app(k, 16)}${app(k, 24)}${app(k, 32)}</span></div><figcaption>16 · 24 · 32 on dark, light, busy</figcaption></figure>
</div>`;

const statesRow = (k) => `
<div class="states">
  ${Object.entries(STATES)
    .map(([st, s]) => `<figure>${img(png(`${k}-${st}-128`), 128)}<span class="states__small">${img(png(`${k}-${st}-48`), 48)}${img(png(`${k}-${st}-32`), 32)}${img(png(`${k}-${st}-16`), 16)}</span><figcaption><i style="background:${s.tint}"></i>${s.name}</figcaption></figure>`)
    .join("")}
</div>`;

const monoRow = (k) => `
<div class="monos">
  <div class="monos__light">${[128, 48, 32, 24, 16].map((px) => mono(k, px, false)).join("")}</div>
  <div class="monos__dark">${[128, 48, 32, 24, 16].map((px) => mono(k, px, true)).join("")}</div>
</div>`;

const DIRS = [
  {
    k: "lit",
    title: "I · Lit",
    idea: "Wisp is the light source. The indigo slab becomes a night tile; the state's colour is the light behind Wisp and the edge of the tile; a hairline streak runs through the eyes at 128 and up, the floating room's lens. No smile, no arms.",
    holds: "The simplest of the three, and the best at 16: a white figure on black is the strongest contrast a tile can have.",
    fails: "On its own it is a very good app icon and not yet a system: nothing in it says instrument, or reach. Bloom carries the look; under 48 there is no bloom and it is just a nice ghost on black.",
    score: "7",
  },
  {
    k: "instrument",
    title: "II · Instrument",
    idea: "Lit Wisp inside the floating room's ring of ticks: the ring is the device, an arc of it lit in the state's colour is what Ghost is doing (where it is listening, where it reaches, all of it while it speaks, the bottom for a yes). Under 32 the ring can't be drawn honestly, so the mark becomes I's small drawing: one system, two levels of detail.",
    holds: "It is the floating room, made still. It reads as an instrument, not a sticker, and it carries state without a second icon. The ring fits Android's circle mask exactly.",
    fails: "At 32 the ring is a hairline and an arc, close to its limit. It needs the night tile to read; on a light page it is a dark chip, never a bare glyph.",
    score: "8.5",
  },
  {
    k: "eyes",
    title: "III · Eyes in the dark",
    idea: "The body is the night, found only by a rim of light from above; the eyes are the light. The most cinematic, the most Blade Runner.",
    holds: "Unmistakable at 16: two lit pairs of pixels in a black square. The monochrome is a clean outline.",
    fails: "It stops being Wisp: it watches you instead of being glad to see you, and next to the agents' family (all solid figures) it is the odd one out. The rim vanishes on a busy wallpaper. Good for a poster, wrong for the thing you talk to all day.",
    score: "6.5",
  },
];

const dirSection = (d) => `
<section class="dir" id="dir-${d.k}" data-name="dir-${d.k}">
  <header class="dir__head"><h2>${d.title}${d.k === "instrument" ? ' <b class="rectag">recommended</b>' : ""}</h2><p>${d.idea}</p><div class="dir__verdict"><p><b>Holds:</b> ${d.holds}</p><p><b>Costs:</b> ${d.fails}</p><p class="score">${d.score}<small>/10</small></p></div></header>
  ${ladder(d.k)}
  <h4>States <small>${d.k === "instrument" ? "the arc says what; the light says how" : "the light and the rim carry the state; Not listening keeps the tray's red stroke (ADR 0057)"}</small></h4>
  ${statesRow(d.k)}
  <h4>One colour <small>tray, themed icons, print${d.k === "instrument" ? "; the ring from 48, the lit arc as longer ticks" : ""}</small></h4>
  ${monoRow(d.k)}
</section>`;

// The character study.
const charSvg = (p, fill, w = 200) => {
  const f = figure(p);
  return `<svg width="${w}" height="${w}" viewBox="1 0.5 22 24"><g transform="${f.pose}"><path fill="${fill}" fill-rule="evenodd" d="${f.d}"/></g></svg>`;
};
const family = () => {
  const f = figure(STILL);
  const glow = (c) => `<filter id="fg${c.slice(1)}" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="0.7"/></filter>`;
  const one = (name, inner, c) => `<figure><svg width="72" height="72" viewBox="0 0 24 24"><defs>${glow(c)}</defs>${inner(c)}</svg><figcaption>${name}</figcaption></figure>`;
  const wisp = (c) => `<g transform="${f.pose}"><path d="${f.body}" fill="${c}" opacity=".7" filter="url(#fg${c.slice(1)})"/><path d="${f.body}" fill="#f4feff"/>${f.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${y}" rx="${rx}" ry="${ry}" fill="#06121a"/>`).join("")}</g>`;
  const shape = (s) => (c) => `<path d="${s.d}" fill="${c}" opacity=".7" filter="url(#fg${c.slice(1)})"/><path d="${s.d}" fill="#f4feff"/>${s.eyes.map(([x, y, rx, ry]) => `<ellipse cx="${x}" cy="${y}" rx="${rx}" ry="${ry}" fill="#06121a"/>`).join("")}`;
  return `<div class="family">${one("Wisp", wisp, "#5fe3ff")}${SHAPES.map((s, i) => one(s.name[0].toUpperCase() + s.name.slice(1), shape(s), HUES[(i + 1) % 8])).join("")}</div>`;
};

const character = `
<section class="char" data-name="character">
  <h2>The character</h2>
  <p class="lede">The floating Wisp has presence for three reasons the icon throws away: it is <b>lit</b>, it is <b>caught mid-drift</b> (tilted, tail flicked, never standing to attention) and it <b>looks at you</b>. The smile and the stick arms are what make today's mark a sticker: they are gestures, and a gesture frozen in an icon is clip art. The redraw keeps Wisp's own curve from <code>wisp.ts</code> and changes only the pose.</p>
  <div class="char__row">
    <figure><img src="../png/today-256.png" width="200" height="200" alt=""><figcaption>Today's app icon (waving, smiling)</figcaption></figure>
    <figure class="ink">${charSvg(TODAY, "#eafcff")}<figcaption>Today's brand glyph (GHOST_PATH): upright, at attention</figcaption></figure>
    <figure class="ink">${charSvg(STILL, "#eafcff")}<figcaption><b>The still pose</b>: tail swung 0.6 (the mascot's own <code>sw</code>), leaning −7° about (12, 13), eyes 6 % narrower, 24 % taller, 0.55 higher: looking up at you</figcaption></figure>
    <figure class="ink"><img src="../svg/character-overlay.svg" width="200" height="200" alt=""><figcaption>Over today's (dashed): same body, same eyes' spacing, a new pose</figcaption></figure>
    <figure class="ink">${charSvg(STILL_SMALL, "#eafcff")}<figcaption>Under 32 px: −4°, swing 0.45, eyes snapped to whole pixels (2×3 at 16, 2×4 at 24), both on one row</figcaption></figure>
  </div>
  <div class="char__notes">
    <p><b>Arms:</b> out of every still mark. The live mascot keeps them (its wave on hover is the best thing it does), so they become a reward for reaching for Ghost instead of a permanent pose. Sculpted into the silhouette they would be two bumps at 32 px and a character the agents' family doesn't share; I didn't draw that one past a sketch.</p>
    <p><b>Mouth:</b> out of the still mark. Wisp speaks with light and the ring in the room; a drawn smile at 16 px is two grey pixels.</p>
    <p><b>Tilt:</b> −7° and not more: past −10° the tail reads as a comma. The lean goes against the tail's curl, so the figure drifts up and left; at rest the ring's lit arc sits on that side, ahead of it.</p>
    <p><b>The family still holds</b>: the agents' shapes (from <code>agentShapes.ts</code>, unchanged) lit the same way sit beside it with no redraw needed. Their eyes are Wisp's two upright ovals; the redraw only made Wisp's taller, which the family can follow later or not at all.</p>
  </div>
  ${family()}
</section>`;

const contexts = `
<section class="contexts" data-name="contexts">
  <h2>In context <small>every mark at the size the surface really uses; drawn here, nobody's desktop</small></h2>
  ${row("GNOME dock", "48 px on a busy wallpaper and a light one", (k) => dock(k, "busy") + dock(k, "pale"))}
  ${row("Top bar tray", "16 px, dark and light panel", tray)}
  ${row("The desktop app's rail", "the brand glyph at 36, listening and not listening (ADR 0057), dark and light theme", (k) => `<div class="rails">${rail(k, "dark")}${rail(k, "light")}</div>`)}
  ${row("Browser tab", "the favicon at 16 among other tabs", tabs)}
  ${row("Android home screen", "the adaptive icon through four launcher masks; the themed (Material You) icon below", android)}
  ${row("Android notification", "the status bar's one-colour icon, and the card's", notification)}
  ${row("GitHub organisation", "the avatar at 80 and at 20 in a list", github)}
  ${row("README header", "GitHub light and dark", readme)}
  ${row("The site's nav", "heyghost.app", nav)}
</section>`;

const big2 = `
<section class="bigs" data-name="big">
  <h2>Big surfaces: the reach <small>Direction C belongs here, where there is room for the beams</small></h2>
  <p class="lede">Beams to named places are the one thing only Ghost does, and they are the floating room's centrepiece. At 16 to 128 px they are noise; on a social card, a splash, the site's hero and a lockup they are the story. So the reach is not a mark: it is what the mark does on a big surface.</p>
  <div class="bigs__row">
    <figure>${social("today")}<figcaption>Today's social card (site/public/og.jpg): a screenshot; no mark at all</figcaption></figure>
    <figure>${social("instrument")}<figcaption>The social card, recommended: the instrument reaching (violet), the name, the one instruction</figcaption></figure>
  </div>
  <div class="bigs__row">
    <figure>${splash("today")}<figcaption>Today: splash-icon.png (in assets; app.json configures no splash)</figcaption></figure>
    <figure>${splash("instrument")}<figcaption>Splash and the first run's welcome: the mark, the name, “say Ghost”</figcaption></figure>
    <figure>${splash("lit")}<figcaption>(I, for comparison)</figcaption></figure>
  </div>
</section>`;

const word = `
<section class="word" data-name="wordmark">
  <h2>Wordmark and lockup</h2>
  <p class="lede">Inter 800 at −0.035 em, titlecase, the weight the floating room already uses for every name it shows. Nothing drawn: the mark does the drawing, the word is plain and heavy. The room's second voice, JetBrains Mono 600 at +0.32 em in capitals, is the tagline and the state, never the name. Wide-tracked capitals for the name (GHOST, the Akira title) was the tempting one: it turns the product into a film poster and fights the mark for attention.</p>
  <div class="word__grid">
    <div class="word__cell dark"><span class="lockup">${img(png("instrument-128"), 96)}<span class="lockup__w">Ghost</span></span></div>
    <div class="word__cell light"><span class="lockup">${img(png("instrument-128"), 96)}<span class="lockup__w">Ghost</span></span></div>
    <div class="word__cell dark"><span class="lockup stack">${img(png("instrument-128"), 128)}<span class="lockup__w">Ghost</span><span class="lockup__s">SAY “GHOST”</span></span></div>
    <div class="word__cell light"><span class="lockup mono">${mono("instrument", 128, false)}<span class="lockup__w">Ghost</span></span></div>
    <div class="word__cell dark rejected"><span class="lockup">${img(png("instrument-128"), 96)}<span class="lockup__w caps">GHOST</span></span><em>not this: a poster, not a product</em></div>
    <div class="word__cell dark small"><span class="lockup">${img(png("instrument-32"), 32)}<span class="lockup__w s">Ghost</span></span><span class="lockup">${img(png("lit-24"), 24)}<span class="lockup__w xs">Ghost</span></span><em>the nav and the README: the mark is the 32 / 24 drawing</em></div>
  </div>
</section>`;

const ROLL = [
  ["Desktop window and dock", "apps/desktop/src-tauri/icons/{32x32,128x128,128x128@2x,icon}.png", "II at each size (32 is the hairline-and-arc drawing)"],
  ["The .deb's desktop entry", "apps/desktop/src-tauri/ghost.desktop (Icon={{icon}}, from the bundle's icons)", "follows the Tauri icons; add 48 and 256 so GNOME doesn't scale 128"],
  ["Tray", "apps/desktop/src-tauri/src/tray.rs (default window icon; struck() greys it)", "one-colour Wisp at rest, lit by state in a session; struck() keeps working on the one-colour drawing"],
  ["The rail's brand glyph", "apps/desktop/src/app/Rail.tsx, src/ui/Brand.tsx (GhostMark, GhostGlyph)", "the night chip at 36 in both themes, its arc by phase; the live mascot on hover inside the chip"],
  ["Favicons", "apps/desktop/public/favicon.{svg,ico}; site/src/pages/favicon.svg.ts", "I's 16/32 drawing (the site's favicon is already cyan on black, halfway there)"],
  ["Phone launcher and adaptive layers", "apps/mobile/assets/{icon,android-icon-foreground,android-icon-background,android-icon-monochrome}.png; app.json backgroundColor #5560E8", "II foreground inside the 66 dp circle, night background, one-colour ring for themed icons; backgroundColor to #05060A"],
  ["Phone splash and favicon", "apps/mobile/assets/{splash-icon,favicon}.png", "the splash lockup above; favicon as the desktop's"],
  ["Phone's notification small icon", "(not set in app.json today)", "the one-colour 24 dp drawing; the lit one in the card"],
  ["Site", "site/src/components/Wordmark.astro, site/public/{apple-touch-icon.png,og.jpg}, Base.astro", "the lockup in the nav; apple-touch-icon is II at 180; og.jpg is the reach card"],
  ["README and GitHub", "README.md header; the org's avatar (uploaded by hand)", "the lockup image on top of the README; II at 512 as the avatar"],
  ["In-app references", "packages/ui/src/{brand,wisp}.ts, docs/design-system.md (Iconography, Page layout)", "a still-pose export next to wispGlyph(); the doc's brand paragraph rewritten"],
];

const recommend = `
<section class="rec" data-name="recommendation">
  <h2>Recommendation: II, the instrument</h2>
  <div class="rec__lead">
    <div><img src="../svg/instrument-1024.svg" width="300" height="300" alt=""></div>
    <div>
      <p class="big">The brief's lean was right, with one correction: <b>A and B are one mark, not two</b>, and D is how it scales, not a fourth idea. Lit Wisp inside the ring from 48 px up; under 32 the ring goes and Lit's small drawing is the mark. Light is the brand: the night tile is never optional, so on a light page the mark is a dark chip, not a bare glyph. The reach (C) is what the mark does on a big surface.</p>
      <table class="scores">
        <tr><th></th><th>Out of 10, against the floating room</th></tr>
        <tr><td>The floating room itself</td><td><b>9</b>: lit, alive, specific; the bar everything is measured against</td></tr>
        <tr><td>Today's mark</td><td><b>4</b>: friendly, clean, and any app's sticker; mush at 16; the indigo slab is the one colour the room never uses</td></tr>
        <tr><td>I · Lit</td><td><b>7</b>: the best 16 px of the three; not a system on its own</td></tr>
        <tr class="hl"><td>II · Instrument</td><td><b>8.5</b> at 48 and up, <b>7.5</b> at 16 to 32 (where it is I): the room, made still</td></tr>
        <tr><td>III · Eyes in the dark</td><td><b>6.5</b>: the most striking poster, the least Wisp</td></tr>
      </table>
    </div>
  </div>
  <div class="rec__cols">
    <div>
      <h3>Must stay flat or one-colour</h3>
      <ul>
        <li><b>The tray at rest</b>: GNOME's panel is one-colour; a lit tile there looks like an ad. Lit only during a session, in the state's colour.</li>
        <li><b>Android's themed icon and the notification's small icon</b>: the system paints them; the one-colour drawing is the only one that survives.</li>
        <li><b>Forced colours and print</b>: one colour, eyes cut out.</li>
        <li><b>The agents' glyphs</b> (<code>LineGlyph</code>) stay flat in their hue on their soft tile: the lit treatment belongs to Ghost, not to every agent in a list.</li>
      </ul>
      <h3>What I would not do</h3>
      <ul>
        <li>Animate the dock icon or the favicon on a timer. A living icon is the tray and the rail, which already know the state; a favicon that pulses on a site is noise.</li>
        <li>Put the beams in anything smaller than a splash.</li>
        <li>Keep the indigo. Iris stays the app's accent for actions; it is not the mark's light. The mark's light is the state's colour, cyan at rest.</li>
        <li>Make the ring turn in the dock. It turns in the room (0.05 rad/s); a still mark is still.</li>
        <li>Redraw the agents' family now. They sit beside the new Wisp as they are; taller eyes for them is a later, separate call.</li>
      </ul>
    </div>
    <div>
      <h3>What it costs to roll out</h3>
      <table class="roll">${ROLL.map(([a, b, c]) => `<tr><td>${a}</td><td><code>${b}</code></td><td>${c}</td></tr>`).join("")}</table>
      <p class="mini">One generator (this folder's <code>src/marks.mjs</code>, moved next to <code>wisp.ts</code> and reading its <code>body()</code>) writes every file above, so the still mark and the live mascot can't drift. About two days: the generator into <code>packages/ui</code>, the Tauri and phone exports, the rail's chip, the tray's two drawings, the site and the doc. The mascot's live drawing changes only in its rest pose.</p>
    </div>
  </div>
</section>`;

const intro = `
<section class="intro" data-name="intro">
  <div class="intro__text">
    <p class="eyebrow">GHOST · THE MARK · CONCEPT ROUND · NOVEMBER 2026</p>
    <h1>Make the mark the floating room, held still.</h1>
    <p class="lede">The operator: <i>“the floating room version of Ghost is levels ahead … compared to the ‘logo’.”</i> They are right, and the gap is not the drawing: it is light, pose and the instrument around it. Three directions, each a whole system, against today's in the same places. Nothing shipped changes in this round.</p>
  </div>
  <div class="intro__gap">
    <figure><img src="../png/today-256.png" width="180" height="180" alt=""><figcaption>Today · 4/10</figcaption></figure>
    <figure class="room"><img src="../../../screenshots/floating-hero.webp" alt=""><figcaption>The floating room · 9/10</figcaption></figure>
    <figure><img src="../svg/instrument-1024.svg" width="180" height="180" alt=""><figcaption>Recommended · 8.5/10</figcaption></figure>
  </div>
</section>`;

const motionSec = `
<section class="motion" data-name="motion">
  <h2>Motion <small>for the places that can animate: the rail's chip, the splash, the site's hero, the README</small></h2>
  <div class="motion__row">
    <figure><img src="../motion/arrival.webp" width="256" height="256" alt=""><figcaption><b>Arrival</b> (1.2 s): from depth with the room's shockwave, the ticks drawing on clockwise, the arc lighting last</figcaption></figure>
    <figure><img src="../motion/breath.webp" width="256" height="256" alt=""><figcaption><b>Idle breath</b> (4 s loop): the light rises and falls 8 %, Wisp drifts 1.5 %, one blink; the ring does not turn</figcaption></figure>
    <figure><img src="../motion/states.webp" width="256" height="256" alt=""><figcaption><b>A state change</b>: ready → listening → reaching → speaking → waiting for a yes; the arc travels, the light changes colour over 450 ms (the mascot's mood blend)</figcaption></figure>
  </div>
</section>`;

const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ghost mark concepts</title>
<link rel="stylesheet" href="sheet.css">
</head><body>
<main class="sheet">
${intro}
${character}
${DIRS.map(dirSection).join("")}
${contexts}
${big2}
${word}
${motionSec}
${recommend}
</main>
</body></html>`;

writeFileSync(join(ROOT, "sheet", "index.html"), html);
console.log("sheet: sheet/index.html");
