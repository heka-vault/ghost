import { describe, expect, it } from "vitest";

import { type Activity, addActivity, appOf, callsOf, createVoicedClock, saidChars, splitSaid, youSpeaking } from "@ghost/ui";

const at = (s: number) => `2026-11-12T10:00:${String(s).padStart(2, "0")}Z`;
const act = (kind: Activity["kind"], s: number, extra: Partial<Activity> = {}): Activity => ({ kind, session_id: "s1", at: at(s), ...extra });

describe("the session's activity (ADR 0055)", () => {
  it("keeps one session's list and starts over with the next session", () => {
    let list: Activity[] = [];
    list = addActivity(list, act("you_started", 1));
    list = addActivity(list, act("you_stopped", 2));
    expect(list).toHaveLength(2);
    expect(addActivity(list, act("you_stopped", 2))).toBe(list); // sent again on a new connection
    list = addActivity(list, { ...act("you_started", 3), session_id: "s2" });
    expect(list.map((a) => a.session_id)).toEqual(["s2"]);
  });

  it("knows when the operator is speaking, without any words", () => {
    expect(youSpeaking([])).toBe(false);
    expect(youSpeaking([act("you_started", 1)])).toBe(true);
    expect(youSpeaking([act("you_started", 1), act("call", 2, { name: "x", id: "1" })])).toBe(true);
    expect(youSpeaking([act("you_started", 1), act("you_stopped", 2)])).toBe(false);
  });

  it("names a tool's app by the longest connected app it starts with", () => {
    expect(appOf("github_issue_read", ["github", "git"])).toBe("github");
    expect(appOf("github_heka_issue_read", ["github", "github_heka"])).toBe("github_heka");
    expect(appOf("ghost_recall", [])).toBe("ghost");
    expect(appOf("lonely", ["github"])).toBeNull();
  });

  it("pairs each call with its result and keeps the ones still running open", () => {
    const calls = callsOf(
      [
        act("call", 1, { name: "github_issue_read", id: "a" }),
        act("call", 1, { name: "github_issue_comments", id: "b" }),
        act("result", 2, { name: "github_issue_read", id: "a" }),
      ],
      ["github"],
    );
    expect(calls.map((c) => [c.app, c.tool, c.endedAt])).toEqual([
      ["github", "issue read", at(2)],
      ["github", "issue comments", null],
    ]);
  });
});

describe("how much of Ghost's reply has been said", () => {
  const text = "Four issues need your sign-off in the app repo.";

  it("says nothing before the voice, and everything once it stops on its own", () => {
    expect(saidChars(text, 0)).toBe(0);
    expect(saidChars(text, 0.4, 1, false)).toBe(text.length);
  });

  it("moves by whole words", () => {
    const n = saidChars(text, 0.5); // 8 characters at 16 a second: "Four iss" → "Four"
    expect(splitSaid(text, n)).toEqual({ said: "Four", rest: " issues need your sign-off in the app repo." });
  });

  it("never claims the last word while the voice is still going", () => {
    const n = saidChars(text, 30);
    expect(text.slice(n).trim()).toBe("repo.");
  });

  it("runs faster for a faster voice", () => {
    expect(saidChars(text, 1, 1.5)).toBeGreaterThan(saidChars(text, 1, 1));
  });

  it("counts only windows where Ghost's voice sounds", () => {
    const clock = createVoicedClock();
    for (const l of [0.5, 0.6, 0, 0, 0.4]) clock.level(l);
    expect(clock.secs).toBeCloseTo(0.15);
    clock.reset();
    expect(clock.secs).toBe(0);
  });
});
