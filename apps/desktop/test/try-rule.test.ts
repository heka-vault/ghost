import { describe, expect, it } from "vitest";

import { matches, parseArgs, triedWords, tryCall } from "../src/screens/policy/tryRule";

const perms = { mode: "open" as const, block: ["github.delete_file"], ask: ["github.issue_write(state=closed)", "github.*"], quiet: [], allow: ["github.issue_write"] };

describe("trying a call against the permissions", () => {
  it("reads arguments the way rules write them", () => {
    expect(parseArgs("state=closed, repo='app'")).toEqual({ state: "closed", repo: "app" });
  });
  it("matches app and tool globs and argument conditions", () => {
    expect(matches("github.*", "github", "list_issues", {})).toBe(true);
    expect(matches("git*", "github", "x", {})).toBe(true);
    expect(matches("github.issue_write(state=closed)", "github", "issue_write", { state: "open" })).toBe(false);
    expect(matches("github.issue_write(state!=closed)", "github", "issue_write", { state: "open" })).toBe(true);
  });
  it("lets the most careful matching rule win, and names what it beat", () => {
    const t = tryCall(perms, "github", "issue_write", false, { state: "closed" });
    expect(t.level).toBe("ask");
    expect(t.rule).toBe("github.issue_write(state=closed)");
    expect(t.beaten.map((b) => b.list)).toEqual(["ask", "allow"]);
    expect(tryCall(perms, "github", "delete_file", false).level).toBe("block");
  });
  it("falls back to reads running, Ghost's own quiet default and the mode", () => {
    const none = { ...perms, block: [], ask: [], allow: [] };
    expect(tryCall(none, "cal", "list_events", true)).toMatchObject({ level: "allow", why: "read" });
    expect(tryCall(none, "ghost", "notify", false)).toMatchObject({ level: "quiet", why: "own" });
    expect(tryCall({ ...none, mode: "ask" }, "cal", "create_event", false)).toMatchObject({ level: "ask", why: "mode" });
    expect(tryCall({ ...none, mode: "read-only" }, "cal", "create_event", false)).toMatchObject({ level: "block", why: "mode" });
    expect(tryCall({ ...none, mode: "read-only", block: ["cal"] }, "cal", "list_events", true).level).toBe("block");
  });
  it("answers in one sentence", () => {
    const t = tryCall(perms, "github", "delete_file", false);
    expect(triedWords(t, () => "Delete file in GitHub", false)).toBe("Blocked: your rule “Delete file in GitHub” decides it.");
  });
});
