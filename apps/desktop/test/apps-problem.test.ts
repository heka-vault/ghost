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

describe("what Ghost used an app for today", () => {
  it("counts the day's calls and the ones since, with what was read", async () => {
    const { usedWords } = await import("../src/screens/apps/model");
    expect(usedWords(null, [])).toBeNull();
    expect(usedWords({ calls: 40, changes: 1, bytes: 1_100_000 }, [{ read: true, bytes: 100_000 }, { read: false, bytes: 0 }])).toBe("used 42 times today, 2 changes, 1.2 MB read");
    expect(usedWords(null, [{ read: true, bytes: 820 }])).toBe("used once today, 820 B read");
  });
});
