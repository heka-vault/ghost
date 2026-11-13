import type { TurnView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { agentRecord } from "../src/screens/conversations/record";

const at = (m: number) => new Date(Date.parse("2026-11-12T15:00:00Z") + m * 60_000).toISOString();
const turn = (id: number, role: TurnView["role"], text: string, m: number, session = "s1"): TurnView => ({ id, role, text, at: at(m), session_id: session, compacted: false });

describe("an agent's conversation as the record", () => {
  it("speaks in the agent's own mark and ends each session", () => {
    const items = agentRecord([turn(1, "user", "Anything waiting?", 0), turn(2, "assistant", "One thing.", 1)], "accounting", [], [], []);
    const kinds = items.map((i) => i.kind);
    expect(kinds[0]).toBe("started");
    expect(kinds).toContain("ended");
    expect(items.find((i) => i.kind === "ghost")).toMatchObject({ slug: "accounting" });
  });
  it("turns a stored call into a work row, unless the session's activity has its calls", () => {
    const turns = [turn(1, "user", "What's open?", 0), turn(2, "call", "github_search_issues(state=open)", 0.5), turn(3, "assistant", "Three.", 1)];
    const work = agentRecord(turns, "accounting", [], [], ["github"]).filter((i) => i.kind === "work");
    expect(work).toHaveLength(1);
    expect(work[0]).toMatchObject({ calls: [{ name: "github_search_issues", app: "github", tool: "search issues" }] });
    const kept = [{ kind: "call" as const, session_id: "s1", at: at(0.5), id: "c1", name: "github_search_issues" }];
    const fromActivity = agentRecord(turns, "accounting", [], kept, ["github"]).filter((i) => i.kind === "work");
    expect(fromActivity).toHaveLength(1);
    expect(fromActivity[0].key.startsWith("wc-")).toBe(false);
  });
});
