import type { AppAction, AppView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { stateWords } from "../src/screens/apps/model";
import { abilityOf, callsOf, latelyWords, limitsWords, toolList, twinsOf } from "../src/screens/apps/plain";

const act = (name: string, reads: boolean, level: AppAction["level"] = "allow"): AppAction => ({ name, reads, level, rule: name, description: "", conditional: false });
const app = (name: string, catalog: string | null, over: Partial<AppView> = {}): AppView =>
  ({ name, title: name, catalog, kind: "sign_in", state: "connected", actions: [], settings: [], limits: { timeout: null, connect_timeout: null, result_chars: null, default_timeout: 300, default_connect_timeout: 60, default_result_chars: 24000, pinned: [] }, ...over }) as AppView;

describe("an app in plain words", () => {
  it("says what it lets Ghost do, and which ask or are blocked", () => {
    const a = abilityOf([act("search_issues", true), act("list_issues", true), act("close_issue", false, "ask"), act("delete_file", false, "block")]);
    expect(a.readLine).toBe("Looks up: search issues and list issues.");
    expect(a.changeLine).toBe("Changes: close issue and delete file.");
    expect(a.ruleLine).toBe("1 is blocked, 1 asks you first.");
    expect(toolList([act("a", true), act("b", true), act("c", true), act("d", true), act("e", true), act("f", true)])).toBe("a, b, c, d and 2 more");
  });
  it("says its limits, defaults included", () => {
    expect(limitsWords(app("x", null).limits)).toBe("A call may take 5 min and connecting 1 min. Ghost reads up to 24,000 characters of each answer; past that it’s cut and Ghost is told how much.");
    expect(limitsWords({ ...app("x", null).limits, timeout: 45, pinned: ["a", "b"] })).toMatch(/^A call may take 45 s .* 2 tools always offered by name\.$/);
  });
  it("finds two connections to the same service, and nothing else", () => {
    const t = twinsOf([app("github", "gh"), app("github_acme", "gh"), app("mobbin", null), app("cal", "cal")]);
    expect(t.get("github")?.map((x) => x.name)).toEqual(["github_acme"]);
    expect(t.get("github_acme")?.map((x) => x.name)).toEqual(["github"]);
    expect(t.has("mobbin")).toBe(false);
    expect(t.has("cal")).toBe(false);
  });
  it("says what Ghost used it for this week", () => {
    const row = { key: "github", apps: ["github"], bytes: 1_200_000, calls: 42, changes: 2, failed: 0, items: 80, ms: 0, reads: 40 };
    expect(latelyWords(row, ["Ghost", "Legal"], () => "1.2 MB")).toBe("Used 42 times this week by Ghost and Legal: 2 changes, 1.2 MB read.");
    expect(latelyWords(undefined, [], () => "")).toBeNull();
    const calls = [{ app: "github" }, { app: "cal" }, { app: "github" }] as never[];
    expect(callsOf(calls, "github")).toHaveLength(2);
  });
  it("never says a sign-in in amber", () => {
    expect(stateWords(app("l", null, { state: "needs_sign_in" })).tone).toBe("danger");
    expect(stateWords(app("l", null, { sign_in_url: "https://x" })).tone).toBe("info");
  });
});
