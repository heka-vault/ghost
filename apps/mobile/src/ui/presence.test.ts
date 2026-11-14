import { ARCS, MARK_STATES } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { lookOf, OFF_LIGHT } from "./presence";

describe("the presence shows each phase as the mark", () => {
  it("lights each phase in its state's colour, with its arc", () => {
    expect(lookOf("listening", null, false)).toMatchObject({ state: "listening", light: MARK_STATES.listening.tint, arc: ARCS.listening, full: false });
    expect(lookOf("thinking", null, false)).toMatchObject({ state: "reaching", light: MARK_STATES.reaching.tint, arc: ARCS.reaching });
    expect(lookOf("speaking", null, false)).toMatchObject({ state: "speaking", full: true, arc: null });
  });

  it("is the Start control at rest: cyan, fully lit, its arc showing", () => {
    expect(lookOf("passive", null, true)).toMatchObject({ state: "idle", glow: 1, arc: ARCS.idle });
    expect(lookOf("passive", null, false).glow).toBeLessThan(1);
  });

  it("is amber while a change waits for a yes, whatever the phase", () => {
    for (const phase of ["passive", "listening", "speaking"] as const) expect(lookOf(phase, "waiting", false).light).toBe(MARK_STATES.waiting.tint);
  });

  it("is grey and struck through when the desk isn't listening, on Home's Start control too", () => {
    for (const float of [false, true]) expect(lookOf("passive", "off", float)).toMatchObject({ state: "off", light: OFF_LIGHT, strike: true });
  });

  it("is grey without the stroke when the desk can't be reached: that isn't Not listening", () => {
    expect(lookOf("offline", null, false)).toMatchObject({ light: OFF_LIGHT, strike: false, arc: null });
  });
});
