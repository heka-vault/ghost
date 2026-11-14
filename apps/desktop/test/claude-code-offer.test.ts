import { describe, expect, it } from "vitest";

import { CLAUDE_CODE, claudeCodeOffer, claudeCodeValues } from "../src/screens/settings/claudeCode";

const view = (state: "plan" | "api_key" | "signed_out" | "unsupported", line = "") => ({ state, version: "2.1.325", line, note: null });

describe("the operator's own Claude Code in the model step (ADR 0069)", () => {
  it("is not offered at all when it isn't installed", () => {
    expect(claudeCodeOffer(null)).toBeNull();
    expect(claudeCodeOffer(undefined)).toBeNull();
  });

  it("leads, signed in with a plan, saying the plan's limits are what it costs", () => {
    const o = claudeCodeOffer(view("plan"));
    expect(o?.kind).toBe("use");
    expect(`${o?.title}. ${o?.body}`).toBe(
      "Claude Code is signed in on this computer. Ghost can run it to answer you. Replies count against your Claude plan’s limits, the same ones your coding uses.",
    );
  });

  it("says a key is billed when it is signed in with one", () => {
    const o = claudeCodeOffer(view("api_key"));
    expect(o?.kind).toBe("use");
    expect(o?.body).toContain("billed to that key");
  });

  it("points a signed-out program at its own sign-in, in a terminal", () => {
    const line = "Claude Code is installed but not signed in. Run `claude auth login` in a terminal, then Look again.";
    const o = claudeCodeOffer(view("signed_out", line));
    expect(o).toEqual({ kind: "fix", title: "Claude Code isn’t signed in", body: line, note: null });
  });

  it("never offers a sign-in of Ghost's, an endorsement, or something free", () => {
    for (const s of ["plan", "api_key", "signed_out", "unsupported"] as const) {
      const o = claudeCodeOffer(view(s, "x"));
      const said = `${o?.title} ${o?.body}`.toLowerCase();
      for (const word of ["sign in with claude", "free", "unlimited", "endorse", "official", "partner", "by anthropic"]) {
        expect(said).not.toContain(word);
      }
    }
  });

  it("chooses it by provider alone, every other connection setting cleared", () => {
    const keys = ["GHOST_LLM_PROVIDER", "GHOST_LLM_MODEL", "GHOST_LLM_BASE_URL", "GHOST_LLM_PASSWORD"];
    expect(claudeCodeValues(keys)).toEqual({ GHOST_LLM_PROVIDER: CLAUDE_CODE, GHOST_LLM_MODEL: null, GHOST_LLM_BASE_URL: null, GHOST_LLM_PASSWORD: null });
  });
});
