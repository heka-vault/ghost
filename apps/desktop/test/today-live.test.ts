/**
 * Today's live strip (2026-11-14, the room view removed): a session that doesn't float is seen
 * on the desk, in the conversation screen's own words, with a yes or a question waiting right
 * above the talk bar, and never twice on the page.
 */
import type { QuestionView, WaitingItemView } from "@ghost/client-ts";
import type { Call } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { floatingSwitchLabel } from "../src/app/FloatingSwitch";
import type { FloatingStatus } from "../src/app/floating";
import { stageState, stageWords } from "../src/screens/session/Stage";
import { liveQuestion, withoutLive } from "../src/screens/today/live";

const q = (key: string, state: QuestionView["state"], at: string): QuestionView => ({
  key, kind: "question", source: "agent:accounting", asked_by: "Accounting", asked_at: at, question: "Send it?", detail: null,
  choices: ["Yes", "No"], choices_only: true, state, urgency: "notify", in_run: false, expires_at: "2026-11-14T11:00:00Z", answer: null, answered_at: null, answered_via: null, choice: null, change_id: null,
});
const item = (key: string, extra: Partial<WaitingItemView> = {}): WaitingItemView => ({ key, source: "gate", title: key, state: "new", first_seen: "2026-11-14T10:00:00Z", changed_at: "2026-11-14T10:00:00Z", age_secs: 0, ...extra });

describe("the live strip's words, from the conversation screen's stage", () => {
  const words = (s: Parameters<typeof stageState>[0], calls: Parameters<typeof stageWords>[2] = []) => stageWords(stageState(s), "Ghost", calls).title;
  const live = { online: true, youTalking: false, ending: false, onPhone: false };
  it("says each phase a person at the desk needs to read at a glance", () => {
    expect(words({ ...live, phase: "listening" })).toBe("Listening");
    expect(words({ ...live, phase: "listening", youTalking: true })).toBe("Listening to you");
    expect(words({ ...live, phase: "thinking" })).toBe("Working on it");
    expect(words({ ...live, phase: "speaking" })).toBe("Ghost is speaking");
    expect(words({ ...live, phase: "hold" })).toBe("On hold");
    expect(words({ ...live, phase: "listening", onPhone: true })).toBe("On your phone");
  });
  it("names what Ghost is reaching while its calls are out", () => {
    const calls: Call[] = [
      { id: "1", name: "github_issue_list", app: "github", tool: "issue list", startedAt: "2026-11-14T10:00:00Z", endedAt: null },
      { id: "2", name: "mobbin_search_screens", app: "mobbin", tool: "search screens", startedAt: "2026-11-14T10:00:00Z", endedAt: null },
    ];
    expect(words({ ...live, phase: "thinking" }, calls)).toBe("Using GitHub and Mobbin");
  });
});

describe("liveQuestion", () => {
  it("offers the newest question still open, and nothing once it is answered", () => {
    expect(liveQuestion([])).toBeNull();
    expect(liveQuestion([q("a", "open", "1"), q("b", "open", "2")])?.key).toBe("b");
    expect(liveQuestion([q("a", "open", "1"), q("b", "answered", "2")])?.key).toBe("a");
    expect(liveQuestion([q("a", "declined", "1"), q("b", "expired", "2")])).toBeNull();
  });
});

describe("withoutLive: nothing shows twice on Today", () => {
  const needs = {
    decide: [item("gate:1", { change: { id: "c1" } as WaitingItemView["change"] }), item("gate:2", { change: { id: "c2" } as WaitingItemView["change"] })],
    answer: [item("question:q1", { question: q("question:q1", "open", "1") }), item("question:q2", { question: q("question:q2", "open", "2") })],
    know: [item("watch:x")],
  };
  it("leaves out the change waiting in the session, which the strip shows", () => {
    const out = withoutLive(needs, { id: "c1" }, { key: "question:q2" });
    expect(out.decide.map((i) => i.key)).toEqual(["gate:2"]);
    // The strip shows the change, not the question: the question stays in Answer, or it would be nowhere.
    expect(out.answer.map((i) => i.key)).toEqual(["question:q1", "question:q2"]);
    expect(out.know).toBe(needs.know);
  });
  it("leaves out the question the strip offers when no change waits", () => {
    const out = withoutLive(needs, null, { key: "question:q2" });
    expect(out.decide.map((i) => i.key)).toEqual(["gate:1", "gate:2"]);
    expect(out.answer.map((i) => i.key)).toEqual(["question:q1"]);
  });
  it("changes nothing outside a session", () => {
    expect(withoutLive(needs, null, null)).toBe(needs);
  });
});

describe("the rail's floating switch, where the room button was", () => {
  const st = (on: boolean, support: FloatingStatus["support"] = { kind: "xwayland" }): FloatingStatus => ({
    support, settings: { on, monitor: "other" }, running: on, capture: "no", monitors: [], helper: false,
  });
  it("says what it does in one line, on or off", () => {
    expect(floatingSwitchLabel(st(true))).toMatch(/floats over your work/);
    expect(floatingSwitchLabel(st(false))).toMatch(/^Float over the desktop/);
  });
  it("is not there where the desktop can't float, or before the shell has said", () => {
    expect(floatingSwitchLabel(st(false, { kind: "no", why: "No X11." }))).toBeNull();
    expect(floatingSwitchLabel(null)).toBeNull();
  });
  it("never mentions a room in the window", () => {
    for (const on of [true, false]) expect(floatingSwitchLabel(st(on))).not.toMatch(/\broom\b/i);
  });
});
