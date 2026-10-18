/**
 * An in-memory ghostd: every route the client uses, plus the event stream. Mutations
 * update this state and emit the same events the real daemon would, so screens behave
 * as they will against the real thing. Foundation-owned; data lives in fixtures/.
 */
import type {
  AbilityInfo,
  AuditEntry,
  ConversationDetail,
  ConversationView,
  DevicesView,
  GhostEvent,
  ModelsView,
  OutcomeView,
  PendingProposal,
  PolicyUpdate,
  PolicyView,
  SettingsUpdate,
  SettingsView,
  Status,
  Turn,
  VoiceInfo,
} from "@ghost/client-ts";

import type { LineRecord } from "./fixtures/conversations";
import type { Fixtures } from "./fixtures";
import { renderToml } from "./fixtures/policy";
import type { SessionFixture } from "./fixtures/session";
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
 * The steps the gate records for a write, worded as the real ability's Change.describe()
 * does: "closed the issue", "comment: …", "labels: …". A comment carried in the summary
 * ("…, with a comment: “…”") becomes its own step.
 */
function stepsOf(operation: string, summary: string): string[] {
  const comment = /with a comment:\s*[“"]?([\s\S]*?)[”"]?\.?$/.exec(summary)?.[1]?.trim();
  const main: Record<string, string | null> = { close: "closed the issue", reopen: "open the issue", comment: null, approve: "approve", request_changes: "request_changes", set_labels: "set_labels" };
  // The comment lands first, then the state change, as the ability orders its steps.
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

export class MockDaemon {
  readonly scenario: Scenario;
  private bootedAt = Date.now();
  private listeners = new Set<Listener>();
  private status: Status;
  private turns: Turn[];
  private pending: PendingProposal | null;
  private lines = new Map<string, LineRecord>();
  private abilities: AbilityInfo[];
  private policy: PolicyView;
  private audit: AuditEntry[];
  private settings: SettingsView;
  private devices: DevicesView;
  private models: ModelsView;
  private voices: VoiceInfo[];
  private script: SessionFixture;
  private timers = new Set<ReturnType<typeof setTimeout>>();

  constructor(scenario: Scenario, f: Fixtures) {
    this.scenario = scenario;
    this.script = f.session;
    this.turns = f.session.turns;
    this.pending = f.session.pending;
    for (const l of f.conversations.lines) this.lines.set(l.detail.slug, l);
    this.voices = f.conversations.voices;
    this.abilities = f.abilities;
    this.policy = f.policy;
    this.audit = f.audit;
    this.settings = f.settings.settings;
    this.devices = f.settings.devices;
    this.models = f.settings.models;
    this.status = {
      phase: scenario.phase,
      session_id: f.session.sessionId,
      models_ready: scenario.modelsReady,
      uptime_secs: 0,
      version: "0.1.0",
      api_version: "1",
      wake_mode: "transcript",
      abilities: this.abilities.filter((a) => a.enabled && a.version !== "built-in").map((a) => a.name),
      pending: this.pending,
      conversation: scenario.session,
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
        undo_of: null,
        created_at: new Date(Date.now() - 8_000).toISOString(),
        events: [{ at: new Date(Date.now() - 8_000).toISOString(), event: "prepared", detail: { ability: this.pending.ability, operation: this.pending.operation, target: `github:${this.pending.target}`, autonomy: "confirm" } }],
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
  private wake() {
    this.requireReady();
    if (this.status.session_id) return;
    this.turns = [];
    this.setActive("ghost");
    this.pushStatus({ session_id: newId(), phase: "listening", conversation: "ghost" });
    this.addTurn("assistant", "Ghost here.", "system");
  }
  private setActive(slug: string | null) {
    for (const l of this.lines.values()) l.detail.active = l.detail.slug === slug;
  }
  private say(text: string) {
    if (!this.status.session_id) this.wake();
    this.addTurn("user", text, "text");
    this.pushStatus({ phase: "thinking" });
    this.later(900, () => {
      const patch = /(?:patch me (?:through )?to|switch to|back to)\s+(?:the\s+)?([\w -]+?)(?:\s+line)?[.!?]*$/i.exec(text);
      if (patch) {
        const want = patch[1].trim().toLowerCase();
        const line = [...this.lines.values()].find((l) => l.detail.slug === want || l.detail.name.toLowerCase() === want || l.detail.aliases.includes(want));
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
    this.pending = { ...p, id: newId(), expires_at: new Date(Date.now() + 60_000).toISOString(), expires_in_secs: 60 };
    this.audit.unshift({
      id: this.pending.id, session_id: this.status.session_id ?? "", ability: p.ability, operation: p.operation, target: p.target,
      summary: p.summary, autonomy: "confirm", status: "pending", undo_of: null, created_at: now(),
      events: [{ at: now(), event: "prepared", detail: { ability: p.ability, operation: p.operation, target: `github:${p.target}`, autonomy: "confirm" } }],
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
    this.pushStatus({ session_id: null, phase: "passive", conversation: null });
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
  private undo(): OutcomeView {
    const last = this.audit.find((a) => a.status === "executed" && !a.undo_of && Date.now() - new Date(a.created_at).getTime() < 10 * 60_000);
    if (!last) throw new HttpError(409, "Nothing to undo from the last ten minutes.");
    last.status = "undone";
    const id = newId();
    last.events.push({ at: now(), event: "undone", detail: { by: id } });
    this.audit.unshift({ ...last, id, status: "executed", undo_of: last.id, summary: `Undo: ${last.summary}`, autonomy: "announce", created_at: now(), events: [{ at: now(), event: "prepared", detail: { undo_of: last.id } }, { at: now(), event: "finished", detail: { status: "success" } }] });
    const outcome: OutcomeView = { proposal_id: id, status: "success", summary: `Undid: ${last.summary}`, target: last.target, applied: ["undo"], state_now: null, error: null, failed_step: null };
    this.emit({ type: "outcome", outcome, via: "undo" });
    if (this.status.session_id) this.reply("Undone.");
    return outcome;
  }

  // ------------------------------------------------------------------ conversations
  private view(d: ConversationDetail): ConversationView {
    const { slug, name, aliases, voice, speed, created_at, last_active_at, active, is_ghost } = d;
    return { slug, name, aliases, voice, speed, created_at, last_active_at, active, is_ghost };
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
    if (this.status.conversation !== slug) this.addTurn("assistant", target.detail.is_ghost ? "Back to Ghost." : `Patching you through to ${target.detail.name}.`, "system");
    this.setActive(slug);
    this.pushStatus({ conversation: slug });
    this.conversationEvent(slug);
  }
  private audited(ability: string, operation: string, target: string, summary: string) {
    this.audit.unshift({
      id: newId(), session_id: this.status.session_id ?? "ui", ability, operation, target, summary, autonomy: "announce", status: "executed", undo_of: null, created_at: now(),
      events: [{ at: now(), event: "prepared", detail: { ability, operation, target } }, { at: now(), event: "claimed" }, { at: now(), event: "finished", detail: { status: "success" } }],
    });
  }

  // ------------------------------------------------------------------ router
  async handle(method: string, path: string, query: URLSearchParams, body: unknown): Promise<unknown> {
    const b = (body ?? {}) as Record<string, unknown>;
    const m = (re: RegExp) => re.exec(path);
    let r: RegExpExecArray | null;

    if (method === "GET" && path === "/status") return this.snapshot();
    if (method === "POST" && path === "/wake") {
      this.wake();
      if (typeof b.text === "string" && b.text) this.say(b.text);
      return { ok: true };
    }
    if (method === "POST" && path === "/sleep") return this.sleepNow(), { ok: true };
    if (method === "POST" && path === "/session/say") {
      this.requireReady();
      this.say(String(b.text ?? ""));
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
    if (method === "POST" && path === "/undo") return this.undo();
    if (method === "GET" && path === "/audit") {
      let items = this.audit;
      const ability = query.get("ability"), status = query.get("status"), sid = query.get("session_id");
      if (ability) items = items.filter((e) => e.ability === ability);
      if (status) items = items.filter((e) => e.status === status);
      if (sid) items = items.filter((e) => e.session_id === sid);
      const limit = Number(query.get("limit") ?? 50), offset = Number(query.get("offset") ?? 0);
      return { items: items.slice(offset, offset + limit), total: items.length, limit, offset };
    }

    if (method === "GET" && path === "/abilities") return this.abilities;
    if (method === "GET" && path === "/policy") return this.policy;
    if (method === "PUT" && path === "/policy") {
      const u = b as PolicyUpdate;
      let restart = false;
      if (u.text != null) {
        this.policy = { ...this.policy, text: u.text, exists: true };
      } else {
        if (u.skills) this.policy.skills = u.skills;
        if (u.default !== undefined) this.policy.default = u.default ?? null;
        if (u.targets) this.policy.targets = u.targets;
        if (u.enabled) {
          restart = true;
          this.policy.enabled = u.enabled;
          for (const a of this.abilities) a.enabled = u.enabled.includes(a.name) || a.version === "built-in";
        }
        this.policy = { ...this.policy, exists: true, text: renderToml(this.policy) };
      }
      for (const a of this.abilities) for (const op of a.operations) op.autonomy = this.policy.skills[a.name]?.[op.name] ?? op.autonomy;
      return { applied: !restart, restart_required: restart, policy: this.policy };
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

    if (method === "GET" && path === "/conversations") return [...this.lines.values()].map((l) => this.view(l.detail));
    if (method === "POST" && path === "/conversations") {
      const name = String(b.name ?? "").trim();
      if (!name) throw new HttpError(422, "A conversation needs a name.");
      const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
      if (this.lines.has(slug)) throw new HttpError(409, `There is already a conversation called ${name}.`);
      const detail: ConversationDetail = {
        slug, name, aliases: (b.aliases as string[]) ?? [], voice: (b.voice as string) ?? "af_heart", speed: (b.speed as number) ?? 1,
        persona: String(b.persona ?? ""), memory: "", notes: [], created_at: now(), last_active_at: null, active: false, is_ghost: false,
        turn_count: 0, compactions: 0, dir: `~/.local/share/ghost/conversations/${slug}`,
      };
      this.lines.set(slug, { detail, turns: [], compactions: [] });
      this.audited("conversations", "create", `conversation:${slug}`, `Started a new line: ${name}.`);
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
        for (const k of ["name", "aliases", "voice", "speed", "persona", "memory", "notes"] as const) {
          if (b[k] != null) (d as unknown as Record<string, unknown>)[k] = b[k];
        }
        this.audited("conversations", "update", `conversation:${slug}`, `On the ${d.name} line: updated ${Object.keys(b).join(", ")}.`);
        this.conversationEvent(slug);
        return d;
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

  dispose() {
    for (const t of this.timers) clearTimeout(t);
    this.listeners.clear();
  }
}
