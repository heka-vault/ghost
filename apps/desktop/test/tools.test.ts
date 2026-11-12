import { describe, expect, it } from "vitest";

import { NAME_MAX, nameProblem, toolName, toolTitle } from "../src/screens/settings/tools";

describe("an outside tool's name (ADR 0048)", () => {
  it("is what the operator typed, as lowercase words joined by hyphens", () => {
    expect(toolName("Claude Code")).toBe("claude-code");
    expect(toolName("  cursor ")).toBe("cursor");
    expect(toolName("Codex (work)")).toBe("codex-work");
    expect(toolName("---")).toBe("");
    expect(toolName("x".repeat(80))).toHaveLength(NAME_MAX);
    expect(toolName(`${"x".repeat(NAME_MAX - 1)} y`)).toBe("x".repeat(NAME_MAX - 1));
  });

  it("is said back as words", () => {
    expect(toolTitle("claude-code")).toBe("Claude Code");
    expect(toolTitle("cursor")).toBe("Cursor");
  });

  it("is refused in a sentence when it won't do", () => {
    expect(nameProblem("", [])).toBeNull();
    expect(nameProblem("Claude Code", [])).toBeNull();
    expect(nameProblem("!!!", [])).toMatch(/letters or digits/);
    expect(nameProblem("Claude Code", ["claude-code"])).toMatch(/Claude Code is already connected/);
  });
});
