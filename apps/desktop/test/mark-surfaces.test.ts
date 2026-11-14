/**
 * Where the app draws Ghost itself (docs/design-system.md, The mark): Ghost is the lit Wisp on the
 * night tile everywhere, never Wisp flat in the accent's indigo on an indigo tint (the old mark's
 * slab), and never a bare white ghost on a light page. An agent is its own shape in its colour.
 */
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { GhostProvider, MARK_STATES, NIGHT } from "@ghost/ui";

import { Presence } from "../src/screens/session/Presence";
import { LineGlyph } from "../src/ui/Brand";

const html = (el: ReturnType<typeof createElement>) => renderToStaticMarkup(el);
/** Inside the provider Presence reads Ghost's voice from; a static render runs no effect, so no client is called. */
const live = (el: ReturnType<typeof createElement>) =>
  html(createElement(GhostProvider, { client: {} as Parameters<typeof GhostProvider>[0]["client"], children: el }));

describe("Ghost's own glyph in a list, a header and the room view", () => {
  it("is the lit tile, not the accent's hue", () => {
    for (const size of ["sm", "md", "lg", "xl"] as const) {
      const out = html(createElement(LineGlyph, { slug: "ghost", name: "Ghost", size }));
      expect(out).toContain("g-chip");
      expect(out).not.toContain("--hue-0");
    }
  });

  it("puts the session's and the room view's Wisp on the night in every state, lit in the state's colour", () => {
    for (const state of ["speaking", "you", "working", "hold", "listening", "ending", "phone", "offline"] as const) {
      const out = live(createElement(Presence, { state, slug: "ghost", size: 120 }));
      expect(out).toContain("presence--ghost");
      expect(out).toContain(`--mark-night:${NIGHT}`);
      expect(out).not.toContain("--hue-0");
    }
    expect(live(createElement(Presence, { state: "speaking", slug: "ghost", size: 64 }))).toContain(`--presence-fg:${MARK_STATES.speaking.tint}`);
    const agent = live(createElement(Presence, { state: "speaking", slug: "accounting", size: 64 }));
    expect(agent).not.toContain("presence--ghost");
    expect(agent).toContain("--presence-bg:var(--hue-");
  });

  it("leaves an agent's look alone, and the picker's overrides", () => {
    expect(html(createElement(LineGlyph, { slug: "accounting", name: "Accounting" }))).not.toContain("g-chip");
    expect(html(createElement(LineGlyph, { slug: "ghost", name: "Ghost", hue: 3 }))).not.toContain("g-chip");
  });
});
