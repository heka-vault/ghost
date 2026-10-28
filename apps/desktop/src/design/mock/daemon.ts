/**
 * An in-memory ghostd: every route the client uses, plus the event stream. Mutations
 * update this state and emit the same events the real daemon would, so screens behave
 * as they will against the real thing. Foundation-owned; data lives in fixtures/.
 */
import type {
  AppConnect,
  AppConnectResult,
  AppView,
  AuditEntry,
  CatalogEntryView,
  CatalogPage,
  ConversationDetail,
  ConversationView,
  DevicesView,
  GhostEvent,
  ModelsView,
  OutcomeView,
  OutputInfo,
  PendingProposal,
  PermissionsUpdate,
  PermissionsView,
  PhonePairing,
  PhoneView,
  PolicyUpdate,
  PolicyView,
  RulesView,
  SettingsUpdate,
  SettingsView,
  SkillInfo,
  SourceInfo,
  Status,
  Turn,
  VoiceInfo,
  WaitingItemView,
  WatchingView,
} from "@ghost/client-ts";
import { pairLink } from "@ghost/client-ts";

import { DESKTOP_VERSION } from "../../app/version";
import { HttpError } from "./errors";
import { type AppFixture, type CatalogSeed, GENERIC_TOOLS, TOOLS_BY_CATALOG } from "./fixtures/apps";
import type { AgentRecord } from "./fixtures/conversations";
import { CONFIG_DIR } from "./fixtures/folder";
import type { Fixtures } from "./fixtures";
import { renderPolicy } from "./fixtures/policy";
import type { SessionFixture } from "./fixtures/session";
import { sourceOf } from "./fixtures/sources";
import { AUTONOMY, LISTS, conditionalFor, levelOf, parseRule, readPolicyText, ruleText, withRule } from "./permissions";
import { GHOST_SKILLS_DIR, type SkillFixture, missingOf } from "./fixtures/skills";
import { hex, type Scenario, seeded } from "./scenarios";

export { HttpError };

type Listener = (e: GhostEvent | Status) => void;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const rand = seeded(Date.now() & 0xffff);
const newId = () => hex(32, rand);
const now = () => new Date().toISOString();

/**
 * The steps the gate records for a write, worded as the gate's own describe() does: "closed the issue", "comment: …", "labels: …". A comment carried in the summary
 * ("…, with a comment: “…”") becomes its own step.
 */
function stepsOf(operation: string, summary: string): string[] {
  const comment = /with a comment:\s*[“"]?([\s\S]*?)[”"]?\.?$/.exec(summary)?.[1]?.trim();
  const main: Record<string, string | null> = { close: "closed the issue", reopen: "open the issue", comment: null, approve: "approve", request_changes: "request_changes", set_labels: "set_labels" };
  // The comment lands first, then the state change, as the declaration orders its steps.
  const steps: string[] = [];
  if (comment) steps.push(`comment: ${comment}`);
  const last = operation in main ? main[operation] : operation;
  if (last) steps.push(last);
  return steps.length ? steps : [operation];
}

/** “ghost-mock/app#151” as Ghost says it: “app 151”. */
function shortName(target: string): string {
  return target.replace("ghost-mock/", "").replace("#", " ");
}

/**
 * The script's canned proposal, aimed at the issue the operator typed: “close 151 as a
 * duplicate of 148” proposes ghost-mock/app#151 (infra when the text says so), so the pending
 * card, the audit row and Ghost's reply all name the same issue. No number: the script's own.
 */
function aimAt<T extends { target: string; summary: string }>(p: T, text: string): T {
  const n = /(?:#|\b)(\d{1,6})\b/.exec(text);
  if (!n) return p;
  const repo = /\binfra\b/i.test(text) ? "ghost-mock/infra" : "ghost-mock/app";
  const target = `${repo}#${n[1]}`;
  return { ...p, target, summary: p.summary.split(p.target).join(target) };
}

/** The gate's summary after the write, in the past tense the real rows use ("closed the issue"). */
function doneSummary(summary: string): string {
  return summary.replace(/: close the issue/, ": closed the issue").replace(/: reopen the issue/, ": reopened the issue").replace(/: approve\b/, ": approved");
}

const slugOf = (name: string) => name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");

/** Mock-only knobs from the URL (the phone, ADR 0016; the catalog, ADR 0034): see mock/index.ts. */
const MOCK_COMMIT = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

/** The status's version and build for ?build= (MockOptions.build). */
function mockBuild(kind: MockOptions["build"]): Pick<Status, "version" | "build"> {
  switch (kind) {
    case "dirty":
      return { version: DESKTOP_VERSION, build: { kind: "dev", commit: MOCK_COMMIT, dirty: true } };
    case "deploy":
      return { version: DESKTOP_VERSION, build: { kind: "deploy", commit: MOCK_COMMIT, dirty: false } };
    case "release":
      return { version: DESKTOP_VERSION, build: { kind: "release", commit: MOCK_COMMIT, dirty: false } };
    case "behind":
      return { version: "2026.9.4", build: { kind: "deploy", commit: "9f8e7d6c5b4a39281706f5e4d3c2b1a098765432", dirty: false } };
    default:
      return { version: DESKTOP_VERSION, build: { kind: "dev", commit: MOCK_COMMIT, dirty: false } };
  }
}

export interface MockOptions {
  /**
   * The catalog (?catalog=…): `slow`, the first copy takes 4 seconds (as on a first run);
   * `problem`, the copy is three days old and the refresh failed; `empty`, the first copy
   * hasn't arrived yet and doesn't for 8 seconds. First run is `slow` without asking.
   */
  catalog?: "slow" | "problem" | "empty" | null;
  /** The phone pairs this many ms after a code is issued (?phone=pairs). 0: it never does. */
  pairsAfterMs?: number;
  /** How long a pairing code lasts, in seconds (?phone=expires makes it short). The real one: 120. */
  codeTtlSecs?: number;
  /**
   * What the daemon says it is (?build=…, docs/releasing.md): `dev` (default) a checkout at
   * this window's version, `dirty` the same with uncommitted changes, `deploy` a make deploy
   * runtime, `release` the .deb, `behind` a deploy of last month's version, so the window and
   * Ghost differ.
   */
  build?: "dev" | "dirty" | "deploy" | "release" | "behind" | null;
}

export class MockDaemon {
  readonly scenario: Scenario;
  private bootedAt = Date.now();
  private listeners = new Set<Listener>();
  private status: Status;
  private turns: Turn[];
  private pending: PendingProposal | null;
  private agents = new Map<string, AgentRecord>();
  /** Ghost's folder (ADR 0035): AGENTS.md, what Ghost made in out/, what it watches. */
  private rules: RulesView;
  private outputs: OutputInfo[];
  private watching: WatchingView;
  /** The apps in apps.json (GET /apps, ADR 0034) and the registry copy to connect more from (GET /catalog). */
  private apps: AppFixture[];
  private catalog: CatalogSeed[];
  /** Ghost's own sources; each connected app adds one when GET /sources answers. */
  private sources: SourceInfo[];
  private policy: PolicyView;
  private audit: AuditEntry[];
  private settings: SettingsView;
  private devices: DevicesView;
  private models: ModelsView;
  private voices: VoiceInfo[];
  private phone: PhoneView;
  private pairing: PhonePairing | null = null;
  private phoneOpts: MockOptions;
  /** The SKILL.md folders Ghost reads (ADR 0023): Ghost's own and the operator's. */
  private skills: SkillFixture[];
  private script: SessionFixture;
  /** What needs the operator (ADR 0029). */
  private waitingItems: WaitingItemView[];
  private timers = new Set<ReturnType<typeof setTimeout>>();

  constructor(scenario: Scenario, f: Fixtures, phoneOpts: MockOptions = {}) {
    this.scenario = scenario;
    this.phone = f.settings.phone;
    this.phoneOpts = phoneOpts;
    this.script = f.session;
    this.turns = f.session.turns;
    this.pending = f.session.pending;
    for (const l of f.conversations.agents) this.agents.set(l.detail.slug, l);
    this.rules = f.rules;
    this.outputs = f.outputs;
    this.watching = f.watching;
    this.voices = f.conversations.voices;
    this.apps = f.apps;
    this.catalog = f.catalog;
    this.sources = f.sources;
    this.policy = f.policy;
    this.audit = f.audit;
    this.settings = f.settings.settings;
    this.devices = f.settings.devices;
    this.models = f.settings.models;
    this.skills = f.skills;
    this.waitingItems = f.waiting;
    this.status = {
      phase: scenario.phase,
      session_id: f.session.sessionId,
      models_ready: scenario.modelsReady,
      uptime_secs: 0,
      ...mockBuild(phoneOpts.build ?? null),
      api_version: "1",
      wake_mode: "transcript",
      apps: f.apps.filter((a) => a.view.state === "connected").map((a) => a.view.name),
      pending: this.pending,
      conversation: scenario.session,
      session_origin: scenario.session ? (scenario.origin ?? "desk") : null,
      ending: false,
      waiting: f.waiting.filter((i) => i.state === "new" || i.state === "delivered").length,
    };
    if (this.pending && scenario.pendingExpiresIn) {
      const secs = scenario.pendingExpiresIn;
      this.pending = { ...this.pending, expires_in_secs: secs, expires_at: new Date(Date.now() + secs * 1000).toISOString() };
      this.scheduleExpiry(this.pending);
    }
    if (this.pending) {
      this.audit.unshift({
        id: this.pending.id,
        session_id: f.session.sessionId ?? "",
        source: this.pending.source,
        operation: this.pending.operation,
        target: this.pending.target,
        summary: this.pending.summary,
        autonomy: "confirm",
        status: "pending",
        created_at: new Date(Date.now() - 8_000).toISOString(),
        events: [{ at: new Date(Date.now() - 8_000).toISOString(), event: "prepared", detail: { source: this.pending.source, operation: this.pending.operation, target: `github:${this.pending.target}`, autonomy: "confirm" } }],
        conversation: this.status.conversation ?? null,
      });
    }
  }

  // ------------------------------------------------------------------ events
  subscribe(l: Listener): () => void {
    this.listeners.add(l);
    queueMicrotask(() => l(this.snapshot()));
    return () => this.listeners.delete(l);
  }
  private emit(e: GhostEvent) {
    for (const l of this.listeners) l(e);
  }
  /** The waiting change with its seconds left counted now, as the real daemon reports it. */
  private live(): PendingProposal | null {
    const p = this.pending;
    return p ? { ...p, expires_in_secs: Math.max(0, (new Date(p.expires_at).getTime() - Date.now()) / 1000) } : null;
  }
  private snapshot(): Status {
    return { ...this.status, uptime_secs: (Date.now() - this.bootedAt) / 1000 + 2731, pending: this.live() };
  }
  private pushStatus(patch: Partial<Status> = {}) {
    this.status = { ...this.status, ...patch, pending: this.pending };
    this.emit({ type: "status", status: this.snapshot() });
  }
  private later(ms: number, f: () => void) {
    const t = setTimeout(() => {
      this.timers.delete(t);
      f();
    }, ms);
    this.timers.add(t);
  }
  private addTurn(role: Turn["role"], text: string, via: Turn["via"]) {
    const t: Turn = { role, text, via, at: now(), session_id: this.status.session_id ?? "" };
    this.turns = [...this.turns, t];
    this.emit({ type: "turn", turn: t });
    const agent = this.agents.get(this.status.conversation ?? "ghost");
    if (agent && role !== "system") {
      agent.turns.push({ id: agent.turns.length + 1, role, text, at: t.at, session_id: t.session_id, compacted: false });
      agent.detail.turn_count = agent.turns.length;
      agent.detail.last_active_at = t.at;
    }
  }
  private conversationEvent(changed: string | null) {
    this.emit({ type: "conversation", active: this.status.session_id ? (this.status.conversation ?? null) : null, changed });
  }

  // ------------------------------------------------------------------ session
  private requireReady() {
    if (!this.status.models_ready) throw new HttpError(409, "Ghost is still loading its voice models.");
  }
  /** A session opens with Ghost, or with the agent a Call names; a retired or unknown agent is refused, as the daemon does. */
  private wake(conversation?: string | null) {
    this.requireReady();
    if (this.status.session_id) return;
    const slug = conversation || "ghost";
    const target = this.agents.get(slug)?.detail;
    if (!target || target.retired_at) throw new HttpError(409, `There's no working agent called ${slug}.`);
    this.turns = [];
    this.setActive(slug);
    this.pushStatus({ session_id: newId(), phase: "listening", conversation: slug, session_origin: "desk" });
    this.addTurn("assistant", target.is_ghost ? "Ghost here." : `${target.name} here.`, "system");
  }
  private setActive(slug: string | null) {
    for (const l of this.agents.values()) l.detail.active = l.detail.slug === slug;
  }
  private say(text: string, conversation?: string | null) {
    if (!this.status.session_id) this.wake(conversation);
    this.addTurn("user", text, "text");
    this.pushStatus({ phase: "thinking" });
    this.later(900, () => {
      const patch = /(?:patch me (?:through )?to|transfer me to|switch to|get me|back to)\s+(?:the\s+)?([\w -]+?)(?:\s+(?:line|agent))?[.!?]*$/i.exec(text);
      if (patch) {
        const want = patch[1].trim().toLowerCase();
        // A retired agent has left voice routing: its name no longer transfers.
        const agent = [...this.agents.values()].find((l) => !l.detail.retired_at && (l.detail.slug === want || l.detail.name.toLowerCase() === want || l.detail.aliases.includes(want)));
        if (agent) {
          this.switchTo(agent.detail.slug);
          return this.reply(`${agent.detail.is_ghost ? "Ghost" : agent.detail.name} here.`);
        }
      }
      const rule = this.script.replies.find((r) => r.match.test(text));
      if (rule?.propose && !this.pending) {
        const p = aimAt(rule.propose, text);
        this.propose(p);
        // The reply names the same issue the card shows: “Close app 151? Say yes, or press Confirm.”
        return this.reply(rule.reply.replace(/\bthat issue\b/, shortName(p.target)));
      }
      this.reply(rule?.reply ?? this.script.fallbackReply);
    });
  }
  private reply(text: string) {
    this.addTurn("assistant", text, "system");
    this.pushStatus({ phase: "speaking" });
    this.later(1600, () => this.status.session_id && this.pushStatus({ phase: "listening" }));
  }
  private propose(p: Omit<PendingProposal, "id" | "expires_at" | "expires_in_secs">) {
    const where = { conversation: this.status.conversation ?? null };
    this.pending = { ...p, id: newId(), expires_at: new Date(Date.now() + 60_000).toISOString(), expires_in_secs: 60 };
    this.audit.unshift({
      id: this.pending.id, session_id: this.status.session_id ?? "", source: p.source, operation: p.operation, target: p.target,
      summary: p.summary, autonomy: "confirm", status: "pending", created_at: now(),
      events: [{ at: now(), event: "prepared", detail: { source: p.source, operation: p.operation, target: `github:${p.target}`, autonomy: "confirm" } }],
      ...where,
    });
    this.emit({ type: "pending", pending: this.pending });
    this.pushStatus();
    this.scheduleExpiry(this.pending);
  }
  /** A waiting change lapses: audit "expired", a pending:null event, and Ghost says so. */
  private scheduleExpiry(p: PendingProposal) {
    const ms = new Date(p.expires_at).getTime() - Date.now();
    this.later(Math.max(0, ms), () => {
      if (this.pending?.id !== p.id) return;
      const entry = this.audit.find((a) => a.id === p.id);
      if (entry) {
        entry.status = "expired";
        entry.events.push({ at: now(), event: "expired" });
      }
      this.pending = null;
      this.emit({ type: "pending", pending: null });
      this.pushStatus();
      if (this.status.session_id) this.reply("I didn’t hear a yes, so I left it alone.");
    });
  }
  private sleepNow() {
    if (this.pending) this.settlePending("cancelled", { reason: "session ended" });
    this.setActive(null);
    this.pushStatus({ session_id: null, phase: "passive", conversation: null, session_origin: null });
    this.conversationEvent(null);
  }
  private settlePending(status: "executed" | "cancelled", detail?: Record<string, unknown>): AuditEntry | null {
    const p = this.pending;
    if (!p) return null;
    const entry = this.audit.find((a) => a.id === p.id);
    if (entry) {
      entry.status = status;
      if (status === "executed") {
        entry.summary = doneSummary(entry.summary);
        entry.events.push(
          { at: now(), event: "user_turn", detail: { verdict: "affirmative", classifier: "ui" } },
          { at: now(), event: "claimed" },
          ...stepsOf(entry.operation, entry.summary).map((step, index) => ({ at: now(), event: "step_applied", detail: { index, step } })),
          { at: now(), event: "finished", detail: { status: "success" } },
        );
      } else {
        entry.events.push({ at: now(), event: "cancelled", detail: detail ?? { reason: "cancelled" } });
      }
    }
    this.pending = null;
    this.emit({ type: "pending", pending: null });
    this.pushStatus();
    return entry ?? null;
  }
  private confirm(): OutcomeView {
    const p = this.pending;
    if (!p) throw new HttpError(409, "Nothing is waiting for your okay.");
    this.settlePending("executed");
    const outcome: OutcomeView = { proposal_id: p.id, status: "success", summary: p.summary.replace(/^On [^:]+: /, ""), target: p.target, applied: [p.operation], state_now: "closed", error: null, failed_step: null };
    this.emit({ type: "outcome", outcome, via: "ui" });
    const short = shortName(p.target);
    this.reply(`Done. ${short[0].toUpperCase()}${short.slice(1)} is closed.`);
    return outcome;
  }

  // ------------------------------------------------------------------ agents (ADR 0014, ADR 0035)
  private view(d: ConversationDetail): ConversationView {
    const { id, slug, name, aliases, job, voice, speed, own_voice, created_at, last_active_at, retired_at, active, is_ghost } = d;
    return { id, slug, name, aliases, job, voice, speed, own_voice, created_at, last_active_at, retired_at, active, is_ghost };
  }
  private agent(slug: string): AgentRecord {
    const l = this.agents.get(slug);
    if (!l) throw new HttpError(404, `No agent called ${slug}.`);
    return l;
  }
  /** The voice an agent without its own speaks in: Ghost's. */
  private ghostVoice(): string {
    return this.agents.get("ghost")?.detail.voice ?? "af_heart";
  }
  /** The old agent speaks the transfer, as ghost_voice/conversation.py does, then the new one is active. */
  private switchTo(slug: string) {
    const target = this.agent(slug);
    if (!this.status.session_id) throw new HttpError(409, "Wake Ghost first: switching transfers a live session.");
    if (target.detail.retired_at) throw new HttpError(409, `${target.detail.name} is retired. Rehire it to transfer.`);
    if (this.status.conversation !== slug) this.addTurn("assistant", target.detail.is_ghost ? "Back to Ghost." : `Transferring you to ${target.detail.name}.`, "system");
    this.setActive(slug);
    this.pushStatus({ conversation: slug });
    this.conversationEvent(slug);
  }
  private audited(source: string, operation: string, target: string, summary: string, autonomy = "announce") {
    this.audit.unshift({
      id: newId(), session_id: this.status.session_id ?? "ui", source, operation, target, summary, autonomy, status: "executed", created_at: now(),
      events: [{ at: now(), event: "prepared", detail: { source, operation, target, autonomy } }, { at: now(), event: "claimed" }, { at: now(), event: "finished", detail: { status: "success" } }],
      conversation: this.status.conversation ?? null,
    });
  }

  // ------------------------------------------------------------------ Ghost's folder (ADR 0035)
  /**
   * POST /rules: the operator's own click, through the gate as rules.add at quiet (the
   * daemon's apply_internal): the permissions' block still refuses it (409, the gate's own
   * sentence), anything else runs at once. Appended verbatim, as ghost_core.rules does: a
   * heading when the file is new, the rule as its own paragraph.
   */
  private addRule(text: string): OutcomeView {
    const rule = text.trim();
    if (!rule) throw new HttpError(409, "the rule is required, in the operator's words");
    if (rule.length > 1000) throw new HttpError(409, "that rule is too long; keep it to a few sentences");
    if (levelOf(this.policy.permissions, "rules", "add", false) === "block") throw new HttpError(409, "rules.add is blocked by the operator's permissions");
    const current = this.rules.text.trim() ? this.rules.text.replace(/\n*$/, "\n") : "# Ghost\n";
    this.rules = { ...this.rules, text: `${current}\n${rule}\n` };
    const id = newId();
    this.audit.unshift({
      id, session_id: "ui", source: "rules", operation: "add", target: "rules:ghost", summary: `On Ghost's rules: add the rule: ${rule}`, autonomy: "quiet", status: "executed", created_at: now(),
      events: [
        { at: now(), event: "prepared", detail: { source: "rules", operation: "add", target: "rules:ghost", autonomy: "quiet" } },
        { at: now(), event: "claimed" },
        { at: now(), event: "step_applied", detail: { index: 0, step: `add the rule: ${rule}` } },
        { at: now(), event: "finished", detail: { status: "success" } },
      ],
      conversation: null,
    });
    return { proposal_id: id, status: "success", summary: `add the rule: ${rule}`, target: "rules:ghost", applied: ["add_rule"], state_now: null, error: null, failed_step: null };
  }

  // ------------------------------------------------------------------ skills (ADR 0023)
  /** GET /skills as the daemon reports it: `missing` re-read from the apps connected now. */
  private skillList(): SkillInfo[] {
    const connected = new Set(this.connectedApps());
    return this.skills.map((f) => ({ ...f.info, missing: missingOf(f.info.requires, connected) }));
  }
  private skillNamed(name: string): SkillFixture | null {
    return this.skills.find((f) => f.info.name === name) ?? null;
  }
  /**
   * POST /skills without a file system: the folder's last name is the skill's. The refusals
   * are worded as ghost_core.skills words them (409), so the screen shows the real sentences:
   * not a folder, no SKILL.md, inside Ghost's own directory, already installed.
   */
  private addSkill(raw: string, wanted: string | null, replace: boolean): SkillInfo {
    const path = raw.trim().replace(/\/+$/, "");
    if (!path) throw new HttpError(409, "path is required: the folder holding SKILL.md");
    if (!/^(~|\/)/.test(path)) throw new HttpError(409, `${path} is not a folder`);
    const folder = path.replace(/\/SKILL\.md$/, "");
    const last = folder.split("/").pop() ?? "";
    if (/\.[a-z0-9]{1,5}$/i.test(last)) throw new HttpError(409, `${path} is not a folder`);
    if (folder.startsWith(`${GHOST_SKILLS_DIR}/`) || /\/\.config\/ghost\/skills\//.test(folder)) throw new HttpError(409, `${path} is already in Ghost's skills`);
    // The mock can't look inside: a home folder, a desktop or a downloads folder is plainly not a skill.
    if (!last || /^(home|desktop|documents|downloads|tmp|projects|src)$/i.test(last)) throw new HttpError(409, `${path} has no SKILL.md with a name`);
    const name = (wanted ?? last).trim();
    if (!/^[a-z0-9][a-z0-9.-]*$/i.test(name)) throw new HttpError(409, `${JSON.stringify(name)} is not a skill name (letters, digits, dots, dashes)`);
    const existing = this.skillNamed(name);
    if (existing && existing.info.source === "ghost" && !replace) throw new HttpError(409, `${name} is already installed`);
    const title = name.replace(/[-.]+/g, " ");
    const added: SkillFixture = {
      info: { name, description: `${title[0].toUpperCase()}${title.slice(1)}, added from ${folder}.`, source: "ghost", path: `${GHOST_SKILLS_DIR}/${name}`, requires: [], missing: [], files: [], problem: null },
      body: `# ${title[0].toUpperCase()}${title.slice(1)}\n\nCopied from ${folder}. Edit SKILL.md in ${GHOST_SKILLS_DIR}/${name} to change what Ghost reads.\n`,
    };
    if (existing && existing.info.source === "ghost") this.skills[this.skills.indexOf(existing)] = added;
    else this.skills.push(added);
    this.audited("skills", "add", `skill:${name}`, existing ? `Replaced the ${name} skill from ${folder}.` : `Added the ${name} skill from ${folder}.`, "quiet");
    return this.skillList().find((i) => i.name === name)!;
  }
  private removeSkill(name: string) {
    const f = this.skillNamed(name);
    if (!f) throw new HttpError(409, `no skill named ${JSON.stringify(name)}`);
    if (f.info.source !== "ghost") throw new HttpError(409, `${name} lives in ${f.info.source}; Ghost only removes its own`);
    this.skills = this.skills.filter((x) => x !== f);
    this.audited("skills", "remove", `skill:${name}`, `Removed the ${name} skill.`, "quiet");
  }

  // ------------------------------------------------------------------ waiting (ADR 0029)
  private waitingView() {
    const count = this.waitingItems.filter((i) => i.state === "new" || i.state === "delivered").length;
    return { items: this.waitingItems, count };
  }
  private markWaiting(key: string, patch: Partial<WaitingItemView>): boolean {
    const i = this.waitingItems.findIndex((x) => x.key === key);
    if (i < 0) return false;
    this.waitingItems[i] = { ...this.waitingItems[i], ...patch };
    this.waitingChanged();
    return true;
  }
  private dropWaiting(key: string) {
    this.waitingItems = this.waitingItems.filter((x) => x.key !== key);
    this.waitingChanged();
  }
  private waitingChanged() {
    const { count } = this.waitingView();
    this.emit({ type: "waiting", count });
    this.pushStatus({ waiting: count });
  }

  // ------------------------------------------------------------------ apps (ADR 0034)
  private connectedApps(): string[] {
    return this.apps.filter((a) => a.view.state === "connected").map((a) => a.view.name);
  }
  /** GET /apps/{name}: each tool at the level the permissions give it now, once the app is connected. */
  private appView(f: AppFixture): AppView {
    const p = this.policy.permissions;
    const name = f.view.name;
    const actions =
      f.view.state === "connected"
        ? f.tools.map((t) => ({ name: t.name, description: t.description, reads: t.reads, level: levelOf(p, name, t.name, t.reads), rule: `${name}.${t.name}`, conditional: conditionalFor(p, name, t.name) }))
        : [];
    return { ...structuredClone(f.view), actions };
  }
  /** GET /sources: Ghost's own, then each connected app, every operation at the level its name gets. */
  private sourceList(): SourceInfo[] {
    const p = this.policy.permissions;
    const all = [...this.sources, ...this.apps.filter((a) => a.view.state === "connected").map(sourceOf)];
    return all.map((src) => ({ ...src, operations: src.operations.map((op) => ({ ...op, autonomy: AUTONOMY[levelOf(p, src.name, op.name, false)] })) }));
  }
  private appsChanged(changed: string | null, sign_in_url: string | null = null) {
    this.emit({ type: "apps", changed, sign_in_url });
    this.pushStatus({ apps: this.connectedApps() });
  }
  private freeName(base: string): string {
    const taken = new Set(this.apps.map((a) => a.view.name));
    if (!taken.has(base)) return base;
    let n = 2;
    while (taken.has(`${base}-${n}`)) n++;
    return `${base}-${n}`;
  }
  private entryView(e: CatalogSeed): CatalogEntryView {
    const { app, ...rest } = structuredClone(e);
    return {
      ...rest,
      version: e.version ?? "1.0.0",
      website: e.website ?? null,
      repository: e.repository ?? null,
      suggested_name: this.freeName(app),
      connected: this.apps.filter((a) => a.view.catalog === e.name).map((a) => a.view.name),
    };
  }
  private async catalogPage(search: string, offset: number, limit: number): Promise<CatalogPage> {
    const opt = this.phoneOpts.catalog ?? (this.scenario.configured ? null : "slow");
    // The first copy arrives 4 seconds after Ghost starts; every request before then waits for it.
    if (opt === "slow") await sleep(Math.max(0, this.bootedAt + 4000 - Date.now()));
    if (opt === "empty" && Date.now() - this.bootedAt < 8000) return { entries: [], total: 0, fetched_at: null, refreshing: true, problem: null };
    const q = search.trim().toLowerCase();
    const all = q
      ? this.catalog.filter((c) => [c.title, c.description, c.name].some((x) => x.toLowerCase().includes(q)))
      : [...this.catalog].sort((a, b) => Number(b.featured) - Number(a.featured));
    return {
      entries: all.slice(offset, offset + limit).map((e) => this.entryView(e)),
      total: all.length,
      fetched_at: new Date(Date.now() - (opt === "problem" ? 3 * 86400000 : 20 * 60000)).toISOString(),
      refreshing: false,
      problem: opt === "problem" ? "Couldn't reach registry.modelcontextprotocol.io: timed out after 20 seconds" : null,
    };
  }
  /** The address a sign-in opens, as the authorization server would hand it back. */
  private signInUrl(name: string, website: string | null | undefined): string {
    const host = website ? new URL(website).host : `${name}.example.com`;
    return `https://${host}/oauth/authorize?client_id=ghost-${hex(8, rand)}&redirect_uri=${encodeURIComponent("http://127.0.0.1:53682/callback")}&response_type=code&state=${hex(16, rand)}`;
  }
  /** Bring an app up a moment later: a sign-in finishes in the browser, a key or a command just connects. */
  private settle(f: AppFixture, after: number, outcome: { problem?: string | null } = {}) {
    this.later(after, () => {
      if (!this.apps.includes(f)) return;
      f.view.sign_in_url = null;
      if (outcome.problem) Object.assign(f.view, { state: "failed", problem: outcome.problem });
      else Object.assign(f.view, { state: "connected", problem: null });
      this.appsChanged(f.view.name);
    });
  }
  private connect(b: AppConnect): AppConnectResult {
    const values = b.values ?? {};
    const made: AppFixture[] = [];
    let website: string | null = null;
    if (b.config != null) {
      let parsed: unknown;
      try {
        parsed = JSON.parse(b.config);
      } catch (e) {
        throw new HttpError(422, `the pasted config: not JSON (${(e as Error).message})`);
      }
      const root = (parsed && typeof parsed === "object" ? ((parsed as Record<string, unknown>).mcpServers ?? parsed) : null) as Record<string, Record<string, unknown>> | null;
      if (!root || typeof root !== "object" || !Object.keys(root).length) throw new HttpError(422, "no apps in it");
      for (const [key, entry] of Object.entries(root)) {
        const name = key.toLowerCase().replace(/[^a-z]+/g, "-").replace(/^-|-$/g, "");
        const secrets = Object.keys({ ...((entry.env as object) ?? {}), ...((entry.headers as object) ?? {}) }).map((k) => `GHOST_APP_${name.toUpperCase().replace(/-/g, "_")}_${k.toUpperCase().replace(/[^A-Z0-9]+/g, "_")}`);
        made.push({
          view: { name, title: key, kind: entry.command ? "local" : secrets.length ? "key" : "custom", state: "connecting", problem: null, catalog: null, icon: null, sign_in_url: null, settings: secrets },
          tools: GENERIC_TOOLS,
        });
      }
    } else if (b.url != null) {
      let u: URL | null = null;
      try {
        u = new URL(b.url);
      } catch {
        u = null;
      }
      if (!u || !/^https?:$/.test(u.protocol)) throw new HttpError(422, "the address must start with https://");
      let name = b.name || u.hostname.toLowerCase().replace(/[^a-z0-9]+/g, "-");
      name = name.replace(/^mcp-/, "").replace(/-com$/, "").replace(/^-+|-+$/g, "").slice(0, 40) || "app";
      const key = (values.key ?? "").trim();
      made.push({
        view: { name, title: values.title || name, kind: key ? "key" : "sign_in", state: "connecting", problem: null, catalog: null, icon: null, sign_in_url: null, settings: key ? [`GHOST_APP_${name.toUpperCase().replace(/-/g, "_")}_KEY`] : [] },
        tools: GENERIC_TOOLS,
      });
      website = `${u.protocol}//${u.host}`;
    } else {
      if (!b.catalog || !b.option) throw new HttpError(422, "give catalog and option, a url, or a config");
      const entry = this.catalog.find((c) => c.name === b.catalog);
      if (!entry) throw new HttpError(404, `${b.catalog} is not in the catalog`);
      const option = entry.options.find((o) => o.id === b.option);
      if (!option) throw new HttpError(422, `${entry.title} has no way to connect called ${b.option}`);
      for (const s of option.settings) if (s.required && !(values[s.key] ?? "").trim() && !s.default) throw new HttpError(422, `${s.label} is required to connect ${entry.title}`);
      const name = b.name || this.freeName(entry.app);
      const env = name.toUpperCase().replace(/-/g, "_");
      made.push({
        view: { name, title: entry.title, kind: option.kind, state: "connecting", problem: null, catalog: entry.name, icon: entry.icon ?? null, sign_in_url: null, settings: option.settings.filter((s) => s.secret).map((s) => `GHOST_APP_${env}_${s.key.toUpperCase()}`) },
        tools: TOOLS_BY_CATALOG[entry.name] ?? GENERIC_TOOLS,
      });
      website = entry.website ?? null;
    }
    for (const f of made) {
      if (!/^[a-z][a-z-]*$/.test(f.view.name)) throw new HttpError(422, `'${f.view.name}': a name is lower-case letters and -`);
      if (this.apps.some((a) => a.view.name === f.view.name) && !b.replace) throw new HttpError(409, `an app is already connected as ${f.view.name}`);
    }
    let url: string | null = null;
    for (const f of made) {
      this.apps = [...this.apps.filter((a) => a.view.name !== f.view.name), f];
      if (f.view.kind === "sign_in") {
        url = f.view.sign_in_url = this.signInUrl(f.view.name, website);
        this.settle(f, 7000);
      } else this.settle(f, 1400);
    }
    this.appsChanged(made[0]?.view.name ?? null, url);
    return { apps: made.map((f) => this.appView(f)), sign_in_url: url, problems: [] };
  }
  /** POST /apps/{name}/sign-in: sign in again, or reconnect an app that isn't connected. */
  private signIn(f: AppFixture): AppConnectResult {
    const problem = f.view.state === "failed" && f.view.kind === "local" ? f.view.problem : null;
    Object.assign(f.view, { state: "connecting", problem: null });
    let url: string | null = null;
    if (f.view.kind === "sign_in") {
      const website = this.catalog.find((c) => c.name === f.view.catalog)?.website;
      url = f.view.sign_in_url = this.signInUrl(f.view.name, website);
      this.settle(f, 7000);
    } else this.settle(f, 1500, { problem });
    this.appsChanged(f.view.name, url);
    return { apps: [this.appView(f)], sign_in_url: url, problems: [] };
  }
  /** PUT /permissions: the mode, whole lists, or one rule moved; only [permissions] is rewritten. */
  private updatePermissions(u: PermissionsUpdate): PermissionsView {
    let p: PermissionsView = { ...this.policy.permissions };
    if (u.mode != null) {
      if (!["open", "ask", "read-only"].includes(u.mode)) throw new HttpError(422, `'${u.mode}' is not a mode: open, ask or read-only`);
      p.mode = u.mode;
    }
    for (const list of LISTS) {
      const v = u[list];
      if (v == null) continue;
      p[list] = v.map((r) => {
        try {
          return ruleText(parseRule(r));
        } catch (e) {
          throw new HttpError(422, `${list}: ${(e as Error).message}`);
        }
      });
    }
    if (u.set) p = withRule(p, u.set.rule, u.set.level ?? null);
    this.policy = { ...this.policy, exists: true, permissions: p, text: renderPolicy(p, this.policy.routes) };
    this.appsChanged(null);
    return p;
  }

  // ------------------------------------------------------------------ router
  async handle(method: string, path: string, query: URLSearchParams, body: unknown): Promise<unknown> {
    const b = (body ?? {}) as Record<string, unknown>;
    const m = (re: RegExp) => re.exec(path);
    let r: RegExpExecArray | null;

    if (method === "GET" && path === "/status") return this.snapshot();
    if (method === "POST" && path === "/wake") {
      this.wake(typeof b.conversation === "string" ? b.conversation : null);
      if (typeof b.text === "string" && b.text) this.say(b.text);
      return { ok: true };
    }
    if (method === "POST" && path === "/sleep") return this.sleepNow(), { ok: true };
    if (method === "POST" && path === "/session/say") {
      this.requireReady();
      this.say(String(b.text ?? ""), typeof b.conversation === "string" ? b.conversation : null);
      return { ok: true };
    }
    if (method === "GET" && path === "/session/turns") return this.turns;
    if (method === "GET" && path === "/proposals/pending") return this.live();
    if (method === "POST" && path === "/proposals/pending/confirm") return this.confirm();
    if (method === "POST" && path === "/proposals/pending/cancel") {
      if (!this.pending) throw new HttpError(409, "Nothing is waiting for your okay.");
      this.settlePending("cancelled", { reason: "ui" });
      this.reply("Okay, I won't.");
      return { ok: true };
    }
    if (method === "GET" && path === "/waiting") return this.waitingView();
    if (method === "POST" && path === "/waiting/ack") return { ok: this.markWaiting(String(b.key ?? ""), { state: "acknowledged", acknowledged_at: now() }) };
    if (method === "POST" && path === "/waiting/snooze") {
      const until = new Date(Date.now() + Number(b.minutes ?? 30) * 60000).toISOString();
      return { ok: this.markWaiting(String(b.key ?? ""), { state: "snoozed", snoozed_until: until }) };
    }
    if ((r = m(/^\/proposals\/([^/]+)\/(confirm|cancel)$/))) {
      const item = this.waitingItems.find((i) => i.change?.id === r![1]);
      if (!item) throw new HttpError(409, "that change isn't waiting");
      this.dropWaiting(item.key);
      if (r[2] === "cancel") return { ok: true };
      return { proposal_id: r[1], target: item.change!.target, status: "success", applied: [item.change!.summary], summary: `Applied on ${item.change!.target}.` } satisfies OutcomeView;
    }
    if (method === "GET" && path === "/audit") {
      let items = this.audit;
      const source = query.get("source"), status = query.get("status"), sid = query.get("session_id");
      if (source) items = items.filter((e) => e.source === source);
      if (status) items = items.filter((e) => e.status === status);
      if (sid) items = items.filter((e) => e.session_id === sid);
      const limit = Number(query.get("limit") ?? 50), offset = Number(query.get("offset") ?? 0);
      return { items: items.slice(offset, offset + limit), total: items.length, limit, offset };
    }

    if (method === "GET" && path === "/sources") return this.sourceList();
    if (method === "GET" && path === "/apps") return this.apps.map((a) => this.appView(a));
    if (method === "POST" && path === "/apps") return this.connect(b as AppConnect);
    if ((r = m(/^\/apps\/([^/]+)(\/sign-in)?$/))) {
      const name = decodeURIComponent(r[1]);
      const f = this.apps.find((a) => a.view.name === name);
      if (!f) throw new HttpError(404, `no app called '${name}'`);
      if (method === "GET" && !r[2]) return this.appView(f);
      if (method === "POST" && r[2]) return this.signIn(f);
      if (method === "DELETE" && !r[2]) {
        this.apps = this.apps.filter((a) => a !== f);
        this.appsChanged(name);
        return { ok: true, detail: null };
      }
    }
    if (method === "GET" && path === "/catalog") return this.catalogPage(query.get("search") ?? "", Number(query.get("offset") ?? 0), Number(query.get("limit") ?? 40));
    if (method === "GET" && (r = m(/^\/catalog\/(.+)$/))) {
      const name = decodeURIComponent(r[1]);
      const e = this.catalog.find((c) => c.name === name);
      if (!e) throw new HttpError(404, `${name} is not in the catalog`);
      return this.entryView(e);
    }
    if (method === "GET" && path === "/permissions") return this.policy.permissions;
    if (method === "PUT" && path === "/permissions") return this.updatePermissions(b as PermissionsUpdate);
    if (method === "GET" && path === "/policy") return this.policy;
    if (method === "PUT" && path === "/policy") {
      const u = b as PolicyUpdate;
      if (u.text != null) {
        // As the daemon does: the file is read before it is written, and refused when it can't be.
        const permissions = readPolicyText(u.text, this.policy.path);
        this.policy = { ...this.policy, text: u.text, exists: true, permissions };
        this.appsChanged(null);
      } else if (u.routes) {
        if (u.routes.escalate_after < 1) throw new HttpError(422, "[routes].escalate_after is a whole number from 1 to 1440");
        const routes = { ...u.routes, enabled: (["digest", "desk", "desktop", "phone", "call", "sms", "whatsapp"] as const).filter((x) => u.routes!.enabled.includes(x)) };
        this.policy = { ...this.policy, routes, exists: true, text: renderPolicy(this.policy.permissions, routes) };
      }
      return { applied: true, policy: this.policy };
    }
    if (method === "GET" && path === "/settings") return this.settings;
    if (method === "PUT" && path === "/settings") {
      const u = b as SettingsUpdate;
      const applied: string[] = [];
      let restart = false;
      for (const [k, v] of Object.entries(u.values ?? {})) {
        if (v == null) delete this.settings.values[k];
        else this.settings.values[k] = v;
        if (k.startsWith("GHOST_WAKE")) applied.push(k);
        else restart = true;
      }
      for (const [k, v] of Object.entries(u.secrets ?? {})) {
        const s = this.settings.secrets.find((x) => x.name === k);
        if (s) s.set = v != null && v !== "";
        restart = true;
      }
      this.settings = { ...this.settings, values: { ...this.settings.values }, secrets: [...this.settings.secrets] };
      return { applied, restart_required: restart, settings: this.settings };
    }
    if (method === "GET" && path === "/devices") return this.devices;
    if (method === "POST" && path === "/setup/echo-cancel") {
      await sleep(700);
      const uninstall = query.get("uninstall") === "true";
      this.devices = { ...this.devices, echo_cancel_active: !uninstall, echo_cancel_source_present: !uninstall };
      return { ok: true, detail: uninstall ? "Removed the ghost.mic echo-cancel source." : "Installed ghost.mic." };
    }
    if (method === "GET" && path === "/setup/models") return this.models;
    if (method === "GET" && path === "/voices") return this.voices;
    if (method === "POST" && (r = m(/^\/voices\/([^/]+)\/sample$/))) {
      this.requireReady();
      await sleep(900);
      return { ok: true, detail: `Spoke a sample in ${decodeURIComponent(r[1])}.` };
    }

    if (method === "GET" && path === "/skills") return this.skillList();
    if (method === "POST" && path === "/skills") return this.addSkill(String(b.path ?? ""), typeof b.name === "string" ? b.name : null, b.replace === true);
    if ((r = m(/^\/skills\/([^/]+)$/))) {
      const name = decodeURIComponent(r[1]);
      if (method === "GET") {
        const f = this.skillNamed(name);
        if (!f) throw new HttpError(404, `no skill ${JSON.stringify(name)}`);
        return { name: f.info.name, body: f.body, files: [...f.info.files] };
      }
      if (method === "DELETE") return this.removeSkill(name), { ok: true, detail: null };
    }

    if (method === "GET" && path === "/rules") return structuredClone(this.rules);
    if (method === "POST" && path === "/rules") return this.addRule(String(b.text ?? ""));
    if (method === "GET" && path === "/outputs") return structuredClone(this.outputs);
    if (method === "GET" && path === "/watches") return structuredClone(this.watching);

    if (method === "GET" && path === "/phone") return this.phone;
    if (method === "POST" && path === "/phone/pairing") return this.newPairing();
    if (method === "DELETE" && path === "/phone") {
      if (!this.phone.device) return { ok: true, detail: "No phone was paired." };
      const name = this.phone.device.name;
      this.phone = { ...this.phone, device: null };
      if (this.status.session_id && this.status.session_origin === "phone") this.sleepNow();
      return { ok: true, detail: `Forgot ${name}.` };
    }

    if (method === "GET" && path === "/conversations") {
      let out = [...this.agents.values()].map((l) => this.view(l.detail));
      const retired = query.get("retired");
      if (retired != null) out = out.filter((a) => (a.retired_at != null) === (retired === "true"));
      return out;
    }
    if (method === "POST" && path === "/conversations") {
      const name = String(b.name ?? "").trim();
      if (!name) throw new HttpError(422, "An agent needs a name.");
      const slug = slugOf(name);
      if (this.agents.has(slug)) throw new HttpError(409, `There is already an agent called ${name}.`);
      // seed_from: a retired agent's memory is the new one's starting memory, a handoff to edit.
      let memory = typeof b.memory === "string" ? b.memory : "";
      const seed = typeof b.seed_from === "string" && b.seed_from ? this.agents.get(b.seed_from) : null;
      if (b.seed_from && !seed) throw new HttpError(422, `No agent called ${String(b.seed_from)} to start from.`);
      if (seed) memory = seed.detail.memory;
      const own = typeof b.voice === "string" && !!b.voice;
      const detail: ConversationDetail = {
        id: newId(), slug, name, aliases: (b.aliases as string[]) ?? [], job: String(b.job ?? ""),
        voice: own ? (b.voice as string) : this.ghostVoice(), speed: (b.speed as number) ?? 1, own_voice: own,
        persona: String(b.persona ?? ""), memory, notes: [], created_at: now(), last_active_at: null, retired_at: null, active: false, is_ghost: false,
        turn_count: 0, compactions: memory ? 1 : 0, dir: `${CONFIG_DIR}/agents/${slug}`,
      };
      this.agents.set(slug, { detail, turns: [], compactions: memory ? [{ id: newId(), at: now(), source: "manual", summary: memory, through_turn_id: null }] : [] });
      this.audited("conversations", "create", `agent:${slug}`, `On ${slug.replace(/-/g, " ")}: hire ${JSON.stringify(name)}${detail.job ? ` for ${detail.job}` : ""}${seed ? " with a handoff memory" : ""}.`);
      this.conversationEvent(slug);
      return detail;
    }
    if ((r = m(/^\/conversations\/([^/]+)(\/[a-z]+)?$/))) {
      const slug = decodeURIComponent(r[1]);
      const sub = r[2] ?? "";
      const l = this.agent(slug);
      const d = l.detail;
      const spoken = slug.replace(/-/g, " ");
      if (method === "GET" && sub === "") return d;
      if (method === "GET" && sub === "/turns") return l.turns.slice(-Number(query.get("limit") ?? 200));
      if (method === "GET" && sub === "/compactions") return l.compactions;
      if (method === "PATCH" && sub === "") {
        if (d.retired_at) throw new HttpError(409, `${d.name} is retired. Rehire it to change it.`);
        if (d.is_ghost && typeof b.name === "string" && slugOf(b.name) !== slug) throw new HttpError(409, "Ghost keeps its name");
        for (const k of ["name", "aliases", "job", "speed", "persona", "memory", "notes"] as const) {
          if (b[k] != null) (d as unknown as Record<string, unknown>)[k] = b[k];
        }
        if (typeof b.voice === "string") {
          d.own_voice = b.voice !== "";
          d.voice = b.voice || this.ghostVoice();
        }
        if (typeof b.memory === "string") {
          d.compactions += 1;
          l.compactions.unshift({ id: newId(), at: now(), source: "edit", summary: b.memory, through_turn_id: null });
        }
        this.audited("conversations", "update", `agent:${slug}`, `On ${spoken}: updated ${Object.keys(b).join(", ")}.`);
        this.conversationEvent(slug);
        return d;
      }
      // Retire and rehire: announce-level, nothing is destroyed.
      if (method === "POST" && sub === "/retire") {
        if (d.is_ghost) throw new HttpError(409, "Ghost stays; it is where every wake lands");
        if (d.active) throw new HttpError(409, "Switch away from this agent before retiring it.");
        if (!d.retired_at) {
          d.retired_at = now();
          this.audited("conversations", "retire", `agent:${slug}`, `On ${spoken}: retire the agent.`);
          this.conversationEvent(slug);
        }
        return { ok: true, detail: `Retired ${d.name}.` };
      }
      if (method === "POST" && sub === "/rehire") {
        if (d.retired_at) {
          d.retired_at = null;
          this.audited("conversations", "rehire", `agent:${slug}`, `On ${spoken}: bring the agent back.`);
          this.conversationEvent(slug);
        }
        return { ok: true, detail: `${d.name} is back.` };
      }
      if (method === "POST" && sub === "/notes") {
        d.notes = [...d.notes, String(b.text ?? "")];
        this.audited("conversations", "remember", `agent:${slug}`, `On ${spoken}: remember: ${b.text}.`);
        this.conversationEvent(slug);
        return d;
      }
      if (method === "DELETE" && sub === "") {
        if (d.is_ghost) throw new HttpError(409, "Ghost stays; it is where every wake lands");
        if (d.active) throw new HttpError(409, "Switch away from this agent before deleting it.");
        this.agents.delete(slug);
        this.audited("conversations", "delete", `agent:${slug}`, `On ${spoken}: delete the agent.`, "confirm");
        this.conversationEvent(slug);
        return { ok: true };
      }
      if (method === "POST" && sub === "/reset") {
        if (d.active) throw new HttpError(409, "Switch away from this agent before resetting it.");
        Object.assign(d, { memory: "", notes: [], turn_count: 0, compactions: 0 });
        l.turns = [];
        l.compactions = [];
        this.audited("conversations", "reset", `agent:${slug}`, `On ${spoken}: reset: forget everything.`, "confirm");
        this.conversationEvent(slug);
        return { ok: true };
      }
      if (method === "POST" && sub === "/switch") return this.switchTo(slug), { ok: true };
      if (method === "POST" && sub === "/compact") {
        if (!d.active) throw new HttpError(409, "Compaction runs in a live session with this agent.");
        this.later(1500, () => {
          const summary = `${d.memory}\nCompacted ${new Date().toLocaleString()}: ${l.turns.length} turns folded in.`.trim();
          d.memory = summary;
          d.compactions += 1;
          l.turns.forEach((t) => (t.compacted = true));
          l.compactions.unshift({ id: newId(), at: now(), source: "operator", summary, through_turn_id: l.turns.length });
          this.conversationEvent(slug);
        });
        return { ok: true, detail: "Compaction started." };
      }
    }
    throw new HttpError(404, `The mock has no route for ${method} ${path}.`);
  }

  // ------------------------------------------------------------------ phone (ADR 0016)
  /** A fresh six-digit code that replaces the last one; with ?phone=pairs the phone spends it. */
  private newPairing(): PhonePairing {
    if (!this.phone.address) throw new HttpError(409, "the phone listener is off (set GHOST_PHONE_LISTEN)");
    const code = String(Math.floor(rand() * 1_000_000)).padStart(6, "0");
    const ttl = this.phoneOpts.codeTtlSecs ?? 120;
    const addresses = this.phone.addresses?.length ? this.phone.addresses : [this.phone.address];
    const p: PhonePairing = { code, address: this.phone.address, addresses, expires_at: new Date(Date.now() + ttl * 1000).toISOString(), link: pairLink(code, addresses) };
    this.pairing = p;
    const after = this.phoneOpts.pairsAfterMs ?? 0;
    if (after > 0) {
      this.later(after, () => {
        if (this.pairing?.code !== code || Date.now() > new Date(p.expires_at).getTime()) return;
        this.pairing = null;
        // The phone's first /hello comes in on the home address, right after it pairs.
        const device = { id: hex(12, rand), name: "Pixel 9", paired_at: now(), last_seen: now(), last_address: addresses[0], last_network: "home" as const };
        this.phone = { ...this.phone, device };
      });
    }
    return p;
  }

  dispose() {
    for (const t of this.timers) clearTimeout(t);
    this.listeners.clear();
  }
}
