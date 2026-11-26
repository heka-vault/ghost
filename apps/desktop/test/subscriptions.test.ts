import type { SubscriptionJobView, SubscriptionView } from "@ghost/client-ts";
import { describe, expect, it, vi } from "vitest";

import { MockSubscriptions } from "../src/design/mock/subscriptions";
import { tidyModelProblem } from "../src/screens/settings/dictation";
import { otherProviderOf } from "../src/screens/settings/ModelPage";
import { QUICK_LINE, THINKS_LINE, cardFace, codeProblem, endedWords, inUseLine, installWords, jobRunning, modelChoices } from "../src/screens/settings/subscriptions";

const sub = (o: Partial<SubscriptionView> = {}): SubscriptionView => ({
  id: "claude-code",
  title: "Claude Code",
  program: "claude",
  installed: true,
  version: "2.1.330",
  signed_in: true,
  usable: true,
  in_use: false,
  ghost_can_use: true,
  install_command: "curl -fsSL https://claude.ai/install.sh | bash",
  install_source: "https://code.claude.com/docs/en/setup",
  sign_in_command: "claude auth login",
  sign_in_here: true,
  line: null,
  note: null,
  job: null,
  ...o,
});

const job = (o: Partial<SubscriptionJobView> = {}): SubscriptionJobView => ({
  kind: "sign_in",
  state: "running",
  command: "claude auth login",
  lines: [],
  url: null,
  device_code: null,
  message: null,
  started_at: "2026-11-25T10:00:00Z",
  ended_at: null,
  signed_in: null,
  ...o,
});

describe("a subscription's card (ADR 0078)", () => {
  it("has one action for each state, in order: install, sign in, use", () => {
    expect(cardFace(sub({ installed: false, signed_in: false, version: null }))).toMatchObject({ action: "install", label: "Install", badge: "Not installed" });
    expect(cardFace(sub({ signed_in: false, usable: false }))).toMatchObject({ action: "sign_in", label: "Sign in", tone: "warn" });
    expect(cardFace(sub())).toMatchObject({ action: "use", label: "Use it" });
    expect(cardFace(sub({ in_use: true }))).toMatchObject({ action: "in_use", label: null, badge: "Ghost uses this" });
  });
  it("says Coming next for a program Ghost can't run yet, and a terminal for Antigravity's sign-in", () => {
    const codex = sub({ id: "codex", title: "OpenAI Codex", ghost_can_use: false, usable: false });
    expect(cardFace(codex)).toMatchObject({ action: "coming_next", label: "Coming next" });
    const agy = sub({ id: "antigravity", title: "Google Antigravity", sign_in_here: false, signed_in: false, sign_in_command: "agy", ghost_can_use: false });
    const face = cardFace(agy);
    expect(face).toMatchObject({ action: "terminal", label: "Sign in" });
    expect(face.line).toContain("Ghost opens a terminal running agy");
    // After Ghost opened it: look whether it is signed in now.
    const opened = cardFace({ ...agy, job: job({ kind: "sign_in", state: "done", message: "Ghost opened a terminal running agy. Sign in there, then look again." }) });
    expect(opened).toMatchObject({ action: "look", label: "Look again" });
    expect(opened.line).toContain("Ghost opened a terminal running agy");
  });
  it("says what stops Ghost for a version it won't run, and shows a job going on", () => {
    expect(cardFace(sub({ usable: false, line: "This version of Claude Code (1.0.31) lacks --effort" }))).toMatchObject({ action: "fix", line: "This version of Claude Code (1.0.31) lacks --effort" });
    expect(cardFace(sub({ job: job({ state: "needs_code" }) }))).toMatchObject({ action: "busy", badge: "Signing in", label: null });
    expect(cardFace(sub({ installed: false, job: job({ kind: "install" }) })).badge).toBe("Installing");
    expect(jobRunning(job({ state: "done" }))).toBe(false);
  });
  it("never words a sign-in as Ghost's own or a vendor's endorsement", () => {
    const all = [sub({ signed_in: false }), sub(), sub({ installed: false }), sub({ in_use: true })].map((s) => JSON.stringify(cardFace(s))).join(" ");
    expect(all).not.toMatch(/Sign in with (Claude|Google|ChatGPT)/);
    expect(all).not.toMatch(/free|unlimited|partner|official/i);
    expect(installWords(sub()).body).toContain("own installer");
  });
  it("says plainly which one Ghost uses", () => {
    expect(inUseLine([sub({ in_use: true })], null)).toBe("Ghost uses your Claude Code now.");
    expect(inUseLine([sub()], "Anthropic")).toBe("Ghost uses Anthropic now, set up another way. Choose a subscription to switch.");
    expect(inUseLine([sub()], null)).toBe("Ghost has no model yet. Choose one of your subscriptions.");
  });
  it("checks a pasted code as a line, and words how a job ended", () => {
    expect(codeProblem(" ")).not.toBeNull();
    expect(codeProblem("a\nb")).not.toBeNull();
    expect(codeProblem("x".repeat(600))).not.toBeNull();
    expect(codeProblem("abc#123")).toBeNull();
    expect(endedWords("Claude Code", job({ state: "done" }))).toEqual({ title: "Claude Code is signed in", tone: "ok" });
    expect(endedWords("Codex", job({ kind: "install", state: "failed" }))?.title).toBe("Codex didn’t install");
    expect(endedWords("Codex", job())).toBeNull();
  });
});

describe("Settings › Model keeps a connection made another way folded away", () => {
  const conn = (provider: string, ready: boolean) => ({ provider, ready, providers: [{ id: "anthropic", title: "Anthropic" }] }) as never;
  it("names it only when it is set up and is not Claude Code", () => {
    expect(otherProviderOf({ values: {} }, conn("anthropic", false))).toBeNull();
    expect(otherProviderOf({ values: { GHOST_LLM_PROVIDER: "anthropic" } }, conn("anthropic", false))).toBe("Anthropic");
    expect(otherProviderOf({ values: {} }, conn("anthropic", true))).toBe("Anthropic");
    expect(otherProviderOf({ values: { GHOST_LLM_PROVIDER: "claude-code" } }, conn("claude-code", true))).toBeNull();
  });
});

describe("the mock's subscriptions play out", () => {
  it("installs, signs in with a pasted code, and becomes the model", () => {
    vi.useFakeTimers();
    let provider = "";
    const m = new MockSubscriptions(new URLSearchParams("subs=fresh"), () => provider, (p) => (provider = p));
    expect(m.view().subscriptions.every((s) => !s.installed)).toBe(true);
    m.handle("POST", "/subscriptions/claude-code/install", {});
    vi.advanceTimersByTime(3100);
    expect(m.view().subscriptions[0].installed).toBe(true);
    m.handle("POST", "/subscriptions/claude-code/sign-in", {});
    vi.advanceTimersByTime(1100);
    expect(m.view().subscriptions[0].job?.state).toBe("needs_code");
    m.handle("POST", "/subscriptions/claude-code/sign-in/code", { code: "mock-code-1" });
    vi.advanceTimersByTime(1000);
    expect(JSON.stringify(m.view())).not.toContain("mock-code-1");
    expect(m.view().subscriptions[0].signed_in).toBe(true);
    m.handle("PUT", "/subscriptions/claude-code/use", {});
    expect(provider).toBe("claude-code");
    expect(m.view().in_use).toBe("claude-code");
    expect(() => m.handle("PUT", "/subscriptions/codex/use", {})).toThrow(/can't use OpenAI Codex/);
    vi.useRealTimers();
  });
});

describe("dictation's tidy on OpenRouter", () => {
  it("checks a model id", () => {
    expect(tidyModelProblem("")).toBeNull();
    expect(tidyModelProblem("meta-llama/llama-3.1-8b-instruct:nitro")).toBeNull();
    expect(tidyModelProblem("not a model")).not.toBeNull();
  });
});

describe("Ghost thinks with, and quick first replies with", () => {
  const providers = [
    { id: "claude-code", title: "Claude Code", program: true },
    { id: "anthropic", title: "Anthropic", program: false },
  ] as never;
  const conn = (o: object) => ({ provider: "claude-code", wire_model: "haiku", fast_model: null, fast_wire_model: null, providers, ...o });

  it("offers Claude Code's three models and shows the one Ghost uses now", () => {
    const c = modelChoices(conn({ wire_model: "sonnet" }));
    expect(c.thinks.options.map((o) => o.label)).toEqual(["Opus", "Sonnet", "Haiku"]);
    expect(c.thinks.value).toBe("sonnet");
    // A model saved some other way is shown as itself, not lost.
    const other = modelChoices(conn({ wire_model: "claude-opus-5-5" }));
    expect(other.thinks.value).toBe("claude-opus-5-5");
    expect(other.thinks.options.at(-1)).toEqual({ value: "claude-opus-5-5", label: "claude-opus-5-5" });
  });

  it("offers None, Haiku or Sonnet first, and says why Claude Code has none", () => {
    const c = modelChoices(conn({ fast_model: "haiku" }));
    expect(c.quick.options.map((o) => o.label)).toEqual(["None", "Haiku", "Sonnet"]);
    // Claude Code answers every turn itself (ADR 0069): None, and the select says why.
    expect(c.quick).toMatchObject({ value: "", disabled: true });
    expect(c.quick.note).toBe("Not with Claude Code yet: it answers every turn itself.");
    // A provider with a fast model shows it, enabled.
    const direct = modelChoices(conn({ provider: "anthropic", wire_model: "sonnet", fast_model: "haiku", fast_wire_model: "haiku" }));
    expect(direct.quick).toMatchObject({ value: "haiku", disabled: false, note: null });
    expect(THINKS_LINE).toBe("Answers you and does the work.");
    expect(QUICK_LINE).toBe("Answers first while Ghost’s model takes over when it’s needed.");
  });
});

describe("Continue with the one subscription that's ready", () => {
  it("is the one signed-in subscription Ghost can run, while Ghost has no model", async () => {
    const { onlyReady } = await import("../src/screens/settings/subscriptions");
    const claude = sub({ id: "claude-code", title: "Claude Code", signed_in: true, ghost_can_use: true, usable: true });
    const codex = sub({ id: "codex", title: "OpenAI Codex", signed_in: true, ghost_can_use: false });
    expect(onlyReady({ in_use: null, subscriptions: [claude, codex] })?.id).toBe("claude-code");
    // Already has a model, or none signed in, or two to choose from: the person picks.
    expect(onlyReady({ in_use: "claude-code", subscriptions: [claude] })).toBeNull();
    expect(onlyReady({ in_use: null, subscriptions: [{ ...claude, signed_in: false }, codex] })).toBeNull();
    expect(onlyReady({ in_use: null, subscriptions: [claude, { ...codex, ghost_can_use: true, usable: true }] })).toBeNull();
    expect(onlyReady(null)).toBeNull();
  });
});
