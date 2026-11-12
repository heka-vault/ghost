import type { ReachCall, ReachView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { bytes, count, perMinute, reachSentence, tallyFor, tallyOf, wireOf } from "../src/screens/today/telemetry";

const call = (at: string, over: Partial<ReachCall> = {}): ReachCall => ({ at, session_id: "run-r1", agent: "ledger", app: "github", tool: "search_issues", read: true, ok: true, items: 30, bytes: 71_200, ms: 340, ...over });
const total = (over: object = {}) => ({ key: "all", agent: null, calls: 3, reads: 2, changes: 1, failed: 0, items: 412, bytes: 3_700_000, ms: 900, apps: ["ghost", "github", "linear"], first: null, last: "2026-11-12T15:00:00.000Z", ...over });
const base: ReachView = {
  since: "2026-11-12T05:00:00.000Z",
  until: "2026-11-12T16:00:00.000Z",
  total: total(),
  by_app: [],
  by_agent: [],
  by_session: [{ ...total({ calls: 2, items: 40, bytes: 1000, apps: ["github"] }), key: "run-r1", agent: "ledger" }],
};

describe("telemetry", () => {
  it("sizes and counts read as a person says them", () => {
    expect(bytes(820)).toBe("820 B");
    expect(bytes(71_200)).toBe("71.2 kB");
    expect(bytes(412_000)).toBe("412 kB");
    expect(bytes(3_700_000)).toBe("3.7 MB");
    expect(bytes(2_000_000)).toBe("2 MB");
    expect(count(1204)).toBe("1,204");
  });

  it("the day's totals, with the calls since folded in once, and Ghost's own app not counted as reached", () => {
    const before = call("2026-11-12T15:59:00.000Z"); // already in base
    const after = call("2026-11-12T16:00:05.000Z", { app: "mail", items: 8, bytes: 300_000 });
    const t = tallyOf(base, [before, after]);
    expect(t.calls).toBe(4);
    expect(t.items).toBe(420);
    expect(t.bytes).toBe(4_000_000);
    expect(t.apps).toEqual(["github", "linear", "mail"]);
    expect(reachSentence(t)).toBe("Read 420 items across 3 apps, 4 MB");
  });

  it("a run's reach is its own row and its own live calls", () => {
    const mine = call("2026-11-12T16:01:00.000Z");
    const other = call("2026-11-12T16:01:00.000Z", { session_id: "s9" });
    const t = tallyFor(base, [mine, other], "run-r1");
    expect(t.calls).toBe(3);
    expect(t.items).toBe(70);
    expect(tallyFor(null, [], "run-x").calls).toBe(0);
  });

  it("nothing yet says so; the wire is newest first; the last minute is counted", () => {
    expect(reachSentence(tallyOf(null, []))).toBe("Nothing read yet");
    const a = call("2026-11-12T16:00:00.000Z"), b = call("2026-11-12T16:00:30.000Z");
    expect(wireOf([a, b])[0]).toBe(b);
    expect(perMinute([a, b], Date.parse("2026-11-12T16:00:45.000Z"))).toBe(2);
    expect(perMinute([a, b], Date.parse("2026-11-12T16:01:10.000Z"))).toBe(1);
  });
});
