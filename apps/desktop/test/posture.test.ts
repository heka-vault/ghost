import { describe, expect, it } from "vitest";

import { ROOM_KEYS, forgetRoomChoices, postureOf } from "../src/app/posture";

const base = { deskSession: false };

describe("postureOf: the desk or floating, nothing else", () => {
  it("is the desk outside a session, however long nobody touched the window", () => {
    // Before 2026-11-14 three minutes away brought the room; now the window stays as it is.
    expect(postureOf(base)).toBe("desk");
    expect(postureOf({ ...base, floating: true, windowActive: false })).toBe("desk");
  });

  it("keeps a session in the window when the floating room is off or can't show", () => {
    // Before, 20 s of quiet in a session brought the room; now it stays at the desk.
    const session = { deskSession: true, windowActive: false };
    expect(postureOf(session)).toBe("desk");
    expect(postureOf({ ...session, floating: false })).toBe("desk");
  });

  it("floats a desk session at once when it is on and the window is not the one in use", () => {
    expect(postureOf({ deskSession: true, floating: true, windowActive: false })).toBe("float");
  });

  it("leaves a session in the window the operator is using", () => {
    expect(postureOf({ deskSession: true, floating: true, windowActive: true })).toBe("desk");
    // A window that doesn't say is its own view: it is in use.
    expect(postureOf({ deskSession: true, floating: true })).toBe("desk");
  });

  it("never floats a session on the phone (deskSession is false for it)", () => {
    expect(postureOf({ deskSession: false, floating: true, windowActive: false, introducing: true })).toBe("desk");
  });

  it("only ever answers desk or float", () => {
    const answers = new Set<string>();
    for (const deskSession of [true, false])
      for (const floating of [true, false, undefined])
        for (const windowActive of [true, false, undefined])
          for (const introducing of [true, false, undefined]) answers.add(postureOf({ deskSession, floating, windowActive, introducing }));
    expect([...answers].sort()).toEqual(["desk", "float"]);
  });
});

describe("the first run's moment (ADR 0059 as amended 2026-11-14)", () => {
  const floating = { deskSession: true, floating: true, windowActive: true };
  it("floats over the window the newcomer is using while its wake beat is open", () => {
    expect(postureOf(floating)).toBe("desk");
    expect(postureOf({ ...floating, introducing: true })).toBe("float");
  });
  it("still lets the floating rule decide: a share or a desktop that can't float keeps it in the window", () => {
    // `floating` is false while the screen is shared and Ghost is kept out of it.
    expect(postureOf({ ...floating, floating: false, introducing: true })).toBe("desk");
    expect(postureOf({ ...floating, deskSession: false, introducing: true })).toBe("desk");
  });
});

describe("an older install's room choices", () => {
  function store(entries: Record<string, string>) {
    const m = new Map(Object.entries(entries));
    return { m, removeItem: (k: string) => void m.delete(k) };
  }
  it("are removed quietly, and nothing else is", () => {
    const s = store({ "ghost.room.away": "10", "ghost.room.session": "off", "ghost.onboarding": "{}" });
    forgetRoomChoices(s);
    expect([...s.m.keys()]).toEqual(["ghost.onboarding"]);
    expect(ROOM_KEYS).toEqual(["ghost.room.away", "ghost.room.session"]);
  });
  it("never throws on storage that refuses, or none at all", () => {
    expect(() =>
      forgetRoomChoices({
        removeItem: () => {
          throw new Error("SecurityError");
        },
      }),
    ).not.toThrow();
    expect(() => forgetRoomChoices(null)).not.toThrow();
  });
  it("leaves an old stored hold of the room with nothing to act on", () => {
    // The hold was never stored, and the rule takes no hold: a `?posture=room` address or a
    // stale value has no input left to reach.
    expect(postureOf({ deskSession: true, ...({ hold: "room" } as object) })).toBe("desk");
  });
});
