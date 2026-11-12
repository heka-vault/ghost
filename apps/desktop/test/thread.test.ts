/**
 * The conversation screen's second pass (ADR 0058): the closed turn's words, which words of a
 * reply are heard, stored activity, call previews, and every start state with the mic closed.
 */
import { describe, expect, it, vi } from "vitest";

import { type Activity, type Captions, captionHeard, captionPhase, captionTurn, charsOfWords, createSaidWords, joinSpoken, mergeActivity, noCaptions, wordEnds } from "@ghost/ui";

import { MockDaemon } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";
import { startCopy } from "../src/screens/session";

type T = { role: string; text: string; via?: string; at: string };
const at = (s: number) => `2026-11-12T10:00:${String(s).padStart(2, "0")}Z`;
const said = (text: string, s: number): T => ({ role: "user", text, via: "voice", at: at(s) });
const typed = (text: string, s: number): T => ({ role: "user", text, via: "text", at: at(s) });
const reply = (text: string, s: number): T => ({ role: "assistant", text, via: "voice", at: at(s) });

describe("a closed turn shows the words Ghost heard it as (ADR 0058)", () => {
  const play = (steps: (T | string | { heard: string; reopened?: boolean })[]): Captions<T> =>
    steps.reduce<Captions<T>>((c, s) => (typeof s === "string" ? captionPhase(c, s) : "heard" in s ? captionHeard(c, { text: s.heard, at: at(9), reopened: s.reopened }) : captionTurn(c, s)), noCaptions<T>());

  it("holds the pieces while the status moves, and replaces them with the whole read when the turn closes", () => {
    const pieces = play([said("Can you check on the deploy", 1), said("for the billing service", 3), "thinking", "speaking"]);
    expect(pieces.shown).toEqual([]); // the status is no longer the turn's close
    const c = captionHeard(pieces, { text: "Can you check on the deploy for the billing service?", at: at(6) });
    expect(c.shown).toEqual([said("Can you check on the deploy for the billing service?", 1)]);
    expect(c.held).toEqual([]);
  });

  it("replaces pieces a reloaded page already shows", () => {
    const loaded: Captions<T> = { shown: [reply("Ghost here.", 0), said("Remind me", 1), said("tomorrow", 2)], held: [] };
    expect(captionHeard(loaded, { text: "Remind me tomorrow.", at: at(3) }).shown.map((t) => t.text)).toEqual(["Ghost here.", "Remind me tomorrow."]);
  });

  it("takes a turn back that was reopened, and the next close has its words", () => {
    const c = play([said("Remind me tomorrow", 1), { heard: "Remind me tomorrow." }, { heard: "", reopened: true }, said("to review it", 4), { heard: "Remind me tomorrow to review it." }]);
    expect(c.shown.map((t) => t.text)).toEqual(["Remind me tomorrow to review it."]);
    expect(c.shown[0].at).toBe(at(1));
  });

  it("leaves a typed turn alone, and ignores an empty close", () => {
    const c = play([typed("close infra 17", 1), { heard: "close infra 17" }, { heard: "  " }]);
    expect(c.shown).toEqual([typed("close infra 17", 1)]);
  });

  it("still shows the pieces when an older daemon sends no close: with the reply", () => {
    expect(joinSpoken(play([said("What's", 1), said("waiting?", 2), reply("Two issues.", 3)]).shown).map((t) => t.text)).toEqual(["What's waiting?", "Two issues."]);
  });
});

describe("which words of a reply are heard (ADR 0058)", () => {
  it("counts words as the TTS stage does: letters or digits, no list markers", () => {
    const text = "Three need you:\n1. App 212, the **invoice** bug.\n- And infra 77 — the export.";
    const ends = wordEnds(text);
    // Three need you: App 212, the invoice bug. And infra 77 the export. (— and markers left out)
    expect(ends.length).toBe(13);
    expect(text.slice(0, charsOfWords(text, 3))).toBe("Three need you:");
    expect(text.slice(0, charsOfWords(text, 4))).toBe("Three need you:\n1. App");
    expect(charsOfWords(text, 0)).toBe(0);
    expect(charsOfWords(text, 99)).toBe(text.length);
  });

  it("keeps the latest stamp outside React and tells its subscribers", () => {
    const s = createSaidWords();
    const seen = vi.fn();
    const off = s.subscribe(seen);
    s.set({ session_id: "s1", reply: 2, from: 3, to: 9 });
    expect(s.get()).toEqual({ session_id: "s1", reply: 2, from: 3, to: 9 });
    off();
    s.set(null);
    expect(seen).toHaveBeenCalledTimes(1);
  });
});

describe("activity kept beside the transcript (ADR 0058)", () => {
  const act = (kind: Activity["kind"], s: number, extra: Partial<Activity> = {}): Activity => ({ kind, session_id: "s1", at: at(s), names: [], ...extra });

  it("merges what was stored with what came live, once each, in time order", () => {
    const stored = [act("call", 1, { id: "c1", name: "github_issue_read" }), act("result", 2, { id: "c1", name: "github_issue_read" }), { ...act("cut", 5), at: "2026-11-12T10:00:05.000000+00:00" }];
    const live = [act("result", 2, { id: "c1", name: "github_issue_read" }), act("cut", 5), act("call", 7, { id: "c2", name: "ghost_status" })];
    expect(mergeActivity(stored, live).map((a) => `${a.kind}:${a.id ?? ""}`)).toEqual(["call:c1", "result:c1", "cut:", "call:c2"]);
  });

  it("the mock answers stored activity for history, and a call's result while kept", async () => {
    const s = scenarioByName("thread");
    const f = composeFixtures(s);
    const d = new MockDaemon(s, f, {});
    const q = (sessions: string) => new URLSearchParams({ sessions });
    const stored = (await d.handle("GET", "/activity", q(f.session.sessionId ?? ""), null)) as Activity[];
    expect(stored.some((a) => a.kind === "handover")).toBe(true);
    expect(stored.some((a) => a.kind === "you_started")).toBe(false); // live only
    const preview = (await d.handle("GET", "/calls/c4", new URLSearchParams(), null)) as { preview: string; dropped: number; read: boolean };
    expect(preview.read).toBe(true);
    expect(preview.preview.length).toBeLessThanOrEqual(2000);
    expect(preview.dropped).toBeGreaterThan(0);
    await expect(d.handle("GET", "/calls/gone", new URLSearchParams(), null)).rejects.toThrow(/no longer kept/);
  });
});

describe("the start state with the mic closed (ADR 0057) never asks the operator to speak", () => {
  const base = { online: true, loading: false, needsSetup: false, locked: false, inSession: false, onPhone: false, recording: false };
  const say = /say|go ahead/i;

  it("says the mic is off, with the way back, in every state that would ask for a voice", () => {
    const idle = startCopy({ ...base, micOff: "Mic off" });
    expect(idle.title).toBe("Ghost isn’t listening");
    expect(`${idle.title} ${idle.text}`).not.toMatch(say);
    expect(idle.text).toMatch(/turn listening on/);
    expect(startCopy({ ...base, micOff: "Mic off until 3:30 PM" }).text).toMatch(/until 3:30 PM/);
    const typedSession = startCopy({ ...base, inSession: true, micOff: "Mic off" });
    expect(typedSession.title).toBe("Your mic is off");
    expect(`${typedSession.title} ${typedSession.text}`).not.toMatch(/go ahead|say /i);
    expect(startCopy({ ...base, loading: true, micOff: "Mic off" }).text).not.toMatch(/Hey Ghost/);
  });

  it("still says the wake phrase when the mic is open", () => {
    expect(startCopy(base).title).toMatch(/Hey Ghost/);
  });
});
