import type { Activity } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { appName, litOf, MIN_LIT_MS, namesText, placeOf, reachOf, RETURN_MS, returnOf, sizeOf, TRACE, wordOf } from "../src/floating/reach";

const t0 = Date.UTC(2026, 10, 12, 15, 0, 0);
const at = (ms: number) => new Date(t0 + ms).toISOString();
const a = (kind: Activity["kind"], ms: number, extra: Partial<Activity> = {}): Activity => ({ kind, at: at(ms), session_id: "s", names: [], ...extra }) as Activity;
const APPS = ["github", "github-acme", "calendar", "mobbin"];

describe("where Ghost is looking", () => {
  it("names a place by its app and a word or two of what was asked, never the tool's identifier", () => {
    expect(placeOf("github_issue_list", APPS)).toEqual({ key: "github", name: "GitHub", word: "issues" });
    expect(placeOf("calendar_events_today", APPS)).toEqual({ key: "calendar", name: "Calendar", word: "events today" });
    expect(placeOf("github-acme_actions_runs", APPS)).toEqual({ key: "github-acme", name: "GitHub (acme)", word: "actions runs" });
    expect(placeOf("github_issue_read", APPS).word).toBe("issue");
    expect(wordOf("list")).toBeNull();
    expect(appName("linear")).toBe("Linear");
  });

  it("reads Ghost's own lookups as places too: its memory, its docs, the agents (the live screenshot's ghost.docs_search)", () => {
    expect(placeOf("ghost_docs_search", APPS)).toMatchObject({ key: "ghost:docs", name: "Docs", word: null });
    expect(placeOf("ghost_recall", APPS)).toMatchObject({ key: "ghost:memory", name: "Memory" });
    expect(placeOf("ghost_transcript_read", APPS).name).toBe("Memory");
    expect(placeOf("ghost_ask_agent", APPS).name).toBe("Agents");
    expect(placeOf("ghost_schedules_list", APPS)).toMatchObject({ name: "Schedules", word: null });
    expect(placeOf("ghost_status", APPS)).toMatchObject({ key: "ghost", name: "Ghost" });
    // A tool no connected app claims (the list not read yet): named by its first part, as
    // tools are offered (`<app>_<tool>`).
    expect(placeOf("weather_now", APPS)).toEqual({ key: "tool:weather", name: "Weather", word: "now" });
    expect(placeOf("calendar_events_today", [])).toEqual({ key: "tool:calendar", name: "Calendar", word: "events today" });
    expect(placeOf("search", [])).toEqual({ key: "tool:search", name: "Search", word: null });
  });

  it("is this turn's places only, in the order reached, with what is still out", () => {
    const { places, more } = reachOf(
      [
        a("call", 0, { id: "old", name: "mobbin_screens_search" }),
        a("result", 100, { id: "old", name: "mobbin_screens_search" }),
        a("you_started", 200),
        a("you_stopped", 1000),
        a("call", 1200, { id: "c1", name: "github_issue_list" }),
        a("call", 1300, { id: "c2", name: "calendar_events_today" }),
        a("call", 1400, { id: "c3", name: "github_issue_read" }),
        a("result", 2000, { id: "c1", name: "github_issue_list" }),
        a("result", 2100, { id: "c2", name: "calendar_events_today" }),
      ],
      APPS,
    );
    expect(more).toBe(0);
    expect(places.map((p) => [p.name, p.word, p.calls, p.open])).toEqual([
      ["GitHub", "issue", 2, 1],
      ["Calendar", "events today", 1, 0],
    ]);
    expect(places[1].endedAt).toBe(t0 + 2100);
  });

  it("draws at most six places and counts the rest", () => {
    const names = ["github_a_list", "calendar_b_list", "mobbin_c_list", "github-acme_d_list", "ghost_recall", "ghost_docs_search", "ghost_ask_agent", "weather_now"];
    const list = [a("you_stopped", 0), ...names.map((n, i) => a("call", 10 + i, { id: `c${i}`, name: n }))];
    const { places, more } = reachOf(list, APPS);
    expect(places).toHaveLength(6);
    expect(more).toBe(2);
    // The newest are kept.
    expect(places[5].key).toBe("tool:weather");
  });

  it("lights a quick call long enough to be seen, flashes its answer back, then leaves a trace", () => {
    const [p] = reachOf([a("you_stopped", 0), a("call", 0, { id: "q", name: "ghost_docs_search" }), a("result", 91, { id: "q", name: "ghost_docs_search" })], APPS).places;
    // 91 ms was three frames at 30 a second: lit for MIN_LIT_MS now.
    expect(litOf(p, t0 + 300)).toBe(1);
    expect(returnOf(p, t0 + 300)).toBe(-1);
    const back = t0 + MIN_LIT_MS + RETURN_MS / 2;
    expect(litOf(p, back)).toBeGreaterThan(TRACE);
    expect(returnOf(p, back)).toBeCloseTo(0.5, 2);
    expect(litOf(p, t0 + MIN_LIT_MS + RETURN_MS + 1)).toBe(TRACE);
    expect(returnOf(p, t0 + 60_000)).toBe(-1);
  });

  it("keeps a place lit while any of its calls is out", () => {
    const [p] = reachOf([a("you_stopped", 0), a("call", 0, { id: "x", name: "github_issue_list" }), a("call", 5, { id: "y", name: "github_issue_read" }), a("result", 50, { id: "x", name: "github_issue_list" })], APPS).places;
    expect(p.open).toBe(1);
    expect(litOf(p, t0 + 30_000)).toBe(1);
    expect(returnOf(p, t0 + 30_000)).toBe(-1);
  });

  it("says where in a few words: the places name themselves", () => {
    expect(namesText([])).toBeNull();
    expect(namesText(["GitHub"])).toBe("GitHub");
    expect(namesText(["GitHub", "Calendar", "Memory"])).toBe("3 places");
  });

  it("sizes the light coming back by what came back, √ of its characters", () => {
    expect(sizeOf(null)).toBeCloseTo(0.15);
    expect(sizeOf(24_000)).toBe(1);
    expect(sizeOf(6_000)).toBeGreaterThan(sizeOf(600));
    expect(sizeOf(10_000_000)).toBe(1);
  });
});
