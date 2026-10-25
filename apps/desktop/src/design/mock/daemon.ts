/**
 * An in-memory ghostd: every route the client uses, plus the event stream. Mutations
 * update this state and emit the same events the real daemon would, so screens behave
 * as they will against the real thing. Foundation-owned; data lives in fixtures/.
 */
import type {
  AuditEntry,
  ConversationView,
  DevicesView,
  GhostEvent,
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
  TemplateView,
  ToolkitUpdate,
  ToolkitView,
  Turn,
  VoiceInfo,
} from "@ghost/client-ts";

import type { Scope, ThreadDetail, ThreadFields } from "../../screens/conversations/threads";
import type { LineRecord } from "./fixtures/conversations";
import type { Fixtures } from "./fixtures";
import type { ServerFixture } from "./fixtures/mcp";
import { renderToml } from "./fixtures/policy";
import { parsePolicy } from "../../screens/policy/model";
import type { SessionFixture } from "./fixtures/session";
import { GHOST_SKILLS_DIR, type SkillFixture, missingOf } from "./fixtures/skills";
import { OWN_TEMPLATES_DIR, type TemplateFixture, needsOf } from "./fixtures/templates";
import { LEVELS, TOOLKIT_SEEDS, problemsOf, renderToolkit } from "./fixtures/toolkits";
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
  private lines = new Map<string, LineRecord>();
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
  /** The SKILL.md folders Ghost reads (ADR 0023): Ghost's own, and a project's read in place. */
  private skills: SkillFixture[];
  /** The `[toolkit.<name>]` tables in the policy file (ADR 0023), without their problems, which are read fresh. */
  private toolkits: Omit<ToolkitView, "problems">[];
  /** Department templates (ADR 0023): shipped and the operator's own; `needs` is read fresh. */
  private templates: TemplateFixture[];
  private script: SessionFixture;
  private timers = new Set<ReturnType<typeof setTimeout>>();

  constructor(scenario: Scenario, f: Fixtures, phoneOpts: PhoneOptions = {}) {
    this.scenario = scenario;
    this.phone = f.settings.phone;
    this.phoneOpts = phoneOpts;
    this.script = f.session;
    this.turns = f.session.turns;
    this.pending = f.session.pending;
    for (const l of f.conversations.lines) this.lines.set(l.detail.slug, l);
    this.voices = f.conversations.voices;
    this.servers = f.mcp;
    this.sources = f.sources;
    this.policy = f.policy;
    this.audit = f.audit;
    this.settings = f.settings.settings;
    this.devices = f.settings.devices;
    this.models = f.settings.models;
    this.skills = f.skills;
    this.toolkits = f.toolkits.map(({ problems: _p, ...t }) => t);
    this.templates = f.templates;
    if (this.toolkits.length) this.policy = { ...this.policy, text: this.policy.text.trimEnd() + "\n\n" + this.toolkits.map(renderToolkit).join("\n") };
    const first = f.conversations.lines.find((l) => l.detail.slug === scenario.session)?.detail ?? null;
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
      department: first ? this.departmentOf(first) : null,
      toolkit: null,
      session_origin: scenario.session ? (scenario.origin ?? "desk") : null,
      ending: false,
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
        toolkit: this.toolkitFor(this.pending.ability),
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
    const line = this.lines.get(this.status.conversation ?? "ghost");
    if (line && role !== "system") {
      line.turns.push({ id: line.turns.length + 1, role, text, at: t.at, session_id: t.session_id, compacted: false });
      line.detail.turn_count = line.turns.length;
      line.detail.last_active_at = t.at;
    }
  }
  private conversationEvent(changed: string | null) {
    this.emit({ type: "conversation", active: this.status.session_id ? (this.status.conversation ?? null) : null, changed });
  }

  // ------------------------------------------------------------------ session
  private requireReady() {
    if (!this.status.models_ready) throw new HttpError(409, "Ghost is still loading its voice models.");
  }
  /** A session opens in Ghost, or in the line a Call names; a settled or unknown line is refused, as the daemon does. */
  private wake(conversation?: string | null) {
    this.requireReady();
    if (this.status.session_id) return;
    const slug = conversation || "ghost";
    const target = this.lines.get(slug)?.detail;
    if (!target || target.settled_at) throw new HttpError(409, `There's no live line called ${slug}.`);
    this.turns = [];
    this.setActive(slug);
    this.pushStatus({ session_id: newId(), phase: "listening", conversation: slug, department: this.departmentOf(target), toolkit: null, session_origin: "desk" });
    this.addTurn("assistant", target.is_ghost ? "Ghost here." : `${target.name} here.`, "system");
  }
  private setActive(slug: string | null) {
    for (const l of this.lines.values()) l.detail.active = l.detail.slug === slug;
  }
  private say(text: string, conversation?: string | null) {
    if (!this.status.session_id) this.wake(conversation);
    this.addTurn("user", text, "text");
    this.pushStatus({ phase: "thinking" });
    this.later(900, () => {
      const patch = /(?:patch me (?:through )?to|switch to|back to)\s+(?:the\s+)?([\w -]+?)(?:\s+line)?[.!?]*$/i.exec(text);
      if (patch) {
        const want = patch[1].trim().toLowerCase();
        // A settled line has left voice routing (ADR 0019): its name no longer patches through.
        const line = [...this.lines.values()].find((l) => !l.detail.settled_at && (l.detail.slug === want || l.detail.name.toLowerCase() === want || l.detail.aliases.includes(want)));
        if (line) {
          this.switchTo(line.detail.slug);
          return this.reply(`${line.detail.is_ghost ? "Ghost" : line.detail.name} here.`);
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
    const where = { conversation: this.status.conversation ?? null, department: this.status.department ?? null, toolkit: this.toolkitFor(p.ability) };
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
    this.pushStatus({ session_id: null, phase: "passive", conversation: null, department: null, toolkit: null, session_origin: null });
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

  // ------------------------------------------------------------------ conversations
  private view(d: ThreadDetail): ConversationView & ThreadFields {
    const { slug, name, aliases, voice, speed, created_at, last_active_at, active, is_ghost, scope, settled_at, department, is_department, toolkits } = d;
    return { slug, name, aliases, voice, speed, created_at, last_active_at, active, is_ghost, scope, settled_at, department: department ?? null, is_department, toolkits: [...toolkits] };
  }
  /** A scope as the body carries it: per source, a list of strings. Anything else is ignored. */
  private scopeFrom(raw: unknown): Scope {
    const out: Scope = {};
    if (raw && typeof raw === "object") {
      for (const [k, items] of Object.entries(raw as Record<string, unknown>)) if (Array.isArray(items)) out[k] = items.map(String);
    }
    return out;
  }
  private line(slug: string): LineRecord {
    const l = this.lines.get(slug);
    if (!l) throw new HttpError(404, `No conversation called ${slug}.`);
    return l;
  }
  /** The old line speaks the transfer, as ghost_voice/conversation.py does, then the new one is active. */
  private switchTo(slug: string) {
    const target = this.line(slug);
    if (!this.status.session_id) throw new HttpError(409, "Wake Ghost first: switching patches a live session through.");
    if (target.detail.settled_at) throw new HttpError(409, `The ${target.detail.name} line is settled. Unsettle it to patch through.`);
    if (this.status.conversation !== slug) this.addTurn("assistant", target.detail.is_ghost ? "Back to Ghost." : `Patching you through to ${target.detail.name}.`, "system");
    this.setActive(slug);
    this.pushStatus({ conversation: slug, department: this.departmentOf(target.detail), toolkit: null });
    this.conversationEvent(slug);
  }
  /** The department a line belongs to (ADR 0023): itself when it is one, its parent for a topic line, none for Ghost. */
  private departmentOf(d: ThreadDetail): string | null {
    return d.is_ghost ? null : (d.department ?? d.slug);
  }
  /** The toolkit a write on `server` goes through from the live line's department: the first that grants the server, or none (Ghost, or a department that reaches everything). */
  private toolkitFor(server: string): string | null {
    const dept = this.status.department ? this.lines.get(this.status.department)?.detail : null;
    if (!dept || !dept.toolkits.length) return null;
    return dept.toolkits.find((n) => this.toolkits.find((t) => t.name === n)?.grants.some((g) => g.server === server)) ?? null;
  }
  private audited(ability: string, operation: string, target: string, summary: string, autonomy = "announce") {
    this.audit.unshift({
      id: newId(), session_id: this.status.session_id ?? "ui", ability, operation, target, summary, autonomy, status: "executed", created_at: now(),
      events: [{ at: now(), event: "prepared", detail: { ability, operation, target, autonomy } }, { at: now(), event: "claimed" }, { at: now(), event: "finished", detail: { status: "success" } }],
      conversation: this.status.conversation ?? null,
      department: this.status.department ?? null,
      toolkit: this.toolkitFor(ability),
    });
  }

  // ------------------------------------------------------------------ toolkits (ADR 0023)
  /** GET /toolkits as the daemon reports it: problems read fresh against the servers and skills of now. */
  private toolkitList(): ToolkitView[] {
    const names = new Set(this.skills.map((f) => f.info.name));
    return this.toolkits.map((t) => ({ ...structuredClone(t), problems: problemsOf(t.name, t, this.servers, names) }));
  }
  /** The policy file re-rendered from its structure (a structured save loses hand comments), toolkits last. */
  private policyText(): string {
    const base = renderToml(this.policy, this.servers).trimEnd();
    return this.toolkits.length ? base + "\n\n" + this.toolkits.map(renderToolkit).join("\n") : base + "\n";
  }
  /** PUT /toolkits/{name}: the file is rewritten with the toolkit and read back; what is wrong with it comes back as problems, not an error. */
  private putToolkit(name: string, u: ToolkitUpdate): ToolkitView {
    if (!/^[a-z0-9][a-z0-9_-]{0,39}$/.test(name)) throw new HttpError(422, "a toolkit name is letters, digits, _ or -");
    const grants = (u.grants ?? []).map((g) => ({ server: String(g.server), reads: g.reads ?? null, writes: g.writes ?? null, items: g.items ?? null, levels: { ...(g.levels ?? {}) } }));
    for (const g of grants) for (const lv of Object.values(g.levels)) if (!(LEVELS as readonly string[]).includes(lv)) throw new HttpError(422, `${JSON.stringify(lv)} is not a level`);
    const t: Omit<ToolkitView, "problems"> = { name, description: u.description ?? "", skills: [...(u.skills ?? [])], grants };
    const i = this.toolkits.findIndex((x) => x.name === name);
    if (i >= 0) this.toolkits[i] = t;
    else this.toolkits.push(t);
    this.policy = { ...this.policy, exists: true, text: this.policyText() };
    return this.toolkitList().find((x) => x.name === name)!;
  }
  private deleteToolkit(name: string) {
    if (!this.toolkits.some((t) => t.name === name)) throw new HttpError(404, `no toolkit ${JSON.stringify(name)}`);
    this.toolkits = this.toolkits.filter((t) => t.name !== name);
    this.policy = { ...this.policy, text: this.policyText() };
  }
  // ------------------------------------------------------------------ templates (ADR 0023)
  /** GET /templates: `needs` worked out against the servers declared now. */
  private templateList(): TemplateView[] {
    const declared = new Set(this.servers.map((f) => f.info.name));
    return this.templates.map((t) => ({ ...t.view, needs: needsOf(t.view.toolkits, t.requires, declared) }));
  }
  /**
   * POST /templates/{name}/apply: the template's toolkits into the file (the ones not there
   * yet), its skills into Ghost's (the ones not installed), then the department, each step
   * audited as the daemon does. 404 for no such template, 409 for a name already taken.
   */
  private applyTemplate(name: string, wanted: string | null): ThreadDetail {
    const t = this.templates.find((x) => x.view.name === name);
    if (!t) throw new HttpError(404, `no template ${JSON.stringify(name)}`);
    for (const tk of t.view.toolkits) {
      if (this.toolkits.some((x) => x.name === tk)) continue;
      const seed = TOOLKIT_SEEDS[tk];
      this.toolkits.push(seed ? structuredClone(seed) : { name: tk, description: `From the ${t.view.title} template.`, skills: [], grants: [] });
    }
    if (t.view.toolkits.some((tk) => !this.policy.text.includes(`[toolkit.${tk}]`))) this.policy = { ...this.policy, exists: true, text: this.policyText() };
    for (const sk of t.view.skills) {
      if (this.skillNamed(sk)) continue;
      const title = sk.replace(/[-.]+/g, " ");
      this.skills.push({
        info: { name: sk, description: `${title[0].toUpperCase()}${title.slice(1)}, from the ${t.view.title} template.`, source: "ghost", path: `${GHOST_SKILLS_DIR}/${sk}`, requires: t.requires, missing: [], files: [] },
        body: `# ${title[0].toUpperCase()}${title.slice(1)}\n\nCopied from the ${t.view.title} template (${t.view.path}/skills/${sk}).\n`,
      });
      this.audited("skills", "add", `skill:${sk}`, `Added the ${sk} skill from the ${t.view.title} template.`, "quiet");
    }
    const title = (wanted ?? "").trim() || t.view.title;
    const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
    if (this.lines.has(slug)) throw new HttpError(409, `there is already a line called ${JSON.stringify(title)}`);
    const detail: ThreadDetail = {
      slug, name: title, aliases: [...t.view.aliases], voice: t.view.voice ?? "af_heart", speed: 1,
      persona: t.persona, memory: "", notes: [], created_at: now(), last_active_at: null, active: false, is_ghost: false,
      turn_count: 0, compactions: 0, dir: `~/.local/share/ghost/conversations/${slug}`,
      scope: {}, settled_at: null, department: null, is_department: true, toolkits: this.validToolkits(t.view.toolkits),
    };
    this.lines.set(slug, { detail, turns: [], compactions: [] });
    this.audited("conversations", "create", `conversation:${slug}`, `Started a new department from the ${t.view.title} template: ${title}.`);
    this.conversationEvent(slug);
    return detail;
  }
  /** POST /conversations/{slug}/export: a department written out as one of the operator's own templates. */
  private exportTemplate(d: ThreadDetail): TemplateView {
    if (d.is_ghost) throw new HttpError(409, "Ghost is the switchboard, not a department");
    if (d.department) throw new HttpError(409, `${d.name} is a line in a department; export the department`);
    const skills = [...new Set(d.toolkits.flatMap((n) => this.toolkits.find((t) => t.name === n)?.skills ?? []))];
    const requires = [...new Set(skills.flatMap((sk) => this.skillNamed(sk)?.info.requires ?? []))];
    const view: Omit<TemplateView, "needs"> = { name: d.slug, title: d.name, description: d.persona.split("\n").map((x) => x.trim()).find(Boolean) ?? "", source: "ghost", path: `${OWN_TEMPLATES_DIR}/${d.slug}`, aliases: [...d.aliases], voice: d.voice, toolkits: [...d.toolkits], skills };
    const fixture: TemplateFixture = { view: { ...view, needs: [] }, persona: d.persona, requires };
    const i = this.templates.findIndex((t) => t.view.name === d.slug && t.view.source === "ghost");
    if (i >= 0) this.templates[i] = fixture;
    else this.templates.push(fixture);
    this.audited("conversations", "export", `conversation:${d.slug}`, `Saved the ${d.name} department as a template at ${view.path}.`, "quiet");
    return this.templateList().find((t) => t.name === d.slug && t.source === "ghost")!;
  }

  /** The names a department may be given (ADR 0023): only the file's, case-insensitively, 422 otherwise as ghost_core does. */
  private validToolkits(raw: unknown): string[] {
    const known = new Map(this.toolkits.map((t) => [t.name.toLowerCase(), t.name]));
    const out: string[] = [];
    for (const n of Array.isArray(raw) ? raw.map(String) : []) {
      const hit = known.get(n.trim().toLowerCase());
      if (!hit) throw new HttpError(422, `there is no toolkit called ${n}; the policy file declares ${[...known.values()].join(", ") || "none"}`);
      if (!out.includes(hit)) out.push(hit);
    }
    return out;
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
      info: { name, description: `${title[0].toUpperCase()}${title.slice(1)}, added from ${folder}.`, source: "ghost", path: `${GHOST_SKILLS_DIR}/${name}`, requires: [], missing: [], files: [] },
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
        this.policy = { ...this.policy, exists: true, text: this.policyText() };
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

    if (method === "GET" && path === "/templates") return this.templateList();
    if (method === "POST" && (r = m(/^\/templates\/([^/]+)\/apply$/))) return this.applyTemplate(decodeURIComponent(r[1]), typeof b.name === "string" ? b.name : null);

    if (method === "GET" && path === "/toolkits") return this.toolkitList();
    if ((r = m(/^\/toolkits\/([^/]+)$/))) {
      const name = decodeURIComponent(r[1]);
      if (method === "PUT") return this.putToolkit(name, (b ?? {}) as ToolkitUpdate);
      if (method === "DELETE") return this.deleteToolkit(name), { ok: true, detail: null };
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

    if (method === "GET" && path === "/conversations") return [...this.lines.values()].map((l) => this.view(l.detail));
    if (method === "POST" && path === "/conversations") {
      const name = String(b.name ?? "").trim();
      if (!name) throw new HttpError(422, "A conversation needs a name.");
      const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
      if (this.lines.has(slug)) throw new HttpError(409, `There is already a conversation called ${name}.`);
      // seed_from (ADR 0019): a settled line's memory is the new line's starting memory, a handoff to edit.
      let memory = "";
      const seed = typeof b.seed_from === "string" && b.seed_from ? this.lines.get(b.seed_from) : null;
      if (b.seed_from && !seed) throw new HttpError(404, `No conversation called ${String(b.seed_from)} to start from.`);
      if (seed) memory = seed.detail.memory;
      // ADR 0023: inside a department the line reaches what the department reaches, so it takes no toolkits of its own.
      const depRaw = typeof b.department === "string" ? b.department.trim() : "";
      const parent = depRaw ? this.lines.get(depRaw) ?? [...this.lines.values()].find((l) => l.detail.name.toLowerCase() === depRaw.toLowerCase()) ?? null : null;
      if (depRaw && (!parent || parent.detail.is_ghost || parent.detail.department)) throw new HttpError(422, `${depRaw} is not a department`);
      const toolkits = this.validToolkits(b.toolkits);
      if (parent && toolkits.length) throw new HttpError(422, "a line in a department reaches what the department reaches");
      const detail: ThreadDetail = {
        slug, name, aliases: (b.aliases as string[]) ?? [], voice: (b.voice as string) ?? "af_heart", speed: (b.speed as number) ?? 1,
        persona: String(b.persona ?? ""), memory, notes: [], created_at: now(), last_active_at: null, active: false, is_ghost: false,
        turn_count: 0, compactions: 0, dir: `~/.local/share/ghost/conversations/${slug}`,
        scope: this.scopeFrom(b.scope), settled_at: null,
        department: parent?.detail.slug ?? null, is_department: !parent, toolkits,
      };
      this.lines.set(slug, { detail, turns: [], compactions: [] });
      this.audited("conversations", "create", `conversation:${slug}`, seed ? `Started a new line: ${name}, from the ${seed.detail.name} line’s memory.` : parent ? `Started a new line in ${parent.detail.name}: ${name}.` : `Started a new line: ${name}.`);
      this.conversationEvent(slug);
      return detail;
    }
    if ((r = m(/^\/conversations\/([^/]+)(\/[a-z]+)?$/))) {
      const slug = decodeURIComponent(r[1]);
      const sub = r[2] ?? "";
      const l = this.line(slug);
      const d = l.detail;
      if (method === "GET" && sub === "") return d;
      if (method === "GET" && sub === "/turns") return l.turns.slice(-Number(query.get("limit") ?? 200));
      if (method === "GET" && sub === "/compactions") return l.compactions;
      if (method === "PATCH" && sub === "") {
        if (d.settled_at) throw new HttpError(409, `The ${d.name} line is settled. Unsettle it to change it.`);
        for (const k of ["name", "aliases", "voice", "speed", "persona", "memory", "notes"] as const) {
          if (b[k] != null) (d as unknown as Record<string, unknown>)[k] = b[k];
        }
        if (b.scope != null) {
          if (d.is_ghost) throw new HttpError(409, "Ghost is unscoped: it sees everything.");
          d.scope = this.scopeFrom(b.scope);
        }
        this.audited("conversations", "update", `conversation:${slug}`, `On the ${d.name} line: updated ${Object.keys(b).join(", ")}.`);
        this.conversationEvent(slug);
        return d;
      }
      if (method === "POST" && sub === "/export") return this.exportTemplate(d);
      // A department's toolkits (ADR 0023): an audited write; Ghost and topic lines take none.
      if (method === "PUT" && sub === "/toolkits") {
        if (d.is_ghost) throw new HttpError(409, "Ghost reaches everything; it takes no toolkits");
        if (d.department) throw new HttpError(409, `${d.name} is a line in a department; set the department's`);
        if (d.settled_at) throw new HttpError(409, `The ${d.name} line is settled. Unsettle it to change it.`);
        d.toolkits = this.validToolkits(b.toolkits);
        this.audited("conversations", "set_toolkits", `conversation:${slug}`, `On the ${d.name} line: toolkits set to ${d.toolkits.length ? d.toolkits.join(", ") : "everything"}.`);
        this.conversationEvent(slug);
        return d;
      }
      // Settle and unsettle (ADR 0019): announce-level, nothing is destroyed.
      if (method === "POST" && sub === "/settle") {
        if (d.is_ghost) throw new HttpError(409, "Ghost is the main line and can't be settled.");
        if (d.active) throw new HttpError(409, "Switch away from this line before settling it.");
        if (!d.settled_at) {
          d.settled_at = now();
          this.audited("conversations", "settle", `conversation:${slug}`, `Settled the ${d.name} line.`);
          this.conversationEvent(slug);
        }
        return { ok: true, detail: `Settled the ${d.name} line.` };
      }
      if (method === "POST" && sub === "/unsettle") {
        if (d.settled_at) {
          d.settled_at = null;
          this.audited("conversations", "unsettle", `conversation:${slug}`, `Brought the ${d.name} line back.`);
          this.conversationEvent(slug);
        }
        return { ok: true, detail: `The ${d.name} line is back.` };
      }
      if (method === "POST" && sub === "/notes") {
        d.notes = [...d.notes, String(b.text ?? "")];
        this.audited("conversations", "remember", `conversation:${slug}`, `On the ${d.name} line: pinned “${b.text}”.`);
        this.conversationEvent(slug);
        return d;
      }
      if (method === "DELETE" && sub === "") {
        if (d.is_ghost) throw new HttpError(409, "Ghost itself can't be deleted.");
        if (d.active) throw new HttpError(409, "Switch away from this line before deleting it.");
        this.lines.delete(slug);
        this.audited("conversations", "delete", `conversation:${slug}`, `Deleted the ${d.name} line.`);
        this.conversationEvent(slug);
        return { ok: true };
      }
      if (method === "POST" && sub === "/reset") {
        if (d.active) throw new HttpError(409, "Switch away from this line before resetting it.");
        Object.assign(d, { memory: "", notes: [], turn_count: 0, compactions: 0 });
        l.turns = [];
        l.compactions = [];
        this.audited("conversations", "reset", `conversation:${slug}`, `Reset the ${d.name} line.`);
        this.conversationEvent(slug);
        return { ok: true };
      }
      if (method === "POST" && sub === "/switch") return this.switchTo(slug), { ok: true };
      if (method === "POST" && sub === "/compact") {
        if (!d.active) throw new HttpError(409, "Compaction runs in a live session on this line.");
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
