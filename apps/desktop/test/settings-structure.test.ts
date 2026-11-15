import { describe, expect, it } from "vitest";

import { HINT_FROM, NO_TAPS, TAP_GAP_MS, UNLOCK_TAPS, tap } from "../src/app/developer";
import { placeOf } from "../src/app/places";
import { DEVELOPER_PAGE, REDIRECTS, SETTINGS_PAGES, SETTINGS_SUBPAGES, redirectFor, resolve, settingsNavId, settingsPageOf } from "../src/app/routes";
import { debugWords, reachSentence, timeLeft } from "../src/screens/settings/plain";
import { MOVED, PAGES_AFTER, PAGES_BEFORE } from "../src/screens/settings/moved";

describe("Settings has five pages for everyone (ADR 0072)", () => {
  it("lists General, Model, Sound, Permissions and About, and Developer only once unlocked", () => {
    expect(SETTINGS_PAGES.map((p) => p.label)).toEqual(PAGES_AFTER);
    expect(PAGES_BEFORE).toHaveLength(11);
    expect(SETTINGS_PAGES.some((p) => p.id === "developer")).toBe(false);
    expect(DEVELOPER_PAGE.path).toBe("/settings/developer");
  });

  it("opens every page and sub-page, and marks a sub-page's parent in the nav", () => {
    for (const p of [...SETTINGS_PAGES, DEVELOPER_PAGE, ...SETTINGS_SUBPAGES]) {
      const r = resolve(p.path);
      expect(r, p.path).not.toBeNull();
      expect(r!.route.area, p.path).toBe("settings");
      expect(settingsPageOf(p.path)?.id, p.path).toBe(p.id);
    }
    expect(settingsNavId(settingsPageOf("/settings/phone")!)).toBe("general");
    expect(settingsNavId(settingsPageOf("/settings/developer/passwords")!)).toBe("developer");
    expect(settingsNavId(settingsPageOf("/settings/developer/audit")!)).toBe("developer");
  });
});

describe("every old address redirects to its new home", () => {
  const cases: [string, string][] = [
    ["/settings", "/settings/general"],
    ["/settings/agents", "/agents"],
    ["/settings/agents/legal", "/agents/legal/settings"],
    ["/settings/passwords", "/settings/developer/passwords"],
    ["/settings/passwords/secrets", "/settings/developer/passwords"],
    ["/settings/skills", "/settings/developer/skills"],
    ["/settings/tools", "/settings/developer/tools"],
    ["/settings/audit", "/settings/developer/audit"],
    ["/settings/notifications", "/settings/developer/notifications"],
    ["/settings/general/updates", "/settings/about/updates"],
    ["/settings/general/about", "/settings/about"],
    ["/skills", "/settings/developer/skills"],
    ["/routes", "/settings/developer/notifications"],
    ["/audit", "/settings/developer/audit"],
    ["/policy", "/settings/permissions"],
    ["/settings/apps/github", "/apps/github"],
  ];
  it.each(cases)("%s → %s", (from, to) => {
    expect(redirectFor(from)).toBe(to);
    expect(resolve(from), from).not.toBeNull();
    expect(resolve(to), to).not.toBeNull();
  });
  it("covers every redirect in the table, and none leads to another redirect", () => {
    for (const r of REDIRECTS) {
      const sample = r.from.replace(/\/:[a-z]+\?/g, "").replace(/:[a-z]+/g, "x");
      const to = redirectFor(sample);
      expect(to, r.from).not.toBeNull();
      expect(redirectFor(to!), `${r.from} → ${to}`).toBeNull();
    }
  });
  it("keeps an agent's conversation and its own page apart", () => {
    expect(resolve("/agents/legal")?.route.id).toBe("conversations");
    expect(resolve("/agents/legal/settings")?.route.id).toBe("agents");
    expect(resolve("/agents")?.route.id).toBe("agents");
    expect(resolve("/agents")?.route.area).toBeUndefined();
  });
});

describe("every control that existed has a home (the PR's checklist)", () => {
  it.each(MOVED.map((x) => [x.was, x.control, x.now] as const))("%s: %s → %s", (_was, _control, now) => {
    if (now === "tray") return;
    expect(resolve(redirectFor(now) ?? now), now).not.toBeNull();
    expect(redirectFor(now), `${now} is current, not an old address`).toBeNull();
  });
  it("names every old page", () => {
    for (const page of PAGES_BEFORE) expect(MOVED.some((x) => x.was === page), page).toBe(true);
  });
});

describe("clicking the version seven times shows Developer", () => {
  it("counts, hints from the fourth, and unlocks on the seventh", () => {
    let taps = NO_TAPS;
    let t = 1000;
    const hints: (string | null)[] = [];
    let unlocked = false;
    for (let i = 0; i < UNLOCK_TAPS; i++) {
      const r = tap(taps, (t += 300), false);
      taps = r.taps;
      hints.push(r.hint);
      unlocked ||= r.unlock;
    }
    expect(unlocked).toBe(true);
    expect(hints.slice(0, HINT_FROM - 1).every((h) => h === null)).toBe(true);
    expect(hints[HINT_FROM - 1]).toBe("3 more clicks for developer settings.");
    expect(hints[UNLOCK_TAPS - 2]).toBe("1 more click for developer settings.");
    expect(hints[UNLOCK_TAPS - 1]).toMatch(/Developer settings are on/);
    expect(taps).toEqual(NO_TAPS);
  });
  it("starts again after a pause", () => {
    let r = tap(NO_TAPS, 0, false);
    for (let i = 1; i < 5; i++) r = tap(r.taps, i * 100, false);
    expect(r.taps.count).toBe(5);
    r = tap(r.taps, 400 + TAP_GAP_MS + 1, false);
    expect(r.taps.count).toBe(1);
    expect(r.hint).toBeNull();
  });
  it("says it is on already, and counts nothing, once unlocked", () => {
    const r = tap({ count: 3, at: 0 }, 100, true);
    expect(r.unlock).toBe(false);
    expect(r.taps).toEqual(NO_TAPS);
    expect(r.hint).toMatch(/already on/);
  });
});

describe("debug mode's marks (ADR 0073)", () => {
  const now = Date.parse("2026-11-14T15:00:00Z");
  it("says the time left, or why it has none", () => {
    expect(debugWords(null, now)).toBeNull();
    expect(debugWords({ until: "2026-11-14T15:42:10Z" }, now)).toBe("43 min left");
    expect(debugWords({ until: "2026-11-14T15:00:30Z" }, now)).toBe("under a minute left");
    expect(debugWords({ until: null }, now)).toBe("on until Ghost stops");
    expect(timeLeft("2026-11-14T15:42:10Z", now)).toBe("42 min 10 s");
    expect(timeLeft("2026-11-14T14:00:00Z", now)).toBeNull();
  });
  it("leads its waiting note to Developer", () => {
    expect(placeOf({ place: "developer", key: "debug:audio" })?.path).toBe("/settings/developer/debug");
    expect(resolve("/settings/developer/debug")?.params).toEqual({ page: "developer", section: "debug" });
  });
});

describe("how Ghost reaches you, in a sentence", () => {
  const base = { escalate_after: 10, batch_secs: 60, calls_per_hour: 2, calls_per_day: 6, carry: "count", greeting: "Hi", urgency: {}, available: [], ntfy: null, ntfy_token: null } as const;
  it("names the ways in order and the quiet hours", () => {
    expect(reachSentence({ ...base, enabled: ["digest"], quiet_hours: null } as never)).toBe("When something needs you, Ghost tells you the next time you wake it. It doesn’t reach out otherwise.");
    expect(reachSentence({ ...base, enabled: ["digest", "desk", "desktop"], quiet_hours: "22:00-07:00" } as never)).toBe(
      "When something needs you, Ghost tells you the next time you wake it. If you don’t hear about it, it tries, in order: asks at the desk, desktop notification. Quiet hours: 22:00 to 07:00, when only the quiet ways are used.",
    );
  });
});
