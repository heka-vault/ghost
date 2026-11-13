import { describe, expect, it } from "vitest";

import { environmentNote } from "../src/screens/settings/environment";

describe("a connection from the environment Ghost was started in (ADR 0037)", () => {
  it("says nothing when every setting is Ghost's own", () => {
    expect(environmentNote({ from_environment: [], password: "ANTHROPIC_API_KEY" })).toBeNull();
  });

  it("names the settings and the password, and how to keep each", () => {
    const n = environmentNote({ from_environment: ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"], password: "ANTHROPIC_AUTH_TOKEN" })!;
    expect(n.title).toBe("From the environment Ghost was started in");
    expect(n.settings).toEqual(["ANTHROPIC_BASE_URL"]);
    expect(n.password).toBe("ANTHROPIC_AUTH_TOKEN");
    expect(n.text).toContain("ANTHROPIC_BASE_URL and ANTHROPIC_AUTH_TOKEN");
    expect(n.text).toContain("until Ghost is started another way");
    expect(n.keepSettings).toContain("settings.env");
    expect(n.keepPassword).toContain("vault");
  });

  it("only a password: nothing to save on the page", () => {
    const n = environmentNote({ from_environment: ["OPENAI_API_KEY"], password: "OPENAI_API_KEY" })!;
    expect(n.settings).toEqual([]);
    expect(n.keepSettings).toBeNull();
    expect(n.keepPassword).toContain("OPENAI_API_KEY");
  });

  it("only settings: no password line", () => {
    const n = environmentNote({ from_environment: ["GHOST_LLM_PROVIDER"], password: "OPENAI_API_KEY" })!;
    expect(n.password).toBeNull();
    expect(n.keepPassword).toBeNull();
  });
});
