import { describe, expect, it } from "vitest";

import { MockSchedules } from "../src/design/mock/schedules";
import { OUTCOME, cronWords, intervalWords, madeBy, runErrorWords, untilWords, whenWords } from "../src/screens/watching/model";

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
    // A cron in plain words, the expression kept as detail; one Ghost can't read is shown as written.
    expect(whenWords({ ...base, kind: "cron", cron: "0 9 * * 1-5" }, "America/Detroit")).toEqual({ text: "Weekdays at 9 AM", detail: "0 9 * * 1-5" });
    expect(whenWords({ ...base, kind: "cron", cron: "0 9 * * 1-5", time_zone: "Europe/London" }, "America/Detroit")).toEqual({ text: "Weekdays at 9 AM, Europe/London time", detail: "0 9 * * 1-5" });
    expect(whenWords({ ...base, kind: "cron", cron: "0 9 1-7 * 1" }, "America/Detroit")).toEqual({ text: "On", mono: "0 9 1-7 * 1" });
    expect(whenWords({ ...base, kind: "at", at: "2031-12-01T14:00:00Z" }, "America/Detroit").text).toMatch(/^Once, Dec 1, 2031 at /);
  });

  it("names who set it up and how each run ended", () => {
    expect(madeBy("operator")).toEqual({ you: true });
    expect(madeBy("agent:ledger")).toEqual({ you: false, slug: "ledger" });
    expect(madeBy("client:claude-code")).toEqual({ you: false, tool: "claude-code" });
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

describe("cronWords", () => {
  it("reads the common shapes in words", () => {
    expect(cronWords("0 9 * * *")).toBe("Every day at 9 AM");
    expect(cronWords("30 17 * * 1-5")).toBe("Weekdays at 5:30 PM");
    expect(cronWords("0 15 * * 5")).toBe("Fridays at 3 PM");
    expect(cronWords("0 8 * * 1,3")).toBe("Mondays and Wednesdays at 8 AM");
    expect(cronWords("0 10 * * 0,6")).toBe("Weekends at 10 AM");
    expect(cronWords("0 9 1 * *")).toBe("The 1st of every month at 9 AM");
    expect(cronWords("*/15 * * * *")).toBe("Every 15 minutes");
    expect(cronWords("0 */2 * * *")).toBe("Every 2 hours");
    expect(cronWords("5 * * * *")).toBe("Every hour at :05");
  });
  it("gives up on anything else rather than guess", () => {
    expect(cronWords("0 9 1-7 * 1")).toBeNull();
    expect(cronWords("0 9 * 6 *")).toBeNull();
    expect(cronWords("nonsense")).toBeNull();
  });
});

describe("a run's failure in plain words", () => {
  it("names the common causes and keeps the first sentence of the rest", () => {
    expect(runErrorWords("HTTP 529: the model is overloaded")).toBe("the model was overloaded");
    expect(runErrorWords("HTTP 429 Too Many Requests")).toBe("the model’s rate limit was reached");
    expect(runErrorWords("it was still working after 10 minutes and was stopped")).toBe("it ran out of time");
    expect(runErrorWords("the agent analytics is retired")).toBe("its agent is retired");
    expect(runErrorWords("GitHub is not connected (signed out)")).toBe("an app it needed wasn’t connected");
    expect(runErrorWords("Something odd happened. More detail here.")).toBe("Something odd happened");
  });
});
