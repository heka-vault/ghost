import type { Turn } from "@ghost/client-ts";
import type { Activity } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { activityItems, cutReplies, gapText, lengthText, weave, withGaps } from "../src/screens/session/timeline";
import { buildHistory, buildTranscript, runMark } from "../src/screens/session/transcript";

const at = (s: number) => new Date(Date.UTC(2026, 10, 12, 10, 0, s)).toISOString();
const turn = (role: Turn["role"], text: string, s: number, via: Turn["via"] = "voice", session = "s1"): Turn => ({ role, text, at: at(s), session_id: session, via });
const act = (kind: Activity["kind"], s: number, extra: Partial<Activity> = {}): Activity => ({ kind, session_id: "s1", at: at(s), ...extra });
const kinds = (items: { kind: string }[]) => items.map((i) => i.kind);

describe("the session as one timeline", () => {
  const turns = [turn("user", "What's waiting?", 1), turn("assistant", "Let me look.", 3), turn("assistant", "Four issues need you.", 8), turn("user", "Close 151.", 12)];

  it("puts the calls between two of Ghost's sentences between them, as one row", () => {
    const extras = activityItems(
      [act("you_started", 0), act("you_stopped", 2), act("call", 4, { name: "github_issue_list", id: "a" }), act("call", 4, { name: "github_issue_read", id: "b" }), act("result", 5, { id: "a" }), act("result", 6, { id: "b" })],
      turns,
      ["github"],
    );
    expect(extras).toHaveLength(1);
    const items = weave(buildTranscript(turns, "ghost", [], []), extras);
    expect(kinds(items)).toEqual(["started", "you", "ghost", "work", "ghost", "you"]);
    const work = items[3];
    expect(work.kind === "work" && work.calls.map((c) => [c.tool, c.endedAt != null])).toEqual([
      ["issue list", true],
      ["issue read", true],
    ]);
  });

  it("keeps a handover with the calls it came with", () => {
    const extras = activityItems([act("handover", 4, { names: ["github_issue_write"], secs: 0.4 }), act("call", 5, { name: "github_issue_write", id: "c" })], turns, ["github"]);
    expect(extras).toHaveLength(1);
    expect(extras[0].kind === "work" && extras[0].handover).toEqual({ secs: 0.4, names: ["github_issue_write"] });
  });

  it("marks the reply a cut came after, and only that one", () => {
    const items = weave(buildTranscript(turns, "ghost", [], []), activityItems([act("cut", 10)], turns, []));
    expect(kinds(items)).toEqual(["started", "you", "ghost", "cut", "you"]);
    expect([...cutReplies(items)]).toEqual([items[2].key]);
  });

  it("says when two minutes or more passed with nothing said", () => {
    const later = [turn("user", "Hi.", 0), turn("assistant", "Hi.", 2), turn("user", "Anything new?", 200)];
    const items = withGaps(buildTranscript(later, "ghost", [], []));
    expect(kinds(items)).toEqual(["started", "you", "ghost", "gap", "you"]);
    expect(gapText(198)).toBe("3 min later");
    expect(gapText(3900)).toBe("1 h 5 min later");
  });
});

describe("the history", () => {
  it("ends each session with how long it ran", () => {
    const items = buildHistory([turn("user", "Hi.", 0, "voice", "a"), turn("assistant", "Hi.", 50, "voice", "a"), turn("user", "Again.", 59, "voice", "b")], []);
    expect(kinds(items)).toEqual(["started", "you", "ghost", "ended", "started", "you", "ended"]);
    expect(lengthText(50)).toBe("under a minute");
    expect(lengthText(3720)).toBe("1 h 2 min");
  });

  it("shows a scheduled run's and an asked turn's entries as runs, without the mark", () => {
    expect(runMark("[scheduled run: morning brief] Two new issues.")).toEqual({ label: "Scheduled run · morning brief", rest: "Two new issues." });
    expect(runMark("[asked by claude-code, an outside tool] What's open?")).toEqual({ label: "Asked by claude-code", rest: "What's open?" });
    expect(runMark("An ordinary reply.")).toBeNull();
    const items = buildHistory([turn("system", "[scheduled run: morning brief] Check the app repo.", 0, "voice", "run-1"), turn("assistant", "[scheduled run: morning brief] Two new issues.", 9, "voice", "run-1")], []);
    expect(kinds(items)).toEqual(["started", "run", "ended"]);
  });
});
