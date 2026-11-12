import { describe, expect, it } from "vitest";

import { problemWords } from "../src/screens/apps/model";

describe("problemWords", () => {
  it("says why in plain words, never the app's own message", () => {
    expect(problemWords("notes-mcp: command not found (looked on PATH and in ~/.local/bin)")).toBe("Its command isn’t installed");
    expect(problemWords("ConnectError: can't reach http://localhost:9")).toBe("Ghost can’t reach it");
    expect(problemWords("HTTP 401 Unauthorized")).toBe("It turned down Ghost’s sign-in");
    expect(problemWords("connect timed out after 60 s")).toBe("It didn’t answer in time");
    expect(problemWords("something else entirely")).toBe("It couldn’t start");
    expect(problemWords(null)).toBe("Not connected");
  });
});
