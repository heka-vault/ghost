import type { ConversationView, ScheduleRunInfo, WaitingItemView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { railAgents } from "../src/app/railAgents";

const agent = (slug: string, last: string | null, over: Partial<ConversationView> = {}): ConversationView =>
  ({ id: slug, slug, name: slug[0].toUpperCase() + slug.slice(1), aliases: [], job: "", voice: "af_heart", speed: 1, own_voice: false, created_at: "2026-11-01T00:00:00Z", last_active_at: last, retired_at: null, active: false, is_ghost: slug === "ghost", look: null, ...over }) as ConversationView;
const item = (source: string): WaitingItemView => ({ key: source, source, title: "t", state: "new", first_seen: "", changed_at: "", age_secs: 0 }) as WaitingItemView;
const run = (agent: string): ScheduleRunInfo => ({ id: "r", agent, status: "running" }) as ScheduleRunInfo;

describe("the rail's agents", () => {
  it("lists the agents themselves, not Ghost and not the retired, the busiest first", () => {
    const rows = railAgents({
      agents: [agent("ghost", null), agent("a", "2026-11-12T10:00:00Z"), agent("b", "2026-11-12T12:00:00Z"), agent("c", null), agent("d", null), agent("old", null, { retired_at: "2026-11-01T00:00:00Z" })],
      working: [run("c")],
      needs: [item("agent:d"), item("agent:d:schedule:x")],
      session: "a",
      phase: "speaking",
    });
    expect(rows.map((r) => r.slug)).toEqual(["a", "d", "c", "b"]);
    expect(rows[0]).toMatchObject({ state: "speaking", stateWords: "speaking" });
    expect(rows[1]).toMatchObject({ state: "waiting", waiting: 2, stateWords: "waiting on you (2)" });
    expect(rows[2]).toMatchObject({ state: "working", stateWords: "working" });
    expect(rows[3].state).toBeNull();
  });
});
