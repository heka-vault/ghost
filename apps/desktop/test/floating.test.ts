import { describe, expect, it } from "vitest";

import { canFloat, prefOf, prefValue, type FloatingStatus } from "../src/app/floating";
import { postureOf } from "../src/app/posture";
import { captionsShown, captionText, fpsOf, goneAfterGoodbye, GOODBYE_MS, isForward, labelOf, sceneOf, type SceneInput } from "../src/floating/scene";

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

  it("says speaking while Ghost's voice plays, even with a turn the VAD opened (the live screenshot)", () => {
    // 2026-11-12: "Listening to you" over a reply. The daemon interrupts Ghost when a turn
    // really starts, so a speaking phase with a turn open is Ghost still at the speaker.
    expect(sceneOf({ ...base, youTalking: true, phase: "speaking" })).toBe("speaking");
    expect(sceneOf({ ...base, youTalking: true, phase: "listening" })).toBe("you");
  });

  it("is working while calls run, whatever the phase says (the phase follows the audio)", () => {
    // "One sec." over the calls: phase listening once it is said, calls still out.
    expect(sceneOf({ ...base, phase: "listening", openCalls: 2 })).toBe("thinking");
    expect(sceneOf({ ...base, phase: "listening", openCalls: 0 })).toBe("listening");
    // Ghost's voice and the operator's turn still win: the beams are drawn in those scenes too.
    expect(sceneOf({ ...base, phase: "speaking", openCalls: 2 })).toBe("speaking");
    expect(sceneOf({ ...base, youTalking: true, openCalls: 1 })).toBe("you");
  });

  it("does not linger after the goodbye", () => {
    expect(sceneOf({ ...base, ending: true })).toBe("ending");
    expect(sceneOf({ ...base, ending: true, gone: true })).toBe("line");
    expect(goneAfterGoodbye({ endingFor: null, quietFor: 9_000, heard: true })).toBe(false);
    // Said and quiet a moment: gone.
    expect(goneAfterGoodbye({ endingFor: 1_500, quietFor: 200, heard: true })).toBe(false);
    expect(goneAfterGoodbye({ endingFor: 1_500, quietFor: 800, heard: true })).toBe(true);
    // Never heard (the voice level never came): its time is the cap.
    expect(goneAfterGoodbye({ endingFor: 1_500, quietFor: 1_500, heard: false })).toBe(false);
    expect(goneAfterGoodbye({ endingFor: GOODBYE_MS, quietFor: 0, heard: true })).toBe(true);
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

  it("labels the scene in a few plain words", () => {
    const o = { agent: "Ghost", using: null, asker: null, micOff: false };
    expect(labelOf("you", o)).toBe("Listening to you");
    expect(labelOf("thinking", { ...o, using: "3 places" })).toBe("Looking in 3 places");
    expect(labelOf("thinking", o)).toBe("Thinking");
    // A slow provider: nothing out, no reply yet, the model is the wait.
    expect(labelOf("thinking", { ...o, modelSecs: 2.5 })).toBe("Thinking");
    expect(labelOf("thinking", { ...o, modelSecs: 5.6 })).toBe("Waiting on the model · 5 s");
    expect(labelOf("question", { ...o, asker: "Accounting" })).toBe("Accounting asks");
    expect(labelOf("listening", { ...o, micOff: true })).toBe("Mic off · type in Ghost");
    expect(labelOf("line", o)).toBe("");
  });
});

describe("captions", () => {
  const loud = { volume: 0.4, muted: false };
  it("are off by default, and only Ghost's own words while it speaks", () => {
    expect(captionsShown("off", "speaking", { volume: 0, muted: true })).toBe(false);
    expect(captionsShown("always", "speaking", loud)).toBe(true);
    expect(captionsShown("always", "you", loud)).toBe(false);
    expect(captionsShown("always", "thinking", loud)).toBe(false);
  });
  it("follow the output when set to 'when the sound is off', and an unread output is not off", () => {
    expect(captionsShown("muted", "speaking", loud)).toBe(false);
    expect(captionsShown("muted", "speaking", { volume: 0.4, muted: true })).toBe(true);
    expect(captionsShown("muted", "speaking", { volume: 0.03, muted: false })).toBe(true);
    expect(captionsShown("muted", "speaking", null)).toBe(false);
  });
  it("hold the newest words, never the wall", () => {
    const long = "The deploy is blocked on its migration step: the job that adds the invoice currency column timed out twice last night, both times after about twelve minutes.";
    const c = captionText(long, null, 80);
    expect(c.length).toBeLessThanOrEqual(81);
    expect(c.startsWith("…")).toBe(true);
    expect(c.endsWith("twelve minutes.")).toBe(true);
    expect(captionText(long, 26)).toBe("The deploy is blocked on i".trim());
    expect(captionText("Short.", null)).toBe("Short.");
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
