import { describe, expect, it } from "vitest";

import { MockSchedules } from "../src/design/mock/schedules";
import { OUTCOME, intervalWords, madeBy, untilWords, whenWords } from "../src/screens/watching/model";

describe("a schedule in the operator's words", () => {
  const now = Date.parse("2026-11-11T14:00:00Z");
  const iso = (minutes: number) => new Date(now + minutes * 60_000).toISOString();

  it("says when the next run is", () => {
    expect(untilWords(null, now)).toBe("");
    expect(untilWords(iso(-5), now)).toBe("now");
    expect(untilWords(iso(48), now)).toBe("in 48 min");
    expect(untilWords(iso(60 * 14), now)).toBe("in 14 h");
    expect(untilWords(iso(60 * 26), now)).toBe("tomorrow");
    expect(untilWords(iso(60 * 24 * 3), now)).toBe("in 3 days");
    expect(untilWords(iso(60 * 24 * 20), now)).toBe("Dec 1");
  });

  it("says how often", () => {
    expect(intervalWords("30m")).toBe("30 minutes");
    expect(intervalWords("1d")).toBe("1 day");
    expect(intervalWords("fortnightly")).toBe("fortnightly");
    const base = { at: null, every: null, cron: null, time_zone: "America/Detroit" };
    expect(whenWords({ ...base, kind: "every", every: "2h" }, "America/Detroit")).toEqual({ text: "Every 2 hours" });
    expect(whenWords({ ...base, kind: "every", every: "1d" }, "America/Detroit")).toEqual({ text: "Every day" });
    // A cron expression is shown as written, and its zone only when it isn't this machine's.
    expect(whenWords({ ...base, kind: "cron", cron: "0 9 * * 1-5" }, "America/Detroit")).toEqual({ text: "On", mono: "0 9 * * 1-5" });
    expect(whenWords({ ...base, kind: "cron", cron: "0 9 * * 1-5", time_zone: "Europe/London" }, "America/Detroit")).toEqual({ text: "In Europe/London, on", mono: "0 9 * * 1-5" });
    expect(whenWords({ ...base, kind: "at", at: "2031-12-01T14:00:00Z" }, "America/Detroit").text).toMatch(/^Once, Dec 1, 2031 at /);
  });

  it("names who set it up and how each run ended", () => {
    expect(madeBy("operator")).toEqual({ you: true });
    expect(madeBy("agent:ledger")).toEqual({ you: false, slug: "ledger" });
    expect(OUTCOME.unchanged).toEqual({ label: "Nothing changed", tone: "neutral" });
    expect(OUTCOME.timed_out.tone).toBe("danger");
  });
});

describe("the mock daemon's schedules", () => {
  const mock = () => {
    const delivered: string[] = [];
    const timers: Array<() => void> = [];
    const m = new MockSchedules(
      { view: { schedules: [], time_zone: "America/Detroit", problems: [] }, runs: {} },
      {
        agent: (slug) => (slug === "ghost" || slug === "ledger" ? "known" : slug === "old" ? "retired" : "unknown"),
        deliver: (_agent, _key, title) => delivered.push(title),
        later: (fn) => timers.push(fn),
      },
    );
    const call = (method: string, path: string, body: Record<string, unknown> = {}) => m.handle(method, path, new URLSearchParams(), body) as Record<string, unknown>;
    return { call, delivered, timers };
  };

  it("adds, pauses, runs and removes a schedule, and refuses as the daemon does", () => {
    const { call, delivered, timers } = mock();
    const made = call("POST", "/schedules", { instruction: "Check the build.", every: "30m", agent: "ledger" });
    expect(made.kind).toBe("every");
    expect(made.created_by).toBe("operator");
    expect(made.name).toBe("Check the build.");
    const id = made.id as string;
    expect((call("GET", "/schedules").schedules as unknown[]).length).toBe(1);
    expect(call("POST", `/schedules/${id}/pause`)).toMatchObject({ enabled: false, next_run: null });
    expect(call("POST", `/schedules/${id}/resume`).enabled).toBe(true);
    const run = call("POST", `/schedules/${id}/run`);
    expect(run.status).toBe("queued");
    expect(call("POST", `/schedules/${id}/run`).id).toBe(run.id); // never a second beside it
    timers.forEach((f) => f());
    expect((call("GET", `/schedules/${id}/runs`).runs as Array<Record<string, unknown>>)[0]).toMatchObject({ status: "ok", delivery: "delivered" });
    expect(delivered).toHaveLength(1);
    expect(call("GET", `/schedules/${id}`).last_outcome).toBe("ok");

    expect(() => call("POST", "/schedules", { instruction: "x" })).toThrow(/at \(a time\), every/);
    expect(() => call("POST", "/schedules", { instruction: "x", every: "1h", at: "20m" })).toThrow(/not two/);
    expect(() => call("POST", "/schedules", { instruction: "x", cron: "at nine" })).toThrow(/not a cron expression/);
    expect(() => call("POST", "/schedules", { instruction: "x", every: "1h", agent: "intern" })).toThrow(/no agent intern/);
    expect(() => call("POST", "/schedules", { instruction: "x", every: "1h", agent: "old" })).toThrow(/retired/);
    expect(() => call("GET", "/schedules/nope")).toThrow(/no schedule nope/);

    expect(call("DELETE", `/schedules/${id}`)).toEqual({ ok: true, detail: null });
    expect((call("GET", "/schedules").schedules as unknown[]).length).toBe(0);
    expect(m_handles_nothing_else(call)).toBe(true);
  });
});

function m_handles_nothing_else(call: (method: string, path: string) => unknown): boolean {
  return call("GET", "/watches") === undefined;
}
