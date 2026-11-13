import { describe, expect, it } from "vitest";

import { insideSafe, layoutOf, MAX_SHARE, PLACEMENTS, type LayoutInput } from "../src/floating/layout";

const MONITORS: [string, number, number][] = [
  ["1920×1080", 1920, 1080],
  ["1280×720 laptop", 1280, 720],
  ["portrait 1080×1920", 1080, 1920],
  ["4K at scale 1", 3840, 2160],
  ["small 1024×600", 1024, 600],
];

describe("the layout rule", () => {
  for (const [name, w, h] of MONITORS)
    for (const placement of PLACEMENTS)
      for (const side of ["left", "right", null] as const)
        for (const places of [0, 1, 3, 6])
          for (const card of [false, true])
            for (const captions of [false, true]) {
              const i: LayoutInput = { w, h, placement, side, places, card, captions };
              it(`${name} ${placement} ${side ?? "-"} places=${places}${card ? " card" : ""}${captions ? " captions" : ""}: inside the safe area and its share`, () => {
                const l = layoutOf(i);
                expect(insideSafe(l.bounds, w, h)).toBe(true);
                for (const p of l.places) expect(insideSafe({ x: p[0] - l.label.w / 2, y: p[1] - l.label.h / 2, w: l.label.w, h: l.label.h }, w, h)).toBe(true);
                if (l.card) expect(insideSafe(l.card, w, h)).toBe(true);
                if (l.captions) expect(insideSafe(l.captions, w, h)).toBe(true);
                expect(insideSafe({ x: l.core[0] - l.wisp / 2, y: l.core[1] - l.wisp / 2, w: l.wisp, h: l.wisp }, w, h)).toBe(true);
                expect(l.share).toBeLessThanOrEqual(MAX_SHARE[placement]);
                expect(l.places).toHaveLength(places);
              });
            }

  it("keeps beside and the corner on the side away from the work", () => {
    const right = layoutOf({ w: 1920, h: 1080, placement: "beside", side: "right", places: 3, card: false, captions: false });
    const left = layoutOf({ w: 1920, h: 1080, placement: "beside", side: "left", places: 3, card: false, captions: false });
    expect(right.core[0]).toBeGreaterThan(1920 * 0.7);
    expect(left.core[0]).toBeLessThan(1920 * 0.3);
    // The places fan toward the middle of the screen.
    for (const p of right.places) expect(p[0]).toBeLessThanOrEqual(right.core[0]);
    const corner = layoutOf({ w: 1920, h: 1080, placement: "corner", side: null, places: 0, card: false, captions: false });
    expect(corner.core[0]).toBeGreaterThan(1920 * 0.8);
    expect(corner.core[1]).toBeGreaterThan(1080 * 0.6);
  });

  it("does not grow with what is said: a card and captions are fixed boxes", () => {
    const a = layoutOf({ w: 1920, h: 1080, placement: "centre", side: null, places: 0, card: true, captions: true });
    expect(a.card!.w).toBeLessThanOrEqual(640);
    expect(a.captions!.h).toBeLessThanOrEqual(80);
  });
});
