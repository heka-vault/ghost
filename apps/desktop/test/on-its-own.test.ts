import type { ScheduleRunInfo } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { schedules, watching } from "../src/design/mock/fixtures/folder";
import { scenarioByName } from "../src/design/mock/scenarios";
import { repliesTo } from "../src/screens/watching/AskGhost";
import { problemOf } from "../src/screens/watching/problems";
import { FILE_OWNS, HOOK_OWNS, QUOTE_MAX, READ_MAX, countWords, doingsOf, goingNow, goingWords, ownersOf, placeProblems, quoteOf, readWords, sentenceOf, sentenceText } from "../src/screens/watching/sentence";

const names: Record<string, string> = { ghost: "Ghost", accounting: "Accounting", ledger: "Ledger" };
const ctx = {
  zone: "America/Detroit",
  locale: "en-US",
  agentName: (s: string) => names[s] ?? s,
  appTitle: (n: string) => ({ github: "GitHub", stripe: "Stripe", calendar: "Calendar", mail: "Mail" })[n] ?? n,
};

/** The busy scenario's whole list: every kind of row, broken and paused ones included. */
function busyList() {
  const s = scenarioByName("busy");
  const w = watching(s);
  return doingsOf(schedules(s).view.schedules, w.watches, w.hooks);
}

describe("On its own: one list, one sentence per row", () => {
  it("says a schedule as when, who and what it was asked, the asked words apart", () => {
    const d = busyList().find((x) => x.kind === "schedule" && x.s.name === "morning build check")!;
    const s = sentenceOf(d, ctx);
    expect(s.group).toBe("time");
    expect(s.trigger).toBe("Weekdays at 9 AM");
    expect(s.what).toBe("Ghost");
    expect(s.quote).toBe("check the build on main in ghost-mock/app");
    expect(sentenceText(s)).toBe("Weekdays at 9 AM: Ghost, check the build on main in ghost-mock/app");
    expect(s.state).toMatch(/^Tells you with a notification · next in \d+ h · last ran \d+ h ago, done$/);
    expect(s.badge).toBeNull();
    expect(s.pause).toEqual({ on: true, why: null });
  });

  it("says an agent's schedule with its hours in the locale's clock and its check, by the app's title", () => {
    const d = busyList().find((x) => x.kind === "schedule" && x.s.name === "vendor inbox")!;
    const s = sentenceOf(d, ctx);
    expect(s.trigger).toBe("Every 2 hours, between 8 AM and 6 PM");
    expect(sentenceOf(d, { ...ctx, locale: "en-GB" }).trigger).toBe("Every 2 hours, between 08:00 and 18:00");
    expect(s.state).toMatch(/^Tells you in the summary, only when Mail has something new · /);
    expect(s.agent).toBe("accounting");
  });

  it("says a paused and a failed schedule in its badge", () => {
    const list = busyList().filter((x) => x.kind === "schedule");
    const badges = list.map((d) => sentenceOf(d, ctx).badge?.label ?? null);
    expect(badges).toContain("Paused");
    expect(badges.some((b) => b === "Failed" || b === "Ran out of time" || b === "Cut short")).toBe(true);
  });

  it("tells watches apart by what they look for, from the read's name, bounded", () => {
    const rows = busyList().filter((x) => x.kind === "watch").map((d) => sentenceOf(d, ctx));
    const ci = rows.find((r) => r.trigger === "New workflow runs in GitHub")!;
    expect(ci.group).toBe("event");
    expect(ci.what).toBe("Ghost tells you with a notification");
    expect(ci.state).toMatch(/^Looked (now|1 min ago) · 1 open$/);
    // The three GitHub watches no longer read alike.
    const github = rows.filter((r) => r.trigger.endsWith("in GitHub")).map((r) => r.trigger);
    expect(new Set(github).size).toBe(github.length);
    expect(readWords("list_workflow_runs")).toBe("workflow runs");
    expect(readWords("search_pull_requests")).toBe("pull requests");
    expect(readWords("???")).toBe("something");
    expect(readWords(`list_${"x_".repeat(60)}`).length).toBeLessThanOrEqual(READ_MAX);
    // The operator's own watch has a switch this window can't flip, and says why.
    expect(ci.pause.why).toBe(FILE_OWNS);
  });

  it("says a watch that acts with the agent that looks into each one", () => {
    const s = sentenceOf(busyList().find((x) => x.kind === "watch" && x.w.name === "failed-refunds")!, ctx);
    expect(s.trigger).toBe("New refunds in Stripe");
    expect(s.what).toBe("Ledger looks into each, then Ghost tells you with a notification");
    expect(s.agent).toBe("ledger");
    expect(s.state).toMatch(/set up by Ledger$/);
    expect(s.pause.why).toBeNull();
  });

  it("says a watch that keeps a record, one that can't read and one that is paused", () => {
    const list = busyList();
    const cal = sentenceOf(list.find((x) => x.kind === "watch" && x.w.name === "standup-moved")!, ctx);
    expect(cal.trigger).toBe("New events in Calendar");
    expect(cal.what).toBe("Ghost keeps a record");
    expect(cal.badge).toEqual({ label: "Can’t read", tone: "danger" });
    // The app's own error stays in the details: the row says only when it last worked.
    expect(cal.state).toMatch(/^Last worked \d+ h ago$/);
    expect(cal.state).not.toContain("sign in again");
    const paused = sentenceOf(list.find((x) => x.kind === "watch" && x.w.name === "slow-deploys")!, ctx);
    expect(paused.state).toMatch(/^Paused/);
    expect(paused.badge?.label).toBe("Paused");
  });

  it("says a webhook by its source and events, and one that is off, never a password's name", () => {
    const list = busyList();
    const gh = sentenceOf(list.find((x) => x.kind === "hook" && x.h.name === "deploys")!, ctx);
    expect(gh.trigger).toBe("When GitHub sends workflow run or deployment status");
    expect(gh.what).toBe("Ghost tells you");
    expect(gh.state).toMatch(/^Last heard from \d+ min ago$/);
    const jobs = sentenceOf(list.find((x) => x.kind === "hook" && x.h.name === "jobs")!, ctx);
    expect(jobs.trigger).toBe("When Jobs sends job failed or job ok");
    const off = sentenceOf(list.find((x) => x.kind === "hook" && x.h.name === "stripe")!, ctx);
    expect(off.badge?.label).toBe("Off");
    expect(off.state).toBe("Off until its password is set");
    expect(off.pause.why).toBe(HOOK_OWNS);
  });

  it("gives every row a switch, and a reason wherever this window can't flip it", () => {
    for (const d of busyList()) {
      const s = sentenceOf(d, ctx);
      expect(typeof s.pause.on).toBe("boolean");
      if (d.kind === "hook" || (d.kind === "watch" && d.w.made_by == null)) expect(s.pause.why).toBeTruthy();
      else expect(s.pause.why).toBeNull();
    }
  });

  it("puts each problem in its row, or a row of its own in its group, never a box above the list", () => {
    const busy = scenarioByName("busy");
    const problems = [...schedules(busy).view.problems, ...watching(busy).problems].map((r) => problemOf(r, { appTitle: (n) => n, agentName: (n) => n }));
    const placed = placeProblems(busyList(), problems);
    expect(placed.byKey.size + placed.rows.length).toBe(problems.length);
    expect(placed.rows.map((r) => [r.group, r.problem.subject?.name, r.problem.chip])).toEqual([
      ["time", "weekly numbers", "Can’t run"],
      ["event", "invoices", "Can’t run"],
      ["event", "billing", "Off"],
    ]);
    // A problem about a row on the list goes to that row.
    const ci = busyList().find((x) => x.kind === "watch" && x.w.name === "ci")!;
    const own = placeProblems(busyList(), [problemOf("~/.config/ghost/watches.json: watch ci: app \"github\" isn't connected", { appTitle: (n) => n, agentName: (n) => n })]);
    expect(own.byKey.get(ci.key)?.chip).toBe("Can’t run");
    expect(own.rows).toEqual([]);
  });

  it("quotes only the first sentence of an instruction, bounded", () => {
    expect(quoteOf("Check the build. Then tell me.")).toBe("check the build");
    expect(quoteOf("GitHub first, then the rest")).toBe("GitHub first, then the rest");
    expect(quoteOf("I want the PRs")).toBe("I want the PRs");
    const long = quoteOf("Look ".repeat(60));
    expect(long.length).toBeLessThanOrEqual(QUOTE_MAX);
    expect(long.endsWith("…")).toBe(true);
    expect(quoteOf("Line one\nline two? More")).toBe("line one line two");
  });

  it("holds every kind of row in one list, schedules then watches then webhooks", () => {
    const kinds = busyList().map((d) => d.kind);
    expect(kinds.indexOf("watch")).toBeGreaterThan(kinds.lastIndexOf("schedule"));
    expect(kinds.indexOf("hook")).toBeGreaterThan(kinds.lastIndexOf("watch"));
    expect(new Set(busyList().map((d) => d.key)).size).toBe(busyList().length);
  });

  it("counts in words and lists every owner, Ghost first", () => {
    expect(countWords(0)).toBe("Nothing yet");
    expect(countWords(1)).toBe("Ghost does 1 thing on its own");
    expect(countWords(3)).toBe("Ghost does 3 things on its own");
    expect(ownersOf(busyList())[0]).toBe("ghost");
    expect(ownersOf(busyList())).toEqual(expect.arrayContaining(["accounting", "ledger"]));
  });

  it("puts what is running now at the top, oldest first, in words", () => {
    const r = (id: string, status: ScheduleRunInfo["status"], started: string, source = "schedule") => ({ id, status, started_at: started, scheduled_for: started, agent: "ledger", name: "vendor inbox", source }) as ScheduleRunInfo;
    const going = goingNow([r("a", "ok", "2026-11-14T10:00:00Z"), r("b", "running", "2026-11-14T10:05:00Z"), r("c", "queued", "2026-11-14T10:01:00Z")]);
    expect(going.map((x) => x.id)).toEqual(["c", "b"]);
    expect(goingWords(going[1], ctx.agentName)).toBe("Ledger is running vendor inbox");
    expect(goingWords(r("d", "running", "", "watch:failed-refunds"), ctx.agentName)).toBe("Ledger is looking into something new for failed-refunds");
  });

  it("an empty Ghost has nothing on the list", () => {
    const s = scenarioByName("empty");
    const w = watching(s);
    expect(doingsOf(schedules(s).view.schedules, w.watches, w.hooks)).toEqual([]);
  });
});

describe("asking Ghost to do something on its own", () => {
  it("shows Ghost's replies to the operator's own words, and nothing before them", () => {
    const turns: { role: "user" | "assistant"; text: string }[] = [
      { role: "assistant", text: "Ghost here." },
      { role: "user", text: "Every weekday at 9, check the build" },
      { role: "assistant", text: "Done. Weekdays at 9 AM I check the build." },
      { role: "assistant", text: "It’s on the list." },
      { role: "user", text: "Thanks" },
      { role: "assistant", text: "Anytime." },
    ];
    expect(repliesTo(turns, " Every weekday at 9, check the build ")).toEqual(["Done. Weekdays at 9 AM I check the build.", "It’s on the list."]);
    expect(repliesTo(turns, "something never said")).toEqual([]);
  });
});

describe("what can't run, said for a newcomer", () => {
  const c = { appTitle: (n: string) => ({ quickbooks: "QuickBooks" })[n] ?? n, agentName: (s: string) => ({ analytics: "Analytics" })[s] ?? s };
  it("says each known kind in a plain sentence, with its fix, the line kept for Details", () => {
    const busy = scenarioByName("busy");
    const raw = [...schedules(busy).view.problems, ...watching(busy).problems];
    const said = raw.map((r) => problemOf(r, c));
    expect(said.map((p) => p.sentence)).toEqual([
      "The weekly numbers schedule can’t run: Analytics is retired.",
      "The invoices watch can’t run: QuickBooks isn’t connected.",
      "The billing webhook is off: Ghost can’t read how it is set up.",
    ]);
    expect(said.map((p) => p.fix?.path ?? null)).toEqual(["/agents", "/apps", null]);
    expect(said[1].fix?.label).toBe("Connect QuickBooks");
    // The raw line and its file stay for Details, never in the sentence.
    expect(said[1].detail).toBe(raw[1]);
    expect(said[1].file).toBe("~/.config/ghost/watches.json");
    for (const p of said) expect(p.sentence).not.toMatch(/\.json|~\/|"/);
  });
  it("reads the daemon's own shapes", () => {
    expect(problemOf("/home/x/.config/ghost/watches.json: watch ci: \"every\" must be a number; the watch is off", c).sentence).toBe("The ci watch is off: Ghost can’t read how it is set up.");
    expect(problemOf("hook stripe is off: its password STRIPE_HOOK_SECRET is not set", c)).toMatchObject({ sentence: "The stripe webhook is off: its password isn’t set.", fix: { path: "/settings/developer/passwords" }, chip: "Off", reason: "Its password isn’t set." });
    expect(problemOf("hook jobs is off: the recorded delivery bad.json was accepted", c).sentence).toBe("The jobs webhook is off: what it was tested with doesn’t check out.");
    expect(problemOf("/home/x/.config/ghost/hooks.json: expected value at line 1 column 1", c).sentence).toBe("Your webhooks file can’t be read, so nothing in it runs.");
    expect(problemOf("The schedule nightly is for ops, which is not an agent Ghost has; it will not run", c).sentence).toBe("The nightly schedule can’t run: there is no agent called ops.");
  });
  it("says an unknown kind as something that can't be read, and bounds every name", () => {
    expect(problemOf("a line Ghost has never written", c)).toMatchObject({ sentence: "Something in your setup can’t be read.", fix: null });
    const long = problemOf(`~/.config/ghost/watches.json: watch ${"x".repeat(200)}: bad; the watch is off`, c).sentence;
    expect(long.length).toBeLessThan(120);
  });
});
