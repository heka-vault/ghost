import { describe, expect, it } from "vitest";

import { choiceOf, draftConnection, namedProvider } from "../src/screens/settings/provider";

// A settings.env from before the named proxy provider was removed (ADR 0037, amended
// 2026-11-14): the daemon reads it as the custom connection it always was and reports that.
const saved = { GHOST_LLM_PROVIDER: "cliproxy", GHOST_LLM_MODEL: "claude-opus-5-5" };
const reported = {
  GHOST_LLM_PROVIDER: "custom",
  GHOST_LLM_PROTOCOL: "anthropic",
  GHOST_LLM_BASE_URL: "http://proxy.lan:8317",
  GHOST_LLM_PASSWORD: "ANTHROPIC_AUTH_TOKEN",
  GHOST_LLM_MODEL: "claude-opus-5-5",
};

describe("a provider Ghost no longer offers, still in settings.env", () => {
  it("is not a provider the page takes as named", () => {
    expect(namedProvider(saved)).toBe("");
    expect(namedProvider({ GHOST_LLM_PROVIDER: " CLIProxy " })).toBe("");
    expect(namedProvider({ GHOST_LLM_PROVIDER: "custom" })).toBe("custom");
    expect(namedProvider({})).toBe("");
  });

  it("shows as Your own address, with the daemon's values", () => {
    const { explicit, values } = draftConnection(saved, reported);
    expect(explicit).toBe(false);
    expect(values).toEqual(reported);
    expect(choiceOf(values.GHOST_LLM_PROVIDER)).toBe("custom");
  });

  it("is written out as custom on the first edit, never as itself", () => {
    const { values } = draftConnection({ ...saved, GHOST_LLM_MODEL: "claude-sonnet-5-5" }, reported);
    expect(values.GHOST_LLM_PROVIDER).toBe("custom");
    expect(values.GHOST_LLM_MODEL).toBe("claude-sonnet-5-5");
    expect(JSON.stringify(values).toLowerCase()).not.toContain("cliproxy");
  });

  it("leaves a named provider's draft as it is", () => {
    const named = { GHOST_LLM_PROVIDER: "openai", GHOST_LLM_MODEL: "gpt-5" };
    expect(draftConnection(named, reported)).toEqual({ explicit: true, values: named });
  });
});
