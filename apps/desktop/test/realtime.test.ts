import { describe, expect, it } from "vitest";

import { MockDaemon } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";
import { checkLine, checkTone, modeValues, needsConsent, parseCap, spendLine } from "../src/screens/settings/realtime";
import type { RealtimeCheckView, RealtimeView } from "@ghost/client-ts";

const q = new URLSearchParams();

describe("the voice mode (ADR 0062)", () => {
  it("is this machine's voice unless the operator chose realtime, and only turning it on asks", () => {
    expect(modeValues("local")).toEqual({ GHOST_VOICE_MODE: null });
    expect(modeValues("realtime")).toEqual({ GHOST_VOICE_MODE: "realtime" });
    expect(needsConsent("local", "realtime")).toBe(true);
    expect(needsConsent("realtime", "local")).toBe(false);
  });

  it("says the day's spend and the check in plain words", () => {
    const spend = { day: "2026-11-12", usd: 0.37, sessions: 6, last_session_usd: 0.06, prices_as_of: "2026-11-12", priced: true };
    expect(spendLine(spend)).toBe("About $0.37 today over 6 sessions; $0.06 the last one.");
    expect(spendLine({ ...spend, usd: 0, sessions: 0 })).toBe("Nothing spent today.");
    expect(spendLine({ ...spend, priced: false })).toContain("can’t estimate");
    const ok: RealtimeCheckView = { checked_at: "", provider: "openai", model: "m", reachable: true, error: null, connect_ms: 380, first_audio_ms: 540, tools: true, usd: 0.001 };
    expect(checkLine(ok)).toBe("Connected: first audio 0.54 s, hands requests to Ghost’s agent.");
    expect(checkTone(ok)).toBe("ok");
    expect(checkTone({ ...ok, tools: false })).toBe("warn");
    expect(checkLine({ ...ok, reachable: false, error: "refused: HTTP 401" })).toBe("Didn’t connect: refused: HTTP 401.");
  });

  it("reads a cap in dollars, empty as none", () => {
    expect(parseCap("")).toEqual({ value: null });
    expect(parseCap("$1.50")).toEqual({ value: "1.5" });
    expect(parseCap("lots")).toHaveProperty("error");
    expect(parseCap("-1")).toHaveProperty("error");
  });

  it("is off in the mock until chosen, says what changes, and checks once its key is set", async () => {
    const s = scenarioByName("passive");
    const d = new MockDaemon(s, composeFixtures(s), {});
    const v = (await d.handle("GET", "/voice/realtime", q, null)) as RealtimeView;
    expect(v.mode).toBe("local");
    const lines = v.providers[0].disclosure.join(" ");
    expect(lines).toContain("your voice itself goes to OpenAI Realtime");
    expect(lines).toContain("stay on this machine");
    expect(lines).toContain("switch back");
    const put = (await d.handle("PUT", "/settings", q, { values: { GHOST_VOICE_MODE: "realtime" } })) as { applied: string[] };
    expect(put.applied).toContain("GHOST_VOICE_MODE");
    expect(((await d.handle("GET", "/voice/realtime", q, null)) as RealtimeView).mode).toBe("realtime");
    const c = (await d.handle("POST", "/voice/realtime/check", q, null)) as RealtimeCheckView;
    expect(c.reachable).toBe(v.password_set);
  });

  it("has a scenario with the realtime voice on, a day of use and a check", async () => {
    const s = scenarioByName("realtime");
    expect(s.name).toBe("realtime");
    const d = new MockDaemon(s, composeFixtures(s), {});
    const v = (await d.handle("GET", "/voice/realtime", q, null)) as RealtimeView;
    expect(v.mode).toBe("realtime");
    expect(v.password_set).toBe(true);
    expect(v.daily_cap_usd).toBe(2);
    expect(v.spend.sessions).toBeGreaterThan(0);
    expect(v.last_check?.reachable).toBe(true);
  });
});
