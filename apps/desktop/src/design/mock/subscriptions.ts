/**
 * The mock's subscriptions (ADR 0078, `ghost_api::subscriptions`): Claude Code, OpenAI Codex and
 * Google Antigravity as their programs would say, and their installs and sign-ins played out on
 * a short clock. Synthetic lines, addresses and codes only.
 *
 *   ?subs=signed-in   Claude Code signed in, the other two not installed (the default)
 *   ?subs=fresh       none installed
 *   ?subs=signed-out  Claude Code installed and signed out, Codex signed in, Antigravity signed out
 *   ?subs=old         a Claude Code too old for Ghost
 *   &subjob=install|sign-in|code|failed   Claude Code's job held in that state
 *
 * Install takes about three seconds; Claude Code's sign-in asks for a code after one, Codex's
 * signs in by itself after three. Use it on Claude Code makes it the model (`setProvider`).
 */
import type { SubscriptionJobView, SubscriptionsView, SubscriptionView } from "@ghost/client-ts";

import { HttpError } from "./errors";

type Program = { installed: boolean; signed_in: boolean; version: string | null; usable_version: boolean };

const BASE: Record<string, Omit<SubscriptionView, "installed" | "version" | "signed_in" | "usable" | "in_use" | "line" | "note" | "job">> = {
  "claude-code": {
    id: "claude-code",
    title: "Claude Code",
    program: "claude",
    ghost_can_use: true,
    install_command: "curl -fsSL https://claude.ai/install.sh | bash",
    install_source: "https://code.claude.com/docs/en/setup",
    sign_in_command: "claude auth login",
    sign_in_here: true,
  },
  codex: {
    id: "codex",
    title: "OpenAI Codex",
    program: "codex",
    ghost_can_use: false,
    install_command: "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
    install_source: "https://learn.chatgpt.com/docs/codex/cli",
    sign_in_command: "codex login",
    sign_in_here: true,
  },
  antigravity: {
    id: "antigravity",
    title: "Google Antigravity",
    program: "agy",
    ghost_can_use: false,
    install_command: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
    install_source: "https://codelabs.developers.google.com/antigravity-cli-hands-on",
    sign_in_command: "agy",
    sign_in_here: false,
  },
};

const VERSIONS: Record<string, string> = { "claude-code": "2.1.330", codex: "0.166.0", antigravity: "1.4.2" };

function programsFor(mode: string | null): Record<string, Program> {
  const p = (installed: boolean, signed_in: boolean, id: string, usable_version = true): Program => ({ installed, signed_in, version: installed ? VERSIONS[id] : null, usable_version });
  switch (mode) {
    case "fresh":
      return { "claude-code": p(false, false, "claude-code"), codex: p(false, false, "codex"), antigravity: p(false, false, "antigravity") };
    case "signed-out":
      return { "claude-code": p(true, false, "claude-code"), codex: p(true, true, "codex"), antigravity: p(true, false, "antigravity") };
    case "old":
      return { "claude-code": { installed: true, signed_in: true, version: "1.0.31", usable_version: false }, codex: p(false, false, "codex"), antigravity: p(false, false, "antigravity") };
    default:
      return { "claude-code": p(true, true, "claude-code"), codex: p(false, false, "codex"), antigravity: p(false, false, "antigravity") };
  }
}

const now = () => new Date().toISOString();

export class MockSubscriptions {
  private programs: Record<string, Program>;
  private jobs: Record<string, SubscriptionJobView | null> = {};
  private timers = new Set<ReturnType<typeof setTimeout>>();

  constructor(
    params: URLSearchParams,
    private readonly provider: () => string,
    private readonly setProvider: (p: string) => void,
  ) {
    this.programs = programsFor(params.get("subs"));
    const held = params.get("subjob");
    if (held === "install") {
      this.programs["claude-code"] = { installed: false, signed_in: false, version: null, usable_version: true };
      this.jobs["claude-code"] = this.job("install", "running", BASE["claude-code"].install_command, ["Setting up Claude Code...", "Downloading the latest version", "Checking the download"]);
    } else if (held === "sign-in" || held === "code") {
      this.programs["claude-code"].signed_in = false;
      this.jobs["claude-code"] = this.signInJob(held === "code" ? "needs_code" : "running");
    } else if (held === "failed") {
      this.programs["claude-code"].signed_in = false;
      this.jobs["claude-code"] = { ...this.signInJob("failed"), message: "Claude Code’s sign-in ended without signing in (it exited with 1).", ended_at: now(), signed_in: false };
    }
  }

  private job(kind: "install" | "sign_in", state: SubscriptionJobView["state"], command: string, lines: string[]): SubscriptionJobView {
    return { kind, state, command, lines, url: null, device_code: null, message: null, started_at: now(), ended_at: null, signed_in: null };
  }

  private signInJob(state: SubscriptionJobView["state"]): SubscriptionJobView {
    return {
      ...this.job("sign_in", state, "claude auth login", ["Opening browser to sign in…", "If the browser didn't open, visit:", "https://claude.ai/oauth/authorize…", ...(state === "needs_code" ? ["Paste code here if prompted >"] : [])]),
      url: "https://claude.ai/oauth/authorize?code=true&state=mock-state",
    };
  }

  private later(ms: number, f: () => void) {
    const t = setTimeout(() => {
      this.timers.delete(t);
      f();
    }, ms);
    this.timers.add(t);
  }

  view(): SubscriptionsView {
    const provider = this.provider();
    const subscriptions = Object.values(BASE).map((b): SubscriptionView => {
      const p = this.programs[b.id];
      const in_use = b.id === "claude-code" && provider === "claude-code";
      const usable = b.ghost_can_use && p.installed && p.signed_in && p.usable_version;
      return {
        ...b,
        installed: p.installed,
        version: p.version,
        signed_in: p.signed_in,
        usable,
        in_use,
        line:
          b.id === "claude-code" && p.installed
            ? !p.usable_version
              ? "This version of Claude Code (1.0.31) lacks --strict-mcp-config that Ghost needs to run it with its own tools off, so Ghost won't use it. Update it with `claude update` in a terminal, then Look again."
              : p.signed_in
                ? "Signed in with a Claude Max plan. Replies count against its limits."
                : "Claude Code is installed but not signed in."
            : null,
        note: null,
        job: this.jobs[b.id] ?? null,
      };
    });
    return { subscriptions, in_use: subscriptions.find((s) => s.in_use)?.id ?? null };
  }

  handle(method: string, path: string, body: Record<string, unknown>): unknown {
    if (method === "GET" && path === "/subscriptions") return this.view();
    const m = /^\/subscriptions\/([^/]+)\/(job|install|sign-in|sign-in\/code|cancel|use)$/.exec(path);
    if (!m) return undefined;
    const [, id, what] = m;
    const base = BASE[id];
    if (!base) throw new HttpError(404, `no subscription called ${id}`);
    const p = this.programs[id];
    const running = () => {
      const j = this.jobs[id];
      return !!j && (j.state === "running" || j.state === "needs_code");
    };
    if (method === "GET" && what === "job") return this.jobs[id] ?? null;
    if (method === "POST" && what === "install") {
      if (p.installed) throw new HttpError(409, `${base.title} is already installed.`);
      if (running()) throw new HttpError(409, `${base.title} is already being installed or signed in.`);
      const j = this.job("install", "running", base.install_command, [`Setting up ${base.title}...`]);
      this.jobs[id] = j;
      this.later(900, () => (j.lines = [...j.lines, "Downloading the latest version"]));
      this.later(1900, () => (j.lines = [...j.lines, "Checking the download", `Installed to ~/.local/bin/${base.program}`]));
      this.later(3000, () => {
        Object.assign(j, { state: "done", ended_at: now(), message: `${base.title} is installed.` });
        this.programs[id] = { installed: true, signed_in: false, version: VERSIONS[id], usable_version: true };
      });
      return j;
    }
    if (method === "POST" && what === "sign-in") {
      if (!base.sign_in_here) throw new HttpError(409, `${base.title} signs in on a screen of its own, with choices that are yours. Run \`${base.sign_in_command}\` in a terminal and sign in there, then look again.`);
      if (!p.installed) throw new HttpError(404, `${base.title} isn't installed on this computer.`);
      if (running()) throw new HttpError(409, `${base.title} is already being installed or signed in.`);
      if (id === "claude-code") {
        const j = this.signInJob("running");
        this.jobs[id] = j;
        this.later(1000, () => Object.assign(j, { state: "needs_code", lines: [...j.lines, "Paste code here if prompted >"] }));
        return j;
      }
      const j: SubscriptionJobView = {
        ...this.job("sign_in", "running", base.sign_in_command, ["Starting local login server on http://localhost:1455.", "If your browser did not open, navigate to this URL to authenticate:", "https://auth.openai.com/oauth/authorize…"]),
        url: "https://auth.openai.com/oauth/authorize?state=mock-state",
      };
      this.jobs[id] = j;
      this.later(3000, () => {
        Object.assign(j, { state: "done", ended_at: now(), signed_in: true, message: `${base.title} is signed in.`, lines: [...j.lines, "Successfully logged in"] });
        p.signed_in = true;
      });
      return j;
    }
    if (method === "POST" && what === "sign-in/code") {
      const j = this.jobs[id];
      if (!j || j.state !== "needs_code") throw new HttpError(409, `${base.title} isn't waiting for a code.`);
      if (!String(body.code ?? "").trim()) throw new HttpError(422, "The code is empty.");
      Object.assign(j, { state: "running", lines: [...j.lines.slice(0, -1), "Paste code here if prompted > ••••••"] });
      this.later(900, () => {
        Object.assign(j, { state: "done", ended_at: now(), signed_in: true, message: `${base.title} is signed in.`, lines: [...j.lines, "Login successful."] });
        p.signed_in = true;
      });
      return j;
    }
    if (method === "POST" && what === "cancel") {
      const j = this.jobs[id];
      if (!j || !running()) throw new HttpError(404, `Nothing of ${base.title}'s is running.`);
      for (const t of this.timers) clearTimeout(t);
      this.timers.clear();
      Object.assign(j, { state: "cancelled", ended_at: now(), message: "Cancelled." });
      return j;
    }
    if (method === "PUT" && what === "use") {
      if (!base.ghost_can_use) throw new HttpError(409, `Ghost can't use ${base.title} as its model yet.`);
      if (!p.installed || !p.signed_in || !p.usable_version) throw new HttpError(409, `${base.title} isn't ready.`);
      this.setProvider(id);
      return { done: `Ghost uses ${base.title} now.`, restart_required: false, subscriptions: this.view() };
    }
    return undefined;
  }
}
