import { describe, expect, it } from "vitest";

import { OFF_FOR, listeningSentence, notListening, notListeningSub, switchLabel, untilText } from "../src/app/listening";
import { MockDaemon } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";

const now = new Date(2026, 10, 12, 14, 0);

describe("not listening, in words", () => {
  it("reads the status's field and nothing else", () => {
    expect(notListening(null)).toBeNull();
    expect(notListening({ not_listening: null })).toBeNull();
    expect(notListening({ not_listening: { until: null } })).toEqual({ until: null });
  });

  it("says until when, or until it is turned back on", () => {
    const later = new Date(2026, 10, 12, 15, 30).toISOString();
    const tomorrow = new Date(2026, 10, 13, 9, 0).toISOString();
    expect(untilText(later, now)).toBe("3:30 PM");
    expect(untilText(tomorrow, now)).toBe("tomorrow 9:00 AM");
    expect(notListeningSub({ until: later }, now)).toBe("Mic off until 3:30 PM");
    expect(notListeningSub({ until: null }, now)).toBe("Mic off");
    expect(listeningSentence({ until: null }, now)).toContain("not even its name");
    expect(listeningSentence(null, now)).toContain("listens for its name");
  });

  it("offers the do-not-disturb lengths and names the switch by what it does", () => {
    expect(OFF_FOR.map((o) => o.minutes)).toEqual([30, 60, undefined]);
    expect(switchLabel(null)).toBe("Stop listening");
    expect(switchLabel({ until: null })).toBe("Listen again");
  });
});

describe("the mock daemon's switch", () => {
  it("starts closed in its scenario, refuses a voice wake, takes a typed turn and opens again", async () => {
    const s = scenarioByName("notlistening");
    const d = new MockDaemon(s, composeFixtures(s), {});
    const status = (await d.handle("GET", "/status", new URLSearchParams(), null)) as { not_listening: unknown; phase: string };
    expect(status.not_listening).toEqual({ until: null });
    await expect(d.handle("POST", "/wake", new URLSearchParams(), {})).rejects.toThrow(/isn’t listening/);
    expect(await d.handle("POST", "/session/say", new URLSearchParams(), { text: "what’s waiting?" })).toEqual({ ok: true });
    expect(((await d.handle("GET", "/status", new URLSearchParams(), null)) as { session_id: string | null }).session_id).toBeTruthy();
    // turning it off ends a session at the desk
    await d.handle("PUT", "/listening", new URLSearchParams(), { listening: false });
    expect(((await d.handle("GET", "/status", new URLSearchParams(), null)) as { session_id: string | null }).session_id).toBeNull();
    const on = await d.handle("PUT", "/listening", new URLSearchParams(), { listening: true });
    expect(on).toEqual({ listening: true, until: null });
    const off = (await d.handle("PUT", "/listening", new URLSearchParams(), { listening: false, minutes: 30 })) as { listening: boolean; until: string };
    expect(off.listening).toBe(false);
    expect(Date.parse(off.until)).toBeGreaterThan(Date.now() + 29 * 60_000);
  });
});
