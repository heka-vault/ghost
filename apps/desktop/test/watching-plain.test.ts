import { describe, expect, it } from "vitest";

import { watchRequest } from "../src/screens/watching/AddWatch";
import { ownersOf } from "../src/screens/watching/index";
import { reachWords } from "../src/screens/watching/Runs";
import { PLAIN_DEFAULT, plainOf, plainWords, requestOf, tookWords } from "../src/screens/watching/when";

describe("when a schedule runs, without cron", () => {
  it("turns each plain choice into what the daemon takes", () => {
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "weekdays", time: "09:00" })).toEqual({ kind: "cron", when: "0 9 * * 1-5" });
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "daily", time: "18:30" })).toEqual({ kind: "cron", when: "30 18 * * *" });
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "days", time: "07:05", days: [5, 1, 1] })).toEqual({ kind: "cron", when: "5 7 * * 1,5" });
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "interval", n: 2, unit: "hours" })).toEqual({ kind: "every", when: "2h" });
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "once", date: "2026-12-02T09:00" })).toEqual({ kind: "at", when: "2026-12-02T09:00" });
  });
  it("says what is missing instead of sending it", () => {
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "days", days: [] })).toEqual({ problem: "Pick at least one day." });
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "interval", n: 2, unit: "minutes" })).toEqual({ problem: "Every 5 minutes at the most often." });
    expect(requestOf({ ...PLAIN_DEFAULT, mode: "once", date: "" })).toEqual({ problem: "Pick a date and a time." });
    expect("problem" in requestOf({ ...PLAIN_DEFAULT, mode: "cron", cron: "0 9 *" })).toBe(true);
  });
  it("opens a stored schedule as the plain choice that says it, else as cron", () => {
    expect(plainOf("cron", "0 9 * * 1-5")).toMatchObject({ mode: "weekdays", time: "09:00" });
    expect(plainOf("cron", "30 18 * * *")).toMatchObject({ mode: "daily", time: "18:30" });
    expect(plainOf("cron", "0 15 * * 5")).toMatchObject({ mode: "days", days: [5], time: "15:00" });
    expect(plainOf("every", "30m")).toMatchObject({ mode: "interval", n: 30, unit: "minutes" });
    expect(plainOf("cron", "*/10 9-17 * * 1-5")).toMatchObject({ mode: "cron", cron: "*/10 9-17 * * 1-5" });
    expect(plainOf("at", "2026-12-02T09:00")).toMatchObject({ mode: "once" });
  });
  it("reads every choice back the way it round-trips", () => {
    for (const when of ["0 9 * * 1-5", "30 18 * * *", "5 7 * * 1,5"]) {
      const r = requestOf(plainOf("cron", when));
      expect(r).toEqual({ kind: "cron", when });
    }
    expect(plainWords({ ...PLAIN_DEFAULT, mode: "weekdays", time: "09:00" })).toBe("Weekdays at 9 AM");
    expect(plainWords({ ...PLAIN_DEFAULT, mode: "days", time: "15:30", days: [1, 3, 5] })).toBe("Mondays, Wednesdays and Fridays at 3:30 PM");
    expect(plainWords({ ...PLAIN_DEFAULT, mode: "interval", n: 1, unit: "hours" })).toBe("Every hour");
  });
  it("says how long a run took", () => {
    expect(tookWords("2026-11-12T15:00:00Z", "2026-11-12T15:00:04Z")).toBe("4 s");
    expect(tookWords("2026-11-12T15:00:00Z", "2026-11-12T15:01:12Z")).toBe("1 min 12 s");
    expect(tookWords(null, "2026-11-12T15:01:12Z")).toBeNull();
  });
});

describe("a run read as what it did", () => {
  const base = { key: "run-1", apps: ["github", "calendar"], bytes: 34_000, calls: 4, changes: 1, failed: 0, items: 12, ms: 900, reads: 3 };
  it("names the apps by their titles and counts what it read and changed", () => {
    const t = (n: string) => ({ github: "GitHub", calendar: "Calendar" })[n] ?? n;
    expect(reachWords(base, t)).toMatch(/^Read 12 items, .* from GitHub and Calendar; made 1 change$/);
    expect(reachWords({ ...base, calls: 0 }, t)).toBeNull();
    expect(reachWords(undefined, t)).toBeNull();
  });
});

describe("the page's owners and a watch asked for in words", () => {
  it("lists every agent that owns something here, Ghost first", () => {
    expect(ownersOf([{ agent: "legal" }, { agent: "ghost" }], [{ made_by: "agent:ledger", acts_through: "accounting" }, { made_by: null }], [{ agent: "infra" }])).toEqual(["ghost", "accounting", "infra", "ledger", "legal"]);
  });
  it("puts the operator's choices into one request Ghost can act on", () => {
    const text = watchRequest({ app: "github", appTitle: "GitHub", what: "New issues labeled bug. ", often: "15m", loud: "notify", agent: "legal", agentTitle: "Legal", act: "label it" });
    expect(text).toBe("Set up a watch on GitHub (github): New issues labeled bug. Look every 15 minutes, and tell me about each new one with a notification. For each new one, have Legal label it.");
  });
});
