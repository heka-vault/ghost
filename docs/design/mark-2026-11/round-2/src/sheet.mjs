/**
 * Round 2's sheet (sheet/index.html): the silhouette test first, then each direction in the
 * ring and in context with the agents beside it, the loops, and the recommendation. Shorter
 * than round 1 on purpose; it reuses round 1's stylesheet.
 *
 *   node docs/design/mark-2026-11/round-2/src/sheet.mjs
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { FIGURES } from "./figures.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
mkdirSync(join(ROOT, "sheet"), { recursive: true });
const png = (n) => `../png/${n}.png`;
const svgf = (n) => `../svg/${n}.svg`;
const img = (src, px, cls = "") => `<img class="px ${cls}" src="${src}" width="${px}" height="${px}" alt="">`;

// The silhouette test: what each reads as in solid black, said bluntly.
const SIL = [
  ["today", "a drop", "bad", "The operator's read: an inverted raindrop, or worse. The single pointed tail is what does it."],
  ["todayArms", "a ghost", "ok", "The stick arms rescue it: with them the drop becomes a figure."],
  ["round1", "a comma", "bad", "Round 1 made it worse: the tail swung out further, the arms gone."],
  ["hem", "a ghost", "good", "Read at once, at 64 and at 16. The 👻 family; the risk is generic."],
  ["hemRound", "a tooth", "bad", "One round bottom: a molar, a thumb, a marshmallow. Never a ghost."],
  ["hemMitts", "a ghost, waving", "good", "The strongest read: a ghost and a character. The hands make it someone."],
  ["tailMitts", "a ghost, with the tail", "ok", "What the hands do alone: they make it a figure, but the tail is still there."],
  ["fade", "a tombstone", "ok", "In black it is a stone fading out; it only works lit, rising from the light."],
  ["peek", "a tombstone", "bad", "A dome cut by the frame: a gravestone or a bullet, until it has eyes and light."],
  ["orb", "a blob", "bad", "A blob with eyes: slime, mochi, any assistant's orb. The lean doesn't save it."],
  ["drift", "a ghost, leaning", "ok", "Hem, hands and a lean: alive, but the tilted hem reads as a mistake at 16."],
];

const silhouettes = `
<section class="r2sil" data-name="silhouettes">
  <p class="eyebrow">GHOST · THE MARK · ROUND 2 · NOVEMBER 2026</p>
  <h1>Lose the tail, keep the hands.</h1>
  <p class="lede">The operator: <i>“the tail … makes it look like an inverted raindrop, or frankly something inappropriate. I like the first option with the hands.”</i> The light, the night tile, the ring and the state colours stay as round 1 drew them; this round is the silhouette. The test that matters is the shape alone, solid black, at 64 and 16 px.</p>
  <div class="sil">
    ${SIL.map(([k, word, verdict, why]) => `<figure class="sil__one ${verdict}"><div class="sil__box">${img(png(`sil-${k}-64`), 64)}${img(png(`sil-${k}-16`), 16)}</div><b>${word}</b><figcaption><span>${FIGURES[k].label}</span>${why}</figcaption></figure>`).join("")}
  </div>
</section>`;

/** A direction: the mark at 1024 (shown at 200), 128, 48, 32, 16 and 16 zoomed; then in context; then the family. */
const DIRS = [
  ["hem", "1a · Rounded hem", "Wisp's head and shoulders, the bottom closed in three shallow round lobes, symmetric. The sheet ghost.", "Floats as today; the hem ripples where the tail swung (a wave across the three lobes). Listening, reach, speaking and the yes are the ring's and the light's, unchanged.", "7"],
  ["hemRound", "1b · One round hem", "The bottom closed in one soft curve.", "As 1a with nothing to ripple: it can only squash.", "3"],
  ["hemMitts", "1c · Hem with hands", "1a with hands drawn as part of the body: two rounded mitts at the shoulders, lit with it, the right a little raised. From 32 px up; under 32 they would only widen the blob.", "The live mascot already has arms (aL, aR): they become these mitts, so the wave on hover, the gesture while speaking and the arms down when offline all carry over. The hem's ripple replaces the tail's swing.", "8.5"],
  ["tailMitts", "2 · Hands on today's tail", "For comparison: what the hands do alone, on the body that has the tail.", "Today's motion with mitts instead of lines.", "6"],
  ["fade", "3a · No bottom half, centred", "Only the head and shoulders are drawn; the lower third dissolves into a pool of light. Nothing to be read as a tail or a hem.", "The float is the best home for it: the body dissolves into the field's glow and rises out of it on arrival. It has no limbs, so the gestures go to the light and the ring.", "7.5"],
  ["peek", "3b · Rising from the frame", "The head rises over the ring's edge, the bottom cut by it.", "In the float there is no frame to rise from except the ring, so it would have to bring one; on a tile or a splash it is good.", "6"],
  ["orb", "4 · Orb", "Nearly round, fuller low on the right, leaning: a drop of light settling, the eyes the only feature.", "Alive through the outline breathing and the light. It is every assistant's orb: the eyes are the only thing that is Ghost.", "5"],
  ["drift", "5 · Drift", "1c with a lean (−5°) and the hem lower on the right: the round-1 sense of drift without a tail.", "As 1c.", "7"],
];

const ctxStrip = (k) => `
<div class="r2ctx">
  <div class="wall busy small"><div class="dock">${GEN_ICONS}<span class="dock__me">${img(png(`${k}-48`), 48)}<i></i></span></div></div>
  <div class="r2ctx__col">
    <div class="bar dark mini2"><span class="bar__l">Thu 14:32</span><span class="bar__r">${img(png(`${k}-mono-16`), 16)}${img(png(`${k}-listening-16`), 16)}</span></div>
    <div class="bar light mini2"><span class="bar__l">Thu 14:32</span><span class="bar__r">${img(png(`${k}-monodark-16`), 16)}${img(png(`${k}-listening-16`), 16)}</span></div>
    <div class="tabs light"><span class="tab"><span class="gfav" style="background:#2f6fe4"></span><span>Pull requests</span></span><span class="tab on">${img(png(`${k}-16`), 16)}<span>Ghost</span></span></div>
  </div>
  <span class="ad" style="width:64px;height:64px;border-radius:50%;background:#05060a"><img src="${svgf(`${k}-fg`)}" style="width:96px;height:96px;left:-16px;top:-16px"></span>
  <span class="ad" style="width:64px;height:64px;border-radius:36%;background:#05060a"><img src="${svgf(`${k}-fg`)}" style="width:96px;height:96px;left:-16px;top:-16px"></span>
  <div class="railbox dark r2rail"><div class="rail__brand"><span class="rail__chip">${img(png(`${k}-listening-36`), 36)}</span><span>Listening</span></div></div>
</div>`;

const GEN_ICONS = `<svg viewBox="0 0 48 48" width="48" height="48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#e8a33d"/><path d="M10 16h11l3 3h14v17H10z" fill="#fff4dc"/></svg><svg viewBox="0 0 48 48" width="48" height="48"><rect x="2" y="2" width="44" height="44" rx="11" fill="#2b2f37"/><path d="M12 18l7 6-7 6" stroke="#e6e9ef" stroke-width="3" fill="none" stroke-linecap="round" stroke-linejoin="round"/><path d="M23 31h12" stroke="#e6e9ef" stroke-width="3" stroke-linecap="round"/></svg>`;

const dir = ([k, title, idea, float, score]) => `
<section class="r2dir ${k === "hemMitts" ? "rec" : ""}" data-name="dir-${k}">
  <div class="r2dir__head"><h2>${title}${k === "hemMitts" ? ' <b class="rectag">recommended</b>' : ""}</h2><p class="score">${score}<small>/10</small></p></div>
  <div class="r2dir__body">
    <div class="r2dir__text"><p>${idea}</p><p><b>In the floating room:</b> ${float}</p></div>
    <div class="r2dir__ladder">
      <figure><img src="${svgf(`${k}-1024`)}" width="200" height="200" alt=""><figcaption>1024 at 200</figcaption></figure>
      <figure>${img(png(`${k}-128`), 128)}<figcaption>128</figcaption></figure>
      <figure>${img(png(`${k}-48`), 48)}<figcaption>48</figcaption></figure>
      <figure>${img(png(`${k}-32`), 32)}<figcaption>32</figcaption></figure>
      <figure>${img(png(`${k}-16`), 16)}<figcaption>16</figcaption></figure>
      <figure><img class="px" src="${png(`${k}-16`)}" width="96" height="96" alt=""><figcaption>16, every pixel</figcaption></figure>
    </div>
  </div>
  ${ctxStrip(k)}
  <div class="r2fam">${Array.from({ length: 8 }, (_, i) => `<img src="${png(`fam-${k}-${i}`)}" width="56" height="56" alt="">`).join("")}<span>with the agents' shapes, unchanged</span></div>
</section>`;

const states = `
<section class="r2states" data-name="states">
  <h2>1c in every state <small>the arc says what, the light says how; unchanged from round 1</small></h2>
  <div class="states">${["idle", "listening", "reaching", "speaking", "waiting", "off"].map((s) => `<figure>${img(png(`hemMitts-${s}-128`), 128)}</figure>`).join("")}</div>
</section>`;

const motion = `
<section class="motion" data-name="motion">
  <h2>The two strongest, alive <small>the arrival and speaking, in the ring</small></h2>
  <div class="motion__row">
    <figure><img src="../motion/hem-mitts-arrival.webp" width="220" height="220" alt=""><figcaption><b>1c arrival</b>: the hem settles last, the right hand waves once</figcaption></figure>
    <figure><img src="../motion/hem-mitts-speaking.webp" width="220" height="220" alt=""><figcaption><b>1c speaking</b>: the ring takes the voice, the hem ripples, the hands gesture, the mouth (the float's only)</figcaption></figure>
    <figure><img src="../motion/fade-arrival.webp" width="220" height="220" alt=""><figcaption><b>3a arrival</b>: rising out of its light</figcaption></figure>
    <figure><img src="../motion/fade-speaking.webp" width="220" height="220" alt=""><figcaption><b>3a speaking</b>: nothing below the shoulders to move, so the light and the ring do it all</figcaption></figure>
  </div>
</section>`;

const rec = `
<section class="rec" data-name="recommendation">
  <h2>Recommendation: 1c, the hem with hands</h2>
  <div class="rec__lead">
    <div><img src="${svgf("hemMitts-1024")}" width="300" height="300" alt=""></div>
    <div>
      <p class="big">It passes the black test at 64 and at 16 as <b>a ghost</b>, and with the hands as <b>someone</b>, which is what the operator liked in today's large icon. No tail, so no drop and nothing rude. Everything round 1 got right stays: the night tile, the ring and its arc, the light by state. Under 32 px the hands drop and the hem alone is the mark (round 1's rule: one system, levels of detail). The honest cost: a three-lobed hem is the most familiar ghost there is (the emoji, the arcade). The light, the ring and the hands are what make it Ghost's and not anyone's; on its own, in black, it is generic.</p>
      <table class="scores">
        <tr><th></th><th>Out of 10, against the floating room (9)</th></tr>
        <tr><td>Today</td><td><b>4</b>: a drop at small sizes</td></tr>
        <tr><td>Round 1 (II)</td><td><b>8.5</b> for the system, but its silhouette fails the test: a comma</td></tr>
        <tr class="hl"><td>1c · Hem with hands</td><td><b>8.5</b>: reads as a ghost and a character at every size; generic in black</td></tr>
        <tr><td>3a · No bottom half</td><td><b>7.5</b>: the most like the room, the least like a mascot; a tombstone in black</td></tr>
        <tr><td>1a · Hem, 5 · Drift</td><td><b>7</b> each</td></tr>
        <tr><td>3b · Peek, 2 · Tail with hands</td><td><b>6</b> each</td></tr>
        <tr><td>4 · Orb</td><td><b>5</b>: every assistant's orb</td></tr>
        <tr><td>1b · One round hem</td><td><b>3</b>: a tooth</td></tr>
      </table>
    </div>
  </div>
  <div class="rec__cols">
    <div>
      <h3>The live mascot (packages/ui/src/wisp.ts)</h3>
      <ul>
        <li><b><code>body(s)</code> becomes <code>hem(w, ph)</code></b>: the same head, the three lobes' depths moving as a wave across the hem. <code>sw</code> is renamed to the ripple's strength and keeps every place it is driven today (idle, listening, speaking, hold, the gesture); its phase runs on the clock, so the hem flows instead of swinging.</li>
        <li><b>Arms become mitts</b>: <code>aL</code>/<code>aR</code> and every pose that sets them are kept; only the drawing changes, from two stroked lines to two filled ovals hinged at the shoulders, inside the body's light. They show from 32 px (the props detail is 40 today; 32 is fine for mitts).</li>
        <li><b>The rest pose is upright</b>, the right mitt a little raised. The talking mouth, the grin, the blink, the thinking dots and the "z" stay.</li>
        <li><code>GHOST_PATH</code> and <code>GHOST_PATH_SMALL</code> come from the new body, so the favicon generator (<code>site/src/pages/favicon.svg.ts</code>) and the site's wordmark follow without edits.</li>
      </ul>
    </div>
    <div>
      <h3>The agents' shapes (agentShapes.ts)</h3>
      <ul>
        <li><b>None of them shares Wisp's tail</b>: Drop, Bell, Hood, Cloud, Lantern, Star and Moth are their own silhouettes. Only Wisp and an agent with no look (drawn as Wisp in its colour) change.</li>
        <li><b>The hem makes Wisp their sibling</b>: Bell, Cloud and Hood already end in a hem. Wisp stays distinct by its straight sides and round lobes (Bell flares, Hood is jagged, Cloud is puffed on top) and, from 32 up, the hands.</li>
        <li><b>Drop gets lonelier</b>: it was Wisp's closest relative and is the shape the operator objected to. Keep it as an agent's shape; it no longer stands for Ghost.</li>
      </ul>
      <h3>The published imagery</h3>
      <ul>
        <li><b>The README's floating-room pictures and the loop</b> (<code>docs/screenshots/floating-*.webp</code>) show the live Wisp with its tail: rerun <code>make screenshots</code> after the mascot changes. The site's <code>og.jpg</code> has no script behind it (captured once with the site): capture it again, or replace it with round 1's reach card.</li>
        <li><b>The site's float demo</b> (<code>site/src/components/FloatDemo.tsx</code>) renders the real floating room, so it follows the mascot.</li>
        <li><b>The icons and favicons</b> are round 1's rollout list, with this body.</li>
      </ul>
    </div>
  </div>
</section>`;

const css = `
.r2sil .sil { display: grid; grid-template-columns: repeat(6, 1fr); gap: 14px; margin-top: 28px; }
.sil__one { background: var(--panel); border-radius: 12px; padding: 14px; display: grid; gap: 6px; align-content: start; }
.sil__box { background: #fff; border-radius: 8px; height: 96px; display: flex; gap: 18px; align-items: center; justify-content: center; }
.sil__one b { font: 800 18px/1.1 var(--sans); }
.sil__one.good b { color: var(--green); }
.sil__one.ok b { color: var(--amber); }
.sil__one.bad b { color: #ff6b7a; }
.sil__one figcaption { margin: 0; font-size: 11px; }
.sil__one figcaption span { display: block; color: var(--ink); margin-bottom: 3px; }
.r2dir { padding: 36px 64px; }
.r2dir.rec { background: radial-gradient(ellipse 40% 60% at 15% 40%, rgb(95 227 255 / 0.07), transparent 70%); }
.r2dir__head { display: flex; justify-content: space-between; align-items: baseline; }
.r2dir__head .score { font: 800 44px/1 var(--sans); letter-spacing: -0.04em; margin: 0; }
.r2dir__head .score small { font-size: 16px; color: var(--faint); }
.r2dir__body { display: grid; grid-template-columns: 380px 1fr; gap: 36px; align-items: end; }
.r2dir__text p { color: var(--dim); font-size: 14.5px; margin: 0 0 10px; }
.r2dir__text b { color: var(--ink); }
.r2dir__ladder { display: flex; gap: 22px; align-items: flex-end; }
.r2dir__ladder figure { display: grid; justify-items: center; }
.r2ctx { display: flex; gap: 18px; align-items: center; margin-top: 22px; }
.wall.small { width: 260px; height: 84px; padding-bottom: 8px; flex: none; }
.r2ctx__col { display: grid; gap: 6px; width: 300px; }
.bar.mini2 { height: 26px; }
.r2rail { flex: none; padding: 12px 14px; }
.r2rail .rail__brand { color: #c9ced8; }
.r2fam { display: flex; gap: 10px; align-items: center; margin-top: 16px; padding: 8px 14px; background: var(--night); border-radius: 12px; border: 1px solid var(--line); width: fit-content; }
.r2fam span { font: 500 11px/1 var(--mono); color: var(--faint); margin-left: 8px; }
.r2states .states figure { gap: 0; }
`;

const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ghost mark, round 2</title>
<link rel="stylesheet" href="../../sheet/sheet.css">
<style>${css}</style>
</head><body><main class="sheet">
${silhouettes}
${DIRS.map(dir).join("")}
${states}
${motion}
${rec}
</main></body></html>`;
writeFileSync(join(ROOT, "sheet", "index.html"), html);
console.log("round-2 sheet: sheet/index.html");
