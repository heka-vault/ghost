import type { ConversationView, ScheduleRunInfo, WaitingItemView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { RAIL_MAX, fitOf, holdStill, railAgents, railShown } from "../src/app/railAgents";

const agent = (slug: string, last: string | null, hired: string, over: Partial<ConversationView> = {}): ConversationView =>
  ({ id: slug, slug, name: slug[0].toUpperCase() + slug.slice(1), aliases: [], job: "", voice: "af_heart", speed: 1, own_voice: false, created_at: hired, last_active_at: last, retired_at: null, active: false, is_ghost: slug === "ghost", look: null, ...over }) as ConversationView;
const item = (source: string): WaitingItemView => ({ key: source, source, title: "t", state: "new", first_seen: "", changed_at: "", age_secs: 0 }) as WaitingItemView;
const run = (agent: string): ScheduleRunInfo => ({ id: "r", agent, status: "running" }) as ScheduleRunInfo;
const day = (d: number) => `2026-11-${String(d).padStart(2, "0")}T00:00:00Z`;

/** Fifteen hired agents, a to o, hired a day apart; the later ones used more recently. */
const fifteen = () => [agent("ghost", null, day(1)), ...Array.from({ length: 15 }, (_, i) => agent(String.fromCharCode(97 + i), i % 3 === 0 ? null : `2026-11-13T${String(i).padStart(2, "0")}:00:00Z`, day(1 + i)))];

describe("the rail's agents", () => {
  it("lists the agents themselves, not Ghost and not the retired, in the order they were hired", () => {
    const rows = railAgents({
      agents: [agent("ghost", null, day(1)), agent("b", "2026-11-12T12:00:00Z", day(3)), agent("a", "2026-11-12T10:00:00Z", day(2)), agent("c", null, day(4)), agent("d", null, day(5)), agent("old", null, day(1), { retired_at: day(6) })],
      working: [run("c")],
      needs: [item("agent:d"), item("agent:d:schedule:x")],
      session: "a",
      phase: "speaking",
    });
    expect(rows.map((r) => r.slug)).toEqual(["a", "b", "c", "d"]);
    expect(rows[0]).toMatchObject({ state: "speaking", stateWords: "speaking" });
    expect(rows[1].state).toBeNull();
    expect(rows[2]).toMatchObject({ state: "working", stateWords: "working" });
    expect(rows[3]).toMatchObject({ state: "waiting", waiting: 2, stateWords: "waiting on you (2)" });
  });
});

describe("the cap", () => {
  const live = (over: { working?: string[]; needs?: string[]; session?: string | null } = {}) =>
    railAgents({ agents: fifteen(), working: (over.working ?? []).map(run), needs: (over.needs ?? []).map((s) => item(`agent:${s}`)), session: over.session ?? null, phase: "listening" });

  it("shows the most recently used up to the room, in hire order, and counts the rest", () => {
    const { shown, hidden } = railShown(live(), 4);
    // Used most recently: o (14:00), n (13:00), l (11:00), k (10:00); shown as hired.
    expect(shown.map((a) => a.slug)).toEqual(["k", "l", "n", "o"]);
    expect(hidden).toBe(11);
  });

  it("never shows more than RAIL_MAX, however tall the window", () => {
    expect(railShown(live(), 40).shown).toHaveLength(RAIL_MAX);
    expect(RAIL_MAX).toBe(6);
  });

  it("always shows an agent in the session, waiting on you or working, even past the room", () => {
    const { shown } = railShown(live({ session: "a", needs: ["d"], working: ["g"] }), 2);
    expect(shown.map((a) => a.slug)).toEqual(["a", "d", "g"]);
    const none = railShown(live({ working: ["b"] }), 0).shown;
    expect(none.map((a) => a.slug)).toEqual(["b"]);
  });

  it("with two agents shows both and hides none", () => {
    const two = railAgents({ agents: [agent("ghost", null, day(1)), agent("a", null, day(2)), agent("b", null, day(3))], working: [], needs: [], session: null, phase: "passive" });
    expect(railShown(two, 4)).toMatchObject({ hidden: 0 });
    expect(railShown(two, 4).shown.map((a) => a.slug)).toEqual(["a", "b"]);
  });

  it("keeps an agent's place when its state changes: its mark shows it, it doesn't move", () => {
    const quiet = railShown(live({ working: ["k"] }), 4).shown.map((a) => a.slug);
    const busy = railShown(live({ working: ["k"], needs: ["k"] }), 4).shown.map((a) => a.slug);
    expect(busy).toEqual(quiet);
  });
});

describe("holding still under the pointer", () => {
  it("puts a newcomer at the end and keeps the rest where they were", () => {
    expect(holdStill(["h", "k", "m"], ["b", "h", "k", "m"], true)).toEqual(["h", "k", "m", "b"]);
  });
  it("keeps one that would leave until the pointer goes", () => {
    expect(holdStill(["h", "k", "m"], ["h", "m"], true)).toEqual(["h", "k", "m"]);
    expect(holdStill(["h", "k", "m"], ["h", "m"], false)).toEqual(["h", "m"]);
  });
});

describe("the room the rail has", () => {
  it("fits whole rows only, none when there is no room, RAIL_MAX at the most", () => {
    // The rail at 1040×720, measured: 435px of nav, 251 for everything but the agents, 42px a row.
    expect(fitOf({ navHeight: 435, restHeight: 251, rowPitch: 42 })).toBe(4);
    expect(fitOf({ navHeight: 195, restHeight: 251, rowPitch: 42 })).toBe(0);
    expect(fitOf({ navHeight: 2000, restHeight: 251, rowPitch: 42 })).toBe(RAIL_MAX);
  });
});
