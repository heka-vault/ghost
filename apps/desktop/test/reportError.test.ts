import { describe as suite, expect, it } from "vitest";

import { MAX_MESSAGE, MAX_STACK, describe } from "../src/app/reportError";

suite("a page's error handed to the shell (ADR 0067)", () => {
  it("is the error's type, message and stack only", () => {
    const e = new TypeError("x is undefined");
    const d = describe("main", "error", e);
    expect(Object.keys(d).sort()).toEqual(["message", "page", "source", "stack", "type"]);
    expect(d.type).toBe("TypeError");
    expect(d.message).toBe("x is undefined");
  });

  it("is cut to what the shell takes", () => {
    const e = new Error("m".repeat(MAX_MESSAGE + 50));
    e.stack = "s".repeat(MAX_STACK + 50);
    const d = describe("floating", "boundary", e);
    expect(d.message.length).toBe(MAX_MESSAGE);
    expect(d.stack.length).toBe(MAX_STACK);
  });

  it("keeps only the type of a value that is not an Error", () => {
    const d = describe("main", "rejection", "github said: Bad credentials for someone@example.com");
    expect(d).toEqual({ page: "main", source: "rejection", type: "string", message: "", stack: "" });
    expect(describe("main", "rejection", { detail: "an app's words" }).message).toBe("");
  });

  it("never passes an odd name as the type", () => {
    const e = new Error("x");
    e.name = "not a <name>";
    expect(describe("main", "error", e).type).toBe("Error");
  });
});
