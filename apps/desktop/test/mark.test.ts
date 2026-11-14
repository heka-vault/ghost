/**
 * The mark (November 2026, "hem with hands"; docs/design-system.md, The mark): one drawing for
 * the live mascot and every still mark, no tail, and the size ladder.
 */
import { describe, expect, it } from "vitest";

import {
  ARCS,
  GHOST_PATH,
  GHOST_PATH_SMALL,
  HEM,
  WISP,
  WISP_BOX,
  detailFor,
  hemCurves,
  hemPath,
  ladder,
  markSvg,
  mittPaths,
  mittPlaces,
  monoSvg,
  traySvg,
  adaptiveSvg,
  wordmarkSvg,
  reachCardSvg,
  REACH_PLACES,
  desktopIconFiles,
  trayIconFiles,
} from "@ghost/ui";

import { markStateOf } from "../src/app/mood";

type P = [number, number];

/** Points along the body below the head (the cubics), and the head's half circle. */
function outline(s = 0): P[] {
  const pts: P[] = [];
  for (let a = 0; a <= 180; a += 3) pts.push([12 - 7 * Math.cos((a * Math.PI) / 180), 10 - 7 * Math.sin((a * Math.PI) / 180)]);
  let from: P = [19, 10];
  for (const k of hemCurves(s)) {
    for (let i = 1; i <= 24; i++) {
      const t = i / 24;
      const u = 1 - t;
      const at = (j: 0 | 1) => u * u * u * from[j] + 3 * u * u * t * k.c1[j] + 3 * u * t * t * k.c2[j] + t * t * t * k.to[j];
      pts.push([at(0), at(1)]);
    }
    from = k.to;
  }
  return pts;
}

/** Points round an oval mitt. */
function mittOutline(m: { cx: number; cy: number; rot: number }): P[] {
  const a = (m.rot * Math.PI) / 180;
  const out: P[] = [];
  for (let i = 0; i < 72; i++) {
    const t = (i / 72) * 2 * Math.PI;
    const x = 1.9 * Math.cos(t);
    const y = 1.3 * Math.sin(t);
    out.push([m.cx + x * Math.cos(a) - y * Math.sin(a), m.cy + x * Math.sin(a) + y * Math.cos(a)]);
  }
  return out;
}

const nearest = (p: P, set: P[]) => Math.min(...set.map((q) => Math.hypot(p[0] - q[0], p[1] - q[1])));

describe("the silhouette", () => {
  it("has no tail: at rest the body is symmetric about the middle, so nothing points off to one side", () => {
    const pts = outline(0);
    const mirrored = pts.map(([x, y]) => [24 - x, y] as P);
    const worst = Math.max(...mirrored.map((p) => nearest(p, pts)));
    expect(worst).toBeLessThan(0.12);
  });

  it("closes in a rounded hem of three shallow lobes, never a point below it", () => {
    const pts = outline(0);
    const bottom = Math.max(...pts.map((p) => p[1]));
    // The lobes' depth below the base, no more: the old tail ran to y 23.6.
    expect(bottom).toBeLessThan(HEM.base + HEM.depth + 0.05);
    expect(bottom).toBeGreaterThan(HEM.base + 0.5);
    // Nothing below the base outside the hem's width (a tail curls out past the sides).
    for (const [x, y] of pts) if (y > HEM.base) expect(x > 4.4 && x < 19.6).toBe(true);
    // Three lobes: three local lowest points along the bottom.
    const hem = pts.filter((p) => p[1] > HEM.base - 0.01).sort((a, b) => a[0] - b[0]);
    let lows = 0;
    for (let i = 1; i < hem.length - 1; i++) if (hem[i][1] > hem[i - 1][1] && hem[i][1] >= hem[i + 1][1]) lows++;
    expect(lows).toBe(3);
  });

  it("keeps the old tail's drawing out of the body", () => {
    expect(hemPath(0)).not.toContain("18.6 22.9");
    expect(GHOST_PATH).not.toContain("16.2 19.8");
  });

  it("ripples the hem when it swings, inside the same bounds, and is still at rest at 0", () => {
    expect(hemPath(0.5)).not.toBe(hemPath(0));
    expect(hemPath(-0.5)).not.toBe(hemPath(0.5));
    for (const s of [-1, -0.5, 0.35, 1]) {
      const pts = outline(s);
      expect(Math.max(...pts.map((p) => p[1]))).toBeLessThan(21.6);
      expect(Math.min(...pts.map((p) => p[0]))).toBeGreaterThan(4.2);
      expect(Math.max(...pts.map((p) => p[0]))).toBeLessThan(19.8);
    }
  });

  it("draws the body and the mitts clockwise and the eyes against it, so one path fills the overlaps and cuts the eyes", () => {
    for (const m of mittPaths()) expect(m).toMatch(/A[\d.]+ [\d.]+ -?[\d.]+ 1 1 /);
    expect(GHOST_PATH).toMatch(/a1\.2 2 0 1 0/);
  });

  it("has a right mitt raised a little above the left at rest", () => {
    const [l, r] = mittPlaces();
    expect(r.cy).toBeLessThan(l.cy);
  });

  it("fits the box it says it has", () => {
    const pts = [...outline(0), ...mittPlaces().flatMap(mittOutline)];
    const box = [Math.min(...pts.map((p) => p[0])), Math.min(...pts.map((p) => p[1])), Math.max(...pts.map((p) => p[0])), Math.max(...pts.map((p) => p[1]))];
    box.forEach((v, i) => expect(Math.abs(v - WISP_BOX[i])).toBeLessThan(0.25));
  });
});

describe("the still mark and the live one are one drawing", () => {
  it("the mascot at rest draws the brand's body and mitts", () => {
    const a = WISP.attrs(WISP.rest, detailFor(96), 0);
    expect(a.body.d).toBe(hemPath(0));
    const [l, r] = mittPaths();
    expect(a.mittL.d).toBe(l);
    expect(a.mittR.d).toBe(r);
    expect(GHOST_PATH.startsWith(hemPath(0) + l + r)).toBe(true);
    expect(GHOST_PATH_SMALL.startsWith(hemPath(0))).toBe(true);
    expect(GHOST_PATH_SMALL).not.toContain(l);
  });

  it("every icon is drawn from the same body", () => {
    for (const px of [16, 32, 128, 512]) expect(markSvg(px)).toContain(hemPath(0));
    expect(markSvg(512)).toContain(mittPaths()[1]);
    expect(monoSvg(48)).toContain(hemPath(0));
    expect(traySvg(32)).toContain(hemPath(0));
    expect(adaptiveSvg("foreground")).toContain(hemPath(0));
  });

  it("the mascot's swing ripples the same hem the still mark has at rest", () => {
    const a = WISP.attrs({ ...WISP.rest, sw: 0.4 }, detailFor(96), 0);
    expect(a.body.d).toBe(hemPath(0.4));
  });

  it("every mood still moves and draws real numbers", () => {
    for (const [name, mood] of Object.entries(WISP.moods)) {
      for (const t of [0, 400, 1300, 2900]) {
        const a = WISP.attrs({ ...WISP.rest, ...mood.pose(t) }, detailFor(176), t);
        expect(JSON.stringify(a), name).not.toMatch(/NaN|Infinity/);
      }
    }
    const wave = WISP.attrs({ ...WISP.rest, ...WISP.enter(500, { look: { x: 0, y: 0 } }) }, detailFor(176), 500);
    expect(wave.mittR.d).not.toBe(mittPaths()[1]);
  });
});

describe("the size ladder", () => {
  it("draws the ring of ticks from 128 px, the mitts from 32, pixel eyes under 32", () => {
    expect(ladder(16)).toMatchObject({ ticks: false, mitts: false, pixelEyes: true, smallEyes: true, bloom: false });
    expect(ladder(24)).toMatchObject({ ticks: false, mitts: false, pixelEyes: true, smallEyes: false });
    expect(ladder(32)).toMatchObject({ ticks: false, mitts: true, pixelEyes: false, bloom: false });
    expect(ladder(48)).toMatchObject({ ticks: false, mitts: true, bloom: true });
    expect(ladder(96)).toMatchObject({ ticks: false });
    expect(ladder(128)).toMatchObject({ ticks: true, streak: true });
    expect(ladder(512)).toMatchObject({ ticks: true });
  });

  it("puts the ticks in the icon only where they read", () => {
    const ticks = (svg: string) => (svg.match(/<line /g) ?? []).length;
    expect(ticks(markSvg(48))).toBe(0);
    expect(ticks(markSvg(96))).toBe(0);
    expect(ticks(markSvg(128))).toBe(72);
    expect(ticks(markSvg(512))).toBe(120);
  });

  it("the live mascot follows it: mitts from 32 px, larger eyes under 24", () => {
    expect(detailFor(31).props).toBe(false);
    expect(detailFor(32).props).toBe(true);
    expect(detailFor(23).simple).toBe(true);
    expect(WISP.scene(detailFor(24)).body).toHaveLength(1);
    expect(WISP.scene(detailFor(40)).body).toHaveLength(3);
  });
});

describe("the rules of the mark", () => {
  it("is lit on the night tile, never the indigo slab, and has no smile", () => {
    const svg = markSvg(256);
    expect(svg).toContain("#05060a");
    expect(svg.toLowerCase()).not.toMatch(/#5560e8|#4f5bd5|indigo/);
    expect(svg).not.toMatch(/Q/); // the smile and the grin are quadratic arcs
  });

  it("lights one arc in the state's colour, the whole ring speaking, and strikes Not listening", () => {
    expect(markSvg(128, { state: "listening" })).toContain("#4dffb8");
    expect(markSvg(128, { state: "waiting" })).toContain("#ffb547");
    expect(markSvg(32, { state: "off" })).toContain("#ff4d5e");
    expect(traySvg(32, "off")).toContain("#ff4d5e");
    expect(ARCS.speaking).toEqual([0, 0]);
  });

  it("the ids of two inlined marks never collide", () => {
    const ids = (s: string) => [...s.matchAll(/id="([^"]+)"/g)].map((m) => m[1]);
    const a = ids(markSvg(128, { id: "a" }));
    const b = ids(markSvg(128, { id: "b" }));
    expect(a.filter((x) => b.includes(x))).toEqual([]);
  });

  it("the wordmark says Ghost beside the tile, with room for the whole word", () => {
    const svg = wordmarkSvg(64);
    expect(svg).toContain(">Ghost</text>");
    // "Ghost" in Inter 800 at −0.035 em is 2.79 em wide (measured in Chrome): it must not be cut at the "t".
    const width = Number(/width="(\d+)"/.exec(svg)?.[1]);
    const fs = 64 * 0.62;
    const x = Number(/<text x="([\d.]+)"/.exec(svg)?.[1]);
    expect(width - x).toBeGreaterThanOrEqual(fs * 2.79);
  });

  it("the social card is the reach: Wisp reaching, a beam and a name for every place, the line", () => {
    const svg = reachCardSvg();
    expect(svg).toContain('width="1200" height="630"');
    expect(svg).toContain(">Ghost</text>");
    expect(svg).toContain("#a98bff");
    for (const p of REACH_PLACES) expect(svg).toContain(`>${p.name}</text>`);
    expect((svg.match(/<line /g) ?? []).length).toBeGreaterThanOrEqual(REACH_PLACES.length * 2);
    expect(svg).not.toContain("evenodd");
    // Wisp at rest is in it: the hem and the mitts.
    expect(svg).toContain(hemPath(0));
    for (const m of mittPaths()) expect(svg).toContain(m);
  });

  it("names every file the desktop ships", () => {
    expect(desktopIconFiles().map((f) => f.name)).toEqual(expect.arrayContaining(["32x32.png", "48x48.png", "128x128.png", "128x128@2x.png", "256x256.png", "icon.png"]));
    expect(trayIconFiles().map((f) => f.name)).toEqual(["tray-rest.png", "tray-listening.png", "tray-reaching.png", "tray-speaking.png", "tray-waiting.png", "tray-off.png"]);
  });

  it("the rail's chip and the tray follow the phase, a yes and the switch", () => {
    expect(markStateOf("passive", false)).toBe("idle");
    expect(markStateOf("listening", false)).toBe("listening");
    expect(markStateOf("thinking", false)).toBe("reaching");
    expect(markStateOf("speaking", false)).toBe("speaking");
    expect(markStateOf("listening", false, true)).toBe("waiting");
    expect(markStateOf("passive", true, true)).toBe("off");
  });
});
