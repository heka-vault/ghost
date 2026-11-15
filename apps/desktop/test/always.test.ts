import type { AlwaysOffer, AlwaysResult, PermissionChoiceView, PolicyView, WaitingView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { alwaysOffer, scopeWords } from "../src/design/mock/always";
import { MockDaemon } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";
import { decidedLine, decidedMeta } from "../src/screens/policy/model";
import { ALWAYS_TOGGLE, ALWAYS_WHERE, alwaysDone, alwaysWords } from "../src/ui/always";

const offer: AlwaysOffer = { rule: "github.issue_write", scope: "“issue write” in github", allow: true };
const words = (s: string) => s.split(/\s+/).filter(Boolean).length;

describe("the card's sticky choice (ADR 0070)", () => {
  it("says the tool and the app, and nothing else", () => {
    const w = alwaysWords(offer);
    expect(w.yes).toBe("Yes, and don’t ask again for “issue write” in github");
    expect(w.no).toBe("No, and never allow “issue write” in github");
    expect(w.held).toBeNull();
  });

  it("keeps the decisive moment short: the card's own words stay within about 25", () => {
    // What is on the card before "More choices" opens: the state line, a summary, the buttons.
    const summary = "On ghost-mock/app#17: close the issue.";
    const shown = ["Waiting for your okay · 0:42", summary, "Confirm", "Cancel", ALWAYS_TOGGLE].join(" ");
    expect(words(shown)).toBeLessThanOrEqual(25);
    // Each sticky choice is one line of its own, short.
    const w = alwaysWords(offer);
    expect(words(w.yes!)).toBeLessThanOrEqual(12);
    expect(words(w.no)).toBeLessThanOrEqual(12);
  });

  it("says why instead of offering an allow a more careful rule would overrule", () => {
    const held = alwaysWords({ ...offer, allow: false, held: "Your rules ask for this every time: change that in Permissions." });
    expect(held.yes).toBeNull();
    expect(held.held).toContain("change that in Permissions");
    expect(held.no).toContain("never");
  });

  it("says where to take it back, once, on the confirmation", () => {
    const yes = alwaysDone("allow", offer.scope);
    expect(yes.title).toBe("Done. “Issue write” in github runs without asking from now on.");
    expect(yes.description).toBe(ALWAYS_WHERE);
    expect(`${yes.title} ${yes.description}`.split("Settings › Permissions").length).toBe(2);
    expect(alwaysDone("block", offer.scope).title).toContain("won’t run");
  });
});

describe("the mock's offer is the daemon's", () => {
  it("names one tool by its own name, in the daemon's words", () => {
    expect(scopeWords("github", "issue_write")).toBe("“issue write” in github");
    expect(scopeWords("linear", "createIssue")).toBe("“create issue” in linear");
    expect(scopeWords("ghost", "notify")).toBe("“notify” in Ghost");
    const p = { path: "", mode: "open" as const, block: [], ask: [], quiet: [], allow: [] };
    expect(alwaysOffer(p, "github", "delete_*")).toBeNull();
    expect(alwaysOffer(p, "github", "close")).toEqual({ rule: "github.close", scope: "“close” in github", allow: true });
    expect(alwaysOffer({ ...p, ask: ["github.*"] }, "github", "close")?.allow).toBe(false);
  });
});

describe("Settings › Permissions: what you've decided", () => {
  it("reads as a line with when and how", () => {
    expect(decidedLine("“close” in github runs without asking")).toBe("“Close” in github runs without asking");
    const c = { choice: "allow" as const, set_at: "2026-11-14T15:00:00Z", in_file: true };
    expect(decidedMeta(c, "en-GB")).toBe("set 14 Nov from your yes");
    expect(decidedMeta({ ...c, choice: "block", in_file: false }, "en-GB")).toBe("set 14 Nov from your no · no longer in your policy file");
  });
});

describe("the mock daemon's sticky decisions", () => {
  const q = new URLSearchParams();
  const daemon = (name: string) => {
    const s = scenarioByName(name);
    return new MockDaemon(s, composeFixtures(s), {});
  };
  const changeOf = (w: WaitingView) => w.items.find((i) => i.change)!.change!;

  it("offers the waiting change's tool, makes a yes stick and takes it back out", async () => {
    const d = daemon("always");
    const before = (await d.handle("GET", "/permissions/choices", q, null)) as PermissionChoiceView[];
    expect(before.map((c) => c.choice)).toEqual(["allow", "block"]);
    const change = changeOf((await d.handle("GET", "/waiting", q, null)) as WaitingView);
    expect(change.always).toEqual({ rule: "github.close", scope: "“close” in github", allow: true });
    const r = (await d.handle("POST", `/proposals/${change.id}/always`, q, { choice: "allow" })) as AlwaysResult;
    expect(r.outcome?.status).toBe("success");
    expect(r.choice.sentence).toBe("“close” in github runs without asking");
    let policy = (await d.handle("GET", "/policy", q, null)) as PolicyView;
    expect(policy.permissions.allow).toContain("github.close");
    expect(((await d.handle("GET", "/waiting", q, null)) as WaitingView).items.some((i) => i.change?.id === change.id)).toBe(false);
    await d.handle("DELETE", `/permissions/choices/${r.choice.id}`, q, null);
    policy = (await d.handle("GET", "/policy", q, null)) as PolicyView;
    expect(policy.permissions.allow).not.toContain("github.close");
    expect(((await d.handle("GET", "/permissions/choices", q, null)) as PermissionChoiceView[]).length).toBe(2);
  });

  it("refuses an allow a careful rule holds back, and still takes a no", async () => {
    const d = daemon("always-held");
    const change = changeOf((await d.handle("GET", "/waiting", q, null)) as WaitingView);
    expect(change.always?.allow).toBe(false);
    await expect(d.handle("POST", `/proposals/${change.id}/always`, q, { choice: "allow" })).rejects.toThrow(/ask for this every time/);
    const r = (await d.handle("POST", `/proposals/${change.id}/always`, q, { choice: "block" })) as AlwaysResult;
    expect(r.outcome).toBeUndefined();
    expect(((await d.handle("GET", "/policy", q, null)) as PolicyView).permissions.block).toContain("github.close");
  });

  it("starts empty where nothing was decided", async () => {
    expect(await daemon("passive").handle("GET", "/permissions/choices", q, null)).toEqual([]);
  });
});

describe("the TOML view never writes over a rule set meanwhile", () => {
  it("refuses a save made from an older copy of the file", async () => {
    const q = new URLSearchParams();
    const s = scenarioByName("always");
    const d = new MockDaemon(s, composeFixtures(s), {});
    const opened = ((await d.handle("GET", "/policy", q, null)) as PolicyView).text;
    const change = ((await d.handle("GET", "/waiting", q, null)) as WaitingView).items.find((i) => i.change)!.change!;
    await d.handle("POST", `/proposals/${change.id}/always`, q, { choice: "block" });
    await expect(d.handle("PUT", "/policy", q, { text: opened, base: opened })).rejects.toThrow(/changed since you opened it/);
    expect(((await d.handle("GET", "/policy", q, null)) as PolicyView).permissions.block).toContain("github.close");
  });
});
