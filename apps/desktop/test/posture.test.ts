import { describe, expect, it } from "vitest";

import { SESSION_IDLE_MS, isRoomKey, nextChangeIn, postureOf } from "../src/app/posture";

const base = { hold: "auto" as const, deskSession: false, idleMs: 0, awayAfterMs: 180_000 };

describe("postureOf", () => {
  it("stays at the desk while the operator is using the window", () => {
    expect(postureOf(base).posture).toBe("desk");
    expect(postureOf({ ...base, deskSession: true, idleMs: 5_000 }).posture).toBe("desk");
  });

  it("goes to the room in a desk session after the conversation screen's own 20 s", () => {
    expect(postureOf({ ...base, deskSession: true, idleMs: SESSION_IDLE_MS - 1 }).posture).toBe("desk");
    expect(postureOf({ ...base, deskSession: true, idleMs: SESSION_IDLE_MS })).toEqual({ posture: "room", reason: "session" });
  });

  it("goes to the room once the operator has stepped away, outside a session", () => {
    expect(postureOf({ ...base, idleMs: 179_999 }).posture).toBe("desk");
    expect(postureOf({ ...base, idleMs: 180_000 })).toEqual({ posture: "room", reason: "away" });
  });

  it("never goes by itself when the operator said never", () => {
    expect(postureOf({ ...base, awayAfterMs: null, idleMs: 9e9 }).posture).toBe("desk");
    expect(postureOf({ ...base, deskSession: true, sessionIdleMs: null, idleMs: 9e9 }).posture).toBe("desk");
  });

  it("lets the operator's hold win either way", () => {
    expect(postureOf({ ...base, hold: "room" })).toEqual({ posture: "room", reason: "held" });
    expect(postureOf({ ...base, hold: "desk", deskSession: true, idleMs: 9e9 }).posture).toBe("desk");
  });

  it("does not use the stepped-away delay inside a session", () => {
    // 3 minutes away is long past 20 s: a session uses its own threshold, not the away one.
    expect(postureOf({ ...base, deskSession: true, awayAfterMs: 1_000, idleMs: 2_000 }).posture).toBe("desk");
  });
});

describe("postureOf, the floating room (ADR 0059)", () => {
  const away = { ...base, deskSession: true, floating: true, windowActive: false };
  it("floats a desk session at once when it is on and the window is not the one in use", () => {
    expect(postureOf(away)).toEqual({ posture: "float", reason: "session" });
    expect(postureOf({ ...away, idleMs: 9e9 }).posture).toBe("float");
  });
  it("changes nothing when it is off or cannot show", () => {
    expect(postureOf({ ...away, floating: false }).posture).toBe("desk");
    expect(postureOf({ ...away, floating: undefined, idleMs: SESSION_IDLE_MS })).toEqual({ posture: "room", reason: "session" });
  });
  it("leaves a session in the window the operator is using", () => {
    expect(postureOf({ ...away, windowActive: true }).posture).toBe("desk");
    expect(postureOf({ ...away, windowActive: true, idleMs: SESSION_IDLE_MS }).posture).toBe("room");
    // A window that doesn't say is its own view: it is in use.
    expect(postureOf({ ...away, windowActive: undefined }).posture).toBe("desk");
  });
  it("never floats outside a session, or a session on the phone", () => {
    expect(postureOf({ ...away, deskSession: false, idleMs: 9e9 })).toEqual({ posture: "room", reason: "away" });
  });
  it("yields to a room held in the window, but not to the desk", () => {
    expect(postureOf({ ...away, hold: "room" }).posture).toBe("room");
    expect(postureOf({ ...away, hold: "desk" }).posture).toBe("float");
  });
  it("has nothing to wait for while floating", () => {
    expect(nextChangeIn({ ...away, idleMs: 1_000 })).toBeNull();
  });
});

describe("nextChangeIn", () => {
  it("says when the room will come with no input", () => {
    expect(nextChangeIn({ ...base, idleMs: 60_000 })).toBe(120_000);
    expect(nextChangeIn({ ...base, deskSession: true, idleMs: 15_000 })).toBe(5_000);
  });
  it("is null when nothing will change by itself", () => {
    expect(nextChangeIn({ ...base, idleMs: 200_000 })).toBeNull();
    expect(nextChangeIn({ ...base, hold: "room" })).toBeNull();
    expect(nextChangeIn({ ...base, awayAfterMs: null })).toBeNull();
  });
});

describe("isRoomKey", () => {
  const k = (key: string, mods: Partial<{ ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean }> = {}) => ({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });
  it("is Ctrl+. alone", () => {
    expect(isRoomKey(k(".", { ctrlKey: true }))).toBe(true);
    expect(isRoomKey(k("."))).toBe(false);
    expect(isRoomKey(k(".", { ctrlKey: true, shiftKey: true }))).toBe(false);
  });
});
