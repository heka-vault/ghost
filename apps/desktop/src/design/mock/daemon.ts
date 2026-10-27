/**
 * An in-memory ghostd: every route the client uses, plus the event stream. Mutations
 * update this state and emit the same events the real daemon would, so screens behave
 * as they will against the real thing. Foundation-owned; data lives in fixtures/.
 */
import type {
  AuditEntry,
  ConversationDetail,
  ConversationView,
  DepartmentView,
  DevicesView,
  GhostEvent,
  JournalEntry,
  McpServerInfo,
  ModelsView,
  OutcomeView,
  PendingProposal,
  PhonePairing,
  PhoneView,
  PolicyUpdate,
  PolicyView,
  SettingsUpdate,
  SettingsView,
  SkillInfo,
  SourceInfo,
  Status,
  Turn,
  VoiceInfo,
} from "@ghost/client-ts";

import type { AgentRecord } from "./fixtures/conversations";
import { CONFIG_DIR } from "./fixtures/departments";
import type { Fixtures } from "./fixtures";
import type { ServerFixture } from "./fixtures/mcp";
import { renderToml } from "./fixtures/policy";
import { parsePolicy } from "../../screens/policy/model";
import type { SessionFixture } from "./fixtures/session";
import { GHOST_SKILLS_DIR, type SkillFixture, missingOf } from "./fixtures/skills";
import { hex, type Scenario, seeded } from "./scenarios";

export class HttpError extends Error {
  constructor(public status: number, public detail: string) {
    super(detail);
  }
}

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

/** Mock-only knobs for the phone (ADR 0016), from the URL: see mock/index.ts. */
export interface PhoneOptions {
  /** The phone pairs this many ms after a code is issued (?phone=pairs). 0: it never does. */
  pairsAfterMs?: number;
  /** How long a pairing code lasts, in seconds (?phone=expires makes it short). The real one: 120. */
  codeTtlSecs?: number;
}

export class MockDaemon {
  readonly scenario: Scenario;
  private bootedAt = Date.now();
  private listeners = new Set<Listener>();
  private status: Status;
  private turns: Turn[];
  private pending: PendingProposal | null;
  private agents = new Map<string, AgentRecord>();
  /** The department folders (ADR 0024), without their agents, which are read off the agents. */
  private departments: Omit<DepartmentView, "agents">[];
  /** The declared servers (GET /mcp) and what the gate knows (GET /sources), ADR 0022. */
  private servers: ServerFixture[];
  private sources: SourceInfo[];
  private policy: PolicyView;
  private audit: AuditEntry[];
  private settings: SettingsView;
  private devices: DevicesView;
  private models: ModelsView;
  private voices: VoiceInfo[];
  private phone: PhoneView;
  private pairing: PhonePairing | null = null;
  private phoneOpts: PhoneOptions;
  /** The SKILL.md folders Ghost reads (ADR 0023): Ghost's own and the operator's. */
  private skills: SkillFixture[];
  private script: SessionFixture;
  private timers = new Set<ReturnType<typeof setTimeout>>();

  constructor(scenario: Scenario, f: Fixtures, phoneOpts: PhoneOptions = {}) {
    this.scenario = scenario;
    this.phone = f.settings.phone;
    this.phoneOpts = phoneOpts;
    this.script = f.session;
    this.turns = f.session.turns;
    this.pending = f.session.pending;
    for (const l of f.conversations.agents) this.agents.set(l.detail.slug, l);
    this.departments = f.departments;
    this.voices = f.conversations.voices;
    this.servers = f.mcp;
    this.sources = f.sources;
    this.policy = f.policy;
    this.audit = f.audit;
    this.settings = f.settings.settings;
    this.devices = f.settings.devices;
    this.models = f.settings.models;
    this.skills = f.skills;
    const first = f.conversations.agents.find((l) => l.detail.slug === scenario.session)?.detail ?? null;
    this.status = {
      phase: scenario.phase,
      session_id: f.session.sessionId,
      models_ready: scenario.modelsReady,
      uptime_secs: 0,
      version: "0.1.0",
      api_version: "1",
      wake_mode: "transcript",
      servers: this.servers.filter((f) => f.info.connected).map((f) => f.info.name),
      pending: this.pending,
      conversation: scenario.session,
      department: first ? first.department : null,
      session_origin: scenario.session ? (scenario.origin ?? "desk") : null,
      ending: false,
      waiting: 0,
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
        ability: this.pending.ability,
        operation: this.pending.operation,
        target: this.pending.target,
        summary: this.pending.summary,
        autonomy: "confirm",
        status: "pending",
        created_at: new Date(Date.now() - 8_000).toISOString(),
        events: [{ at: new Date(Date.now() - 8_000).toISOString(), event: "prepared", detail: { ability: this.pending.ability, operation: this.pending.operation, target: `github:${this.pending.target}`, autonomy: "confirm" } }],
        conversation: this.status.conversation ?? null,
        department: this.status.department ?? null,
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
    this.pushStatus({ session_id: newId(), phase: "listening", conversation: slug, department: target.department, session_origin: "desk" });
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
      const patch = /(?:patch me (?:through )?to|transfer me to|switch to|get me|back to)\s+(?:the\s+)?([\w -]+?)(?:\s+(?:line|agent|department))?[.!?]*$/i.exec(text);
      if (patch) {
        const want = patch[1].trim().toLowerCase();
        // A retired agent has left voice routing (ADR 0024): its name no longer transfers.
        const agent =
          [...this.agents.values()].find((l) => !l.detail.retired_at && (l.detail.slug === want || l.detail.name.toLowerCase() === want || l.detail.aliases.includes(want))) ??
          // A department's name lands on its own agent.
          [...this.agents.values()].find((l) => l.detail.is_default && this.departments.some((d) => d.slug === l.detail.department && (d.slug === want || d.title.toLowerCase() === want || d.aliases.includes(want))));
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
    const where = { conversation: this.status.conversation ?? null, department: this.status.department ?? null };
    this.pending = { ...p, id: newId(), expires_at: new Date(Date.now() + 60_000).toISOString(), expires_in_secs: 60 };
    this.audit.unshift({
      id: this.pending.id, session_id: this.status.session_id ?? "", ability: p.ability, operation: p.operation, target: p.target,
      summary: p.summary, autonomy: "confirm", status: "pending", created_at: now(),
      events: [{ at: now(), event: "prepared", detail: { ability: p.ability, operation: p.operation, target: `github:${p.target}`, autonomy: "confirm" } }],
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
    this.pushStatus({ session_id: null, phase: "passive", conversation: null, department: null, session_origin: null });
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

  // ------------------------------------------------------------------ agents (ADR 0024)
  private view(d: ConversationDetail): ConversationView {
    const { id, slug, name, aliases, department, job, voice, speed, own_voice, created_at, last_active_at, retired_at, active, is_ghost, is_default } = d;
    return { id, slug, name, aliases, department, job, voice, speed, own_voice, created_at, last_active_at, retired_at, active, is_ghost, is_default };
  }
  private agent(slug: string): AgentRecord {
    const l = this.agents.get(slug);
    if (!l) throw new HttpError(404, `No agent called ${slug}.`);
    return l;
  }
  private departmentNamed(raw: string | null | undefined): Omit<DepartmentView, "agents"> {
    const want = (raw ?? "ghost").trim().toLowerCase() || "ghost";
    const d = this.departments.find((x) => x.slug === want || x.title.toLowerCase() === want || x.aliases.map((a) => a.toLowerCase()).includes(want));
    if (!d) throw new HttpError(422, `there is no department called ${raw}`);
    return d;
  }
  private departmentVoice(slug: string): string {
    return this.departments.find((d) => d.slug === slug)?.voice ?? "af_heart";
  }
  /** The old agent speaks the transfer, as ghost_voice/conversation.py does, then the new one is active. */
  private switchTo(slug: string) {
    const target = this.agent(slug);
    if (!this.status.session_id) throw new HttpError(409, "Wake Ghost first: switching transfers a live session.");
    if (target.detail.retired_at) throw new HttpError(409, `${target.detail.name} is retired. Rehire it to transfer.`);
    if (this.status.conversation !== slug) this.addTurn("assistant", target.detail.is_ghost ? "Back to Ghost." : `Transferring you to ${target.detail.name}.`, "system");
    this.setActive(slug);
    this.pushStatus({ conversation: slug, department: target.detail.department });
    this.conversationEvent(slug);
  }
  private audited(ability: string, operation: string, target: string, summary: string, autonomy = "announce", department?: string | null) {
    this.audit.unshift({
      id: newId(), session_id: this.status.session_id ?? "ui", ability, operation, target, summary, autonomy, status: "executed", created_at: now(),
      events: [{ at: now(), event: "prepared", detail: { ability, operation, target, autonomy } }, { at: now(), event: "claimed" }, { at: now(), event: "finished", detail: { status: "success" } }],
      conversation: this.status.conversation ?? null,
      department: department === undefined ? (this.status.department ?? null) : department,
    });
  }

  // ------------------------------------------------------------------ departments (ADR 0024)
  /** GET /departments: each folder with its agents, the department's own first. */
  private departmentView(d: Omit<DepartmentView, "agents">): DepartmentView {
    const agents = [...this.agents.values()].filter((a) => a.detail.department === d.slug).map((a) => a.detail.slug).sort((a, b) => (a === d.slug ? -1 : b === d.slug ? 1 : a.localeCompare(b)));
    return { ...structuredClone(d), agents, servers_up: d.slug === "ghost" || d.slug === this.status.department };
  }
  private createDepartment(name: string, aliases: string[], voice: string | null): DepartmentView {
    const title = name.trim();
    if (!title) throw new HttpError(422, "A department needs a name.");
    const slug = slugOf(title);
    if (slug === "ghost" || this.departments.some((d) => d.slug === slug)) throw new HttpError(409, `there is already a department called ${JSON.stringify(title)}`);
    const d: Omit<DepartmentView, "agents"> = { slug, title, aliases, path: `${CONFIG_DIR}/departments/${slug}`, root: null, voice, is_ghost: false, rules_lead: title, rules: `# ${title}\n`, memory: "", servers: [], uses: [], servers_up: false, skills: [], docs: [], problems: [], watches: [], hooks: [] };
    this.departments.push(d);
    // Its own agent, named for it.
    const detail: ConversationDetail = {
      id: newId(), slug, name: title, aliases: [...aliases], department: slug, job: "", voice: voice ?? "af_heart", speed: 1, own_voice: false,
      persona: "", memory: "", notes: [], created_at: now(), last_active_at: null, retired_at: null, active: false, is_ghost: false, is_default: true,
      turn_count: 0, compactions: 0, dir: `${CONFIG_DIR}/departments/${slug}/agents/${slug}`,
    };
    this.agents.set(slug, { detail, turns: [], compactions: [] });
    this.conversationEvent(slug);
    return this.departmentView(d);
  }
  /** POST /departments/{slug}/rules: appended verbatim, as the gate does, audited. */
  private addRule(slug: string, text: string): DepartmentView {
    const d = this.departments.find((x) => x.slug === slug);
    if (!d) throw new HttpError(404, `no department ${JSON.stringify(slug)}`);
    if (d.root) throw new HttpError(409, `${d.title}'s rules live in ${d.root}/AGENTS.md; Ghost does not write into a project folder, so add the rule there`);
    const rule = text.trim();
    if (!rule) throw new HttpError(409, "the rule is required, in the operator's words");
    const current = d.rules.trim() ? d.rules.replace(/\n*$/, "\n") : `# ${d.title}\n`;
    d.rules = `${current}\n${rule}\n`;
    d.rules_lead = d.rules.split("\n").map((l) => l.replace(/^#+\s*/, "").trim()).find(Boolean) ?? "";
    this.audited("departments", "add_rule", `department:${slug}`, `On ${d.title}: add the rule: ${rule}.`, "confirm", slug);
    return this.departmentView(d);
  }
  /** GET /departments/{slug}/journal: executed writes made in the department, newest first, from the audit. */
  private journal(slug: string, limit: number): JournalEntry[] {
    return this.audit
      .filter((e) => e.status === "executed" && e.department === slug)
      .slice(0, limit)
      .map((e) => ({ at: e.created_at, agent: e.conversation ? this.agents.get(e.conversation)?.detail.name ?? e.conversation : "", operation: `${e.ability}.${e.operation}`, target: e.target, what: e.summary }));
  }

  // ------------------------------------------------------------------ skills (ADR 0023)
  /** GET /skills as the daemon reports it: `missing` re-read from the servers connected now. */
  private skillList(): SkillInfo[] {
    const connected = new Set(this.servers.filter((f) => f.info.connected).map((f) => f.info.name));
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
    if (method === "GET" && path === "/audit") {
      let items = this.audit;
      const ability = query.get("ability"), status = query.get("status"), sid = query.get("session_id");
      if (ability) items = items.filter((e) => e.ability === ability);
      if (status) items = items.filter((e) => e.status === status);
      if (sid) items = items.filter((e) => e.session_id === sid);
      const limit = Number(query.get("limit") ?? 50), offset = Number(query.get("offset") ?? 0);
      return { items: items.slice(offset, offset + limit), total: items.length, limit, offset };
    }

    if (method === "GET" && path === "/sources") return this.sources;
    if (method === "GET" && path === "/mcp") return this.servers.map((f): McpServerInfo => f.info);
    if (method === "GET" && path === "/policy") return this.policy;
    if (method === "PUT" && path === "/policy") {
      const u = b as PolicyUpdate;
      if (u.text != null) {
        // As the daemon does: the file is read before it is written, and refused when it can't be.
        const r = parsePolicy(u.text);
        if (!r.ok && r.certain) throw new HttpError(422, `request: ${r.message}`);
        this.policy = r.ok ? { ...this.policy, text: u.text, exists: true, default: r.data.default, skills: r.data.skills, targets: r.data.targets } : { ...this.policy, text: u.text, exists: true };
      } else {
        if (u.skills) this.policy.skills = u.skills;
        if (u.default !== undefined) this.policy.default = u.default ?? null;
        if (u.targets) this.policy.targets = u.targets;
        this.policy = { ...this.policy, exists: true, text: renderToml(this.policy, this.servers) };
      }
      // Levels apply live (ADR 0022): every source's operations and every server's declared
      // writes read the new file's levels; a write set to deny is no longer offered.
      for (const src of this.sources) for (const op of src.operations) op.autonomy = this.policy.skills[src.name]?.[op.name] ?? op.autonomy;
      for (const f of this.servers)
        for (const w of f.info.writes) {
          w.autonomy = this.policy.skills[f.info.name]?.[w.operation] ?? w.autonomy;
          w.offered = f.info.connected && w.autonomy !== "deny";
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

    if (method === "GET" && path === "/departments") return this.departments.map((d) => this.departmentView(d));
    if (method === "POST" && path === "/departments") return this.createDepartment(String(b.name ?? ""), Array.isArray(b.aliases) ? b.aliases.map(String) : [], typeof b.voice === "string" && b.voice ? b.voice : null);
    if ((r = m(/^\/departments\/([^/]+)(\/[a-z]+)?$/))) {
      const slug = decodeURIComponent(r[1]);
      const sub = r[2] ?? "";
      const d = this.departments.find((x) => x.slug === slug);
      if (!d) throw new HttpError(404, `no department ${JSON.stringify(slug)}`);
      if (method === "GET" && sub === "") return this.departmentView(d);
      if (method === "GET" && sub === "/journal") return this.journal(slug, Number(query.get("limit") ?? 30));
      if (method === "POST" && sub === "/rules") return this.addRule(slug, String(b.text ?? ""));
    }

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
      const dept = query.get("department"), retired = query.get("retired");
      if (dept) out = out.filter((a) => a.department === dept);
      if (retired != null) out = out.filter((a) => (a.retired_at != null) === (retired === "true"));
      return out;
    }
    if (method === "POST" && path === "/conversations") {
      const name = String(b.name ?? "").trim();
      if (!name) throw new HttpError(422, "An agent needs a name.");
      const slug = slugOf(name);
      if (this.agents.has(slug)) throw new HttpError(409, `There is already an agent called ${name}.`);
      if (this.departments.some((d) => d.slug === slug)) throw new HttpError(422, `${name} is a department's name; pick another`);
      const dept = this.departmentNamed(typeof b.department === "string" ? b.department : null);
      // seed_from (ADR 0024): a retired agent's memory is the new one's starting memory, a handoff to edit.
      let memory = typeof b.memory === "string" ? b.memory : "";
      const seed = typeof b.seed_from === "string" && b.seed_from ? this.agents.get(b.seed_from) : null;
      if (b.seed_from && !seed) throw new HttpError(422, `No agent called ${String(b.seed_from)} to start from.`);
      if (seed) memory = seed.detail.memory;
      const own = typeof b.voice === "string" && !!b.voice;
      const detail: ConversationDetail = {
        id: newId(), slug, name, aliases: (b.aliases as string[]) ?? [], department: dept.slug, job: String(b.job ?? ""),
        voice: own ? (b.voice as string) : this.departmentVoice(dept.slug), speed: (b.speed as number) ?? 1, own_voice: own,
        persona: String(b.persona ?? ""), memory, notes: [], created_at: now(), last_active_at: null, retired_at: null, active: false, is_ghost: false, is_default: false,
        turn_count: 0, compactions: memory ? 1 : 0, dir: dept.slug === "ghost" ? `${CONFIG_DIR}/agents/${slug}` : `${CONFIG_DIR}/departments/${dept.slug}/agents/${slug}`,
      };
      this.agents.set(slug, { detail, turns: [], compactions: memory ? [{ id: newId(), at: now(), source: "manual", summary: memory, through_turn_id: null }] : [] });
      this.audited("conversations", "create", `agent:${slug}`, seed ? `On ${slug.replace(/-/g, " ")}: hire ${JSON.stringify(name)} in ${dept.slug} with a handoff memory.` : `On ${slug.replace(/-/g, " ")}: hire ${JSON.stringify(name)} in ${dept.slug}${detail.job ? ` for ${detail.job}` : ""}.`, "announce", dept.slug);
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
        if (d.is_default && typeof b.name === "string" && slugOf(b.name) !== slug) throw new HttpError(409, `${d.name} is named for its department`);
        for (const k of ["name", "aliases", "job", "speed", "persona", "memory", "notes"] as const) {
          if (b[k] != null) (d as unknown as Record<string, unknown>)[k] = b[k];
        }
        if (typeof b.voice === "string") {
          d.own_voice = b.voice !== "";
          d.voice = b.voice || this.departmentVoice(d.department);
        }
        if (typeof b.memory === "string") {
          d.compactions += 1;
          l.compactions.unshift({ id: newId(), at: now(), source: "edit", summary: b.memory, through_turn_id: null });
        }
        this.audited("conversations", "update", `agent:${slug}`, `On ${spoken}: updated ${Object.keys(b).join(", ")}.`, "announce", d.department);
        this.conversationEvent(slug);
        return d;
      }
      // Retire and rehire (ADR 0024): announce-level, nothing is destroyed.
      if (method === "POST" && sub === "/retire") {
        if (d.is_default) throw new HttpError(409, `${d.name} is its department's own agent; it stays`);
        if (d.active) throw new HttpError(409, "Switch away from this agent before retiring it.");
        if (!d.retired_at) {
          d.retired_at = now();
          this.audited("conversations", "retire", `agent:${slug}`, `On ${spoken}: retire the agent.`, "announce", d.department);
          this.conversationEvent(slug);
        }
        return { ok: true, detail: `Retired ${d.name}.` };
      }
      if (method === "POST" && sub === "/rehire") {
        if (d.retired_at) {
          d.retired_at = null;
          this.audited("conversations", "rehire", `agent:${slug}`, `On ${spoken}: bring the agent back.`, "announce", d.department);
          this.conversationEvent(slug);
        }
        return { ok: true, detail: `${d.name} is back.` };
      }
      // Move (ADR 0024): the folder goes into another department; the voice follows unless its own.
      if (method === "POST" && sub === "/move") {
        if (d.is_default) throw new HttpError(409, `${d.name} is its department's own agent; it cannot move`);
        if (d.active) throw new HttpError(409, "the session is with this agent; ask Ghost to move it");
        const dept = this.departmentNamed(typeof b.department === "string" ? b.department : null);
        if (dept.slug === d.department) throw new HttpError(409, `${d.name} is already in ${dept.slug}`);
        const from = d.department;
        d.department = dept.slug;
        if (!d.own_voice) d.voice = this.departmentVoice(dept.slug);
        d.dir = dept.slug === "ghost" ? `${CONFIG_DIR}/agents/${slug}` : `${CONFIG_DIR}/departments/${dept.slug}/agents/${slug}`;
        this.audited("conversations", "move", `agent:${slug}`, `On ${spoken}: move to ${dept.slug}.`, "confirm", from);
        this.conversationEvent(slug);
        return d;
      }
      if (method === "POST" && sub === "/notes") {
        d.notes = [...d.notes, String(b.text ?? "")];
        this.audited("conversations", "remember", `agent:${slug}`, `On ${spoken}: remember: ${b.text}.`, "announce", d.department);
        this.conversationEvent(slug);
        return d;
      }
      if (method === "DELETE" && sub === "") {
        if (d.is_default) throw new HttpError(409, `${d.name} is its department's own agent; it stays`);
        if (d.active) throw new HttpError(409, "Switch away from this agent before deleting it.");
        this.agents.delete(slug);
        this.audited("conversations", "delete", `agent:${slug}`, `On ${spoken}: delete the agent.`, "confirm", d.department);
        this.conversationEvent(slug);
        return { ok: true };
      }
      if (method === "POST" && sub === "/reset") {
        if (d.active) throw new HttpError(409, "Switch away from this agent before resetting it.");
        Object.assign(d, { memory: "", notes: [], turn_count: 0, compactions: 0 });
        l.turns = [];
        l.compactions = [];
        this.audited("conversations", "reset", `agent:${slug}`, `On ${spoken}: reset: forget everything.`, "confirm", d.department);
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
    const p: PhonePairing = { code, address: this.phone.address, expires_at: new Date(Date.now() + ttl * 1000).toISOString() };
    this.pairing = p;
    const after = this.phoneOpts.pairsAfterMs ?? 0;
    if (after > 0) {
      this.later(after, () => {
        if (this.pairing?.code !== code || Date.now() > new Date(p.expires_at).getTime()) return;
        this.pairing = null;
        this.phone = { ...this.phone, device: { id: hex(12, rand), name: "Pixel 9", paired_at: now(), last_seen: now() } };
      });
    }
    return p;
  }

  dispose() {
    for (const t of this.timers) clearTimeout(t);
    this.listeners.clear();
  }
}
