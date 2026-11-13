import type { Activity } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { canFloat, prefOf, prefValue, type FloatingStatus } from "../src/app/floating";
import { postureOf } from "../src/app/posture";
import { elapsedText, fpsOf, isForward, labelOf, msText, sceneOf, stageX, telemetryOf, type SceneInput } from "../src/floating/scene";

const base: SceneInput = { posture: "float", capture: "no", phase: "listening", ending: false, youTalking: false, pending: false, question: false };

describe("sceneOf", () => {
  it("rests at the line unless the posture rule floats the session", () => {
    expect(sceneOf({ ...base, posture: "desk" })).toBe("line");
    expect(sceneOf({ ...base, posture: "room" })).toBe("line");
    expect(sceneOf(base)).toBe("listening");
  });

  it("hides everything, the line too, while the screen is captured", () => {
    expect(sceneOf({ ...base, capture: "yes" })).toBe("hidden");
    expect(sceneOf({ ...base, capture: "yes", posture: "desk" })).toBe("hidden");
    // A desktop that can't tell is not a capture.
    expect(sceneOf({ ...base, capture: "unknown" })).toBe("listening");
  });

  it("puts what needs the operator first: a yes, then a question, then their turn", () => {
    expect(sceneOf({ ...base, pending: true, question: true, youTalking: true, phase: "speaking" })).toBe("okay");
    expect(sceneOf({ ...base, question: true, youTalking: true })).toBe("question");
    expect(sceneOf({ ...base, youTalking: true, phase: "thinking" })).toBe("you");
  });

  it("follows the phase otherwise", () => {
    expect(sceneOf({ ...base, phase: "thinking" })).toBe("thinking");
    expect(sceneOf({ ...base, phase: "speaking" })).toBe("speaking");
    expect(sceneOf({ ...base, phase: "asking" })).toBe("listening");
    expect(sceneOf({ ...base, phase: "hold" })).toBe("hold");
    expect(sceneOf({ ...base, phase: "speaking", ending: true })).toBe("ending");
  });

  it("is fed by the one posture rule", () => {
    const deskSession = { hold: "auto" as const, deskSession: true, idleMs: 0, awayAfterMs: null, sessionIdleMs: null };
    const p = (windowActive: boolean, floating: boolean) => postureOf({ ...deskSession, windowActive, floating }).posture;
    expect(sceneOf({ ...base, posture: p(false, true) })).toBe("listening");
    // The main window is the one in use: the session stays there.
    expect(sceneOf({ ...base, posture: p(true, true) })).toBe("line");
    expect(sceneOf({ ...base, posture: p(false, false) })).toBe("line");
  });
});

describe("cost and words", () => {
  it("runs no frames at rest, 60 for the arrival, 30 otherwise", () => {
    expect(fpsOf("line", true)).toBe(0);
    expect(fpsOf("hidden", false)).toBe(0);
    expect(fpsOf("hold", false)).toBe(0);
    expect(fpsOf("listening", true)).toBe(60);
    expect(fpsOf("speaking", false)).toBe(30);
    expect(isForward("okay")).toBe(true);
  });

  it("labels the scene in plain words", () => {
    const o = { agent: "Ghost", using: null, asker: null, micOff: false };
    expect(labelOf("you", o)).toBe("Listening to you");
    expect(labelOf("thinking", { ...o, using: "github" })).toBe("Using github");
    expect(labelOf("question", { ...o, asker: "Accounting" })).toBe("Accounting asks");
    expect(labelOf("listening", { ...o, micOff: true })).toBe("Mic off · type in Ghost");
    expect(labelOf("line", o)).toBe("");
  });

  it("places the stage away from the work on one monitor", () => {
    expect(stageX("centre")).toBe(0.5);
    expect(stageX("left")).toBeLessThan(0.5);
    expect(stageX("right")).toBeGreaterThan(0.5);
    expect(stageX(null)).toBe(0.5);
  });
});

describe("telemetryOf", () => {
  const at = (s: number) => new Date(Date.UTC(2026, 10, 12, 15, 0, s)).toISOString();
  const a = (kind: Activity["kind"], s: number, extra: Partial<Activity> = {}): Activity => ({ kind, at: at(s), session_id: "s", names: [], ...extra }) as Activity;

  it("is the session's calls, timed from its activity, with the apps they reached", () => {
    const t = telemetryOf(
      [
        a("you_started", 0),
        a("call", 1, { id: "c1", name: "github_issue_list" }),
        a("call", 2, { id: "c2", name: "calendar_events_list" }),
        a("result", 3, { id: "c1", name: "github_issue_list" }),
        a("handover", 3),
        a("call", 4, { id: "c3", name: "unknown_tool" }),
        a("cut", 5),
      ],
      ["github", "calendar"],
    );
    expect(t.calls).toBe(3);
    expect(t.open).toBe(2);
    expect(t.apps).toEqual(["github", "calendar"]);
    expect(t.rows.map((r) => [r.name, r.ms])).toEqual([
      ["github.issue_list", 2000],
      ["calendar.events_list", null],
      ["unknown_tool", null],
    ]);
    expect([t.handovers, t.cuts]).toEqual([1, 1]);
  });

  it("keeps the last few rows but counts them all", () => {
    const list = Array.from({ length: 12 }, (_, i) => a("call", i, { id: `c${i}`, name: "github_issue_read" }));
    const t = telemetryOf(list, ["github"], 8);
    expect(t.rows).toHaveLength(8);
    expect(t.rows[7].id).toBe("c11");
    expect(t.calls).toBe(12);
  });

  it("says times the way the margin reads them", () => {
    expect(msText(null)).toBe("…");
    expect(msText(412)).toBe("412 ms");
    expect(msText(1440)).toBe("1.4 s");
    expect(msText(12_400)).toBe("12 s");
    expect(elapsedText(at(0), Date.parse(at(65)))).toBe("1:05");
    expect(elapsedText(null, 0)).toBeNull();
  });
});

describe("the floating room's switch in the main window", () => {
  const status = (over: Partial<FloatingStatus> = {}): FloatingStatus => ({
    support: { kind: "xwayland" },
    settings: { on: true, monitor: "other" },
    running: true,
    capture: "no",
    monitors: [],
    helper: false,
    ...over,
  });
  it("floats only when on, up and not captured", () => {
    expect(canFloat(status())).toBe(true);
    expect(canFloat(status({ capture: "unknown" }))).toBe(true);
    expect(canFloat(status({ capture: "yes" }))).toBe(false);
    expect(canFloat(status({ running: false }))).toBe(false);
    expect(canFloat(status({ settings: { on: false, monitor: "other" } }))).toBe(false);
    expect(canFloat(null)).toBe(false);
  });
  it("reads and writes the monitor choice as the shell stores it", () => {
    expect(prefValue({ named: "HDMI-3" })).toBe("named:HDMI-3");
    expect(prefOf("named:HDMI-3")).toEqual({ named: "HDMI-3" });
    expect(prefOf("here")).toBe("here");
    expect(prefOf("other")).toBe("other");
  });
});
