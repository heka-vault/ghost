/**
 * An in-memory desk for building the phone before the WebRTC transport exists. It behaves
 * like the real one at the level the screens see: the link comes up, Start opens a session,
 * Ghost asks before a confirm-level change, a spoken yes or no settles it, the audit log
 * fills in. The voice side has no mic yet, so the Mock panel drives it through `act`.
 */
import type { AuditEntry, OutcomeView, PendingProposal, Status, Turn } from "@ghost/client-ts";

import type { ConversationView } from "@ghost/client-ts";

import type { Desk, DeskState, Link, Route } from "./types";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();
const id = () => Math.random().toString(16).slice(2) + Math.random().toString(16).slice(2);

const ASK_SECS = 45;

/** What Ghost asks in the mock, and what the gate writes when it is settled. */
const QUESTION = {
  ability: "github_heka",
  operation: "close",
  target: "ghost-mock/infra#77",
  summary: "On ghost-mock/infra#77: close the issue, with a comment: “Fixed by the runner image bump.”",
  done: "On ghost-mock/infra#77: closed the issue, with a comment: “Fixed by the runner image bump.”",
  rationale: "The runner image bump in #81 fixed the flaky job, and nobody has reported it since.",
  spoken: "Infra 77 looks fixed by the runner image bump. Close it with a note saying so?",
};

function entry(target: string, operation: string, summary: string, status: string, minutesAgo: number, extra: Partial<AuditEntry> = {}): AuditEntry {
  return {
    id: id(),
    session_id: "earlier",
    ability: "github_heka",
    operation,
    target,
    summary,
    autonomy: "announce",
    status,
    created_at: ago(minutesAgo),
    events: [{ at: ago(minutesAgo), event: "finished", detail: { status: "success" } }],
    ...extra,
  };
}

function seedAudit(): AuditEntry[] {
  return [
    entry("ghost-mock/app#208", "approve", "On ghost-mock/app#208: approved, with a comment: “Dana’s fix looks right to me.”", "executed", 60 * 3 + 4, {
      autonomy: "confirm",
      events: [
        { at: ago(60 * 3 + 4.2), event: "user_turn", detail: { verdict: "positive", classifier: "jev", text: "yeah, approve it" } },
        { at: ago(60 * 3 + 4), event: "finished", detail: { status: "success" } },
      ],
    }),
    entry("ghost-mock/app#199", "reopen", "On ghost-mock/app#199: reopened the issue.", "executed", 60 * 3 + 5, { conversation: "heka-workshop", department: "heka-workshop" }),
    entry("ghost-mock/app#212", "set_labels", "On ghost-mock/app#212: labels: add bug, priority-high.", "executed", 60 * 3 + 6, { conversation: "q3-audit", department: "accounting" }),
    entry("ghost-mock/app#199", "close", "On ghost-mock/app#199: closed the issue.", "executed", 60 * 3 + 9),
    entry("ghost-mock/infra#64", "close", "On ghost-mock/infra#64: close the issue, with a comment: “Duplicate of #61.”", "expired", 60 * 26, {
      autonomy: "confirm",
      events: [{ at: ago(60 * 26), event: "expired" }],
    }),
  ];
}

export type MockAct = "ask" | "yes" | "no" | "talk" | "drop" | "deskDown" | "deskUp" | "deskSession";

const MOCK_LINES: ConversationView[] = [
  { id: "a0", slug: "ghost", name: "Ghost", aliases: [], department: "ghost", job: "", voice: "af_heart", speed: 1, own_voice: false, created_at: "2026-10-01T09:00:00Z", last_active_at: "2026-10-20T13:00:00Z", retired_at: null, active: false, is_ghost: true, is_default: true },
  { id: "a1", slug: "heka-workshop", name: "Heka workshop", aliases: ["workshop"], department: "ghost", job: "the workshop", voice: "am_adam", speed: 1, own_voice: true, created_at: "2026-10-18T09:00:00Z", last_active_at: "2026-10-20T11:00:00Z", retired_at: null, active: false, is_ghost: false, is_default: false },
  { id: "a2", slug: "quotient-research", name: "Quotient research", aliases: [], department: "ghost", job: "", voice: "bf_emma", speed: 1, own_voice: true, created_at: "2026-10-19T09:00:00Z", last_active_at: "2026-10-19T18:00:00Z", retired_at: null, active: false, is_ghost: false, is_default: false },
  // A department's own agent and an agent hired into it (ADR 0024): the dial pad shows one key, which opens both.
  { id: "a3", slug: "accounting", name: "Accounting", aliases: ["the books"], department: "accounting", job: "", voice: "bm_george", speed: 1.05, own_voice: false, created_at: "2026-10-02T09:00:00Z", last_active_at: "2026-10-20T09:00:00Z", retired_at: null, active: false, is_ghost: false, is_default: true },
  { id: "a4", slug: "q3-audit", name: "Q3 audit", aliases: ["the audit"], department: "accounting", job: "the auditors’ questions", voice: "bm_george", speed: 1.05, own_voice: false, created_at: "2026-10-14T09:00:00Z", last_active_at: "2026-10-19T15:00:00Z", retired_at: null, active: false, is_ghost: false, is_default: false },
];

export class MockDesk implements Desk {
  private state: DeskState;
  private listeners = new Set<() => void>();
  private expiry: ReturnType<typeof setTimeout> | null = null;
  private muted = false;

  constructor() {
    this.state = { link: "connecting", via: "Wi-Fi", lastCall: null, audio: { route: "speaker", available: ["speaker", "earpiece"] }, conversations: MOCK_LINES, status: null, pending: null, turns: [], audit: null, auditTotal: 0, outcomes: {} };
    const audit = seedAudit();
    setTimeout(() => this.set({ link: "online", status: this.status({}), audit, auditTotal: audit.length }), 700);
  }

  getState = () => this.state;

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  private set(patch: Partial<DeskState>) {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  private status(patch: Partial<Status>): Status {
    return {
      servers: ["github"],
      api_version: "1",
      conversation: "ghost",
      models_ready: true,
      pending: null,
      phase: "passive",
      session_id: null,
      session_origin: null,
      ending: false,
      uptime_secs: 0,
      version: "0.1.0",
      wake_mode: "transcript",
      ...this.state?.status,
      ...patch,
    };
  }

  private phase(phase: Status["phase"]) {
    this.set({ status: this.status({ phase }) });
  }

  private reachable() {
    if (this.state.link !== "online") throw new Error("Your desk isn’t reachable right now.");
  }

  private say(role: Turn["role"], text: string) {
    const sessionId = this.state.status?.session_id ?? "";
    this.set({ turns: [...this.state.turns, { role, text, at: new Date().toISOString(), session_id: sessionId, via: role === "user" ? "voice" : "system" }] });
  }

  /** Ghost says something: speaking for a moment, then back to listening. */
  private async speak(text: string) {
    this.phase("speaking");
    this.say("assistant", text);
    await sleep(Math.min(4000, 900 + text.length * 30));
    if (this.state.status?.session_id) this.phase("listening");
  }

  private inSession() {
    return !!this.state.status?.session_id && this.state.status.session_origin !== "desk";
  }

  async start(conversation?: string) {
    this.reachable();
    if (this.state.status?.session_origin === "desk") throw new Error("Ghost is busy at your desk. End that session there, then start here.");
    await sleep(500);
    this.set({ status: this.status({ phase: "listening", session_id: id(), session_origin: "phone", conversation: conversation ?? "ghost" }), turns: [] });
    void this.speak("Ghost here. Two issues are waiting on you.");
  }

  setRoute(route: Route) {
    this.set({ audio: { ...this.state.audio, route } });
  }

  async end() {
    this.reachable();
    await sleep(300);
    this.clearExpiry();
    this.set({ status: this.status({ phase: "passive", session_id: null, session_origin: null, pending: null }), pending: null, turns: [] });
  }

  async retry() {
    await sleep(600);
    if (this.state.link === "offline") throw new Error("Still can’t reach it.");
  }

  async loadAudit(_limit: number) {
    await sleep(200);
  }

  setMuted(muted: boolean) {
    this.muted = muted;
  }

  /** The voice side and the network, for the Mock panel. */
  async act(what: MockAct) {
    switch (what) {
      case "ask":
        return this.ask();
      case "yes":
        return this.answer(true);
      case "no":
        return this.answer(false);
      case "talk":
        if (!this.inSession() || this.muted) return;
        this.say("user", "What else is on my plate today?");
        this.phase("thinking");
        await sleep(1400);
        return this.speak("Just the flaky runner on infra 77. Everything else is waiting on other people.");
      case "drop":
        this.set({ link: "reconnecting" });
        await sleep(6000);
        return this.set({ link: "online" });
      case "deskDown":
        return this.set({ link: "offline" });
      case "deskSession": {
        // Someone woke Ghost at the desk; toggles.
        if (this.state.status?.session_origin === "desk") return this.set({ status: this.status({ phase: "passive", session_id: null, session_origin: null }) });
        if (this.inSession()) return;
        return this.set({ status: this.status({ phase: "listening", session_id: id(), session_origin: "desk" }) });
      }
      case "deskUp":
        this.set({ link: "connecting" });
        await sleep(700);
        return this.set({ link: "online" });
    }
  }

  private async ask() {
    if (!this.inSession() || this.state.pending) return;
    this.phase("thinking");
    await sleep(900);
    const p: PendingProposal = {
      id: id(),
      ability: QUESTION.ability,
      operation: QUESTION.operation,
      target: QUESTION.target,
      summary: QUESTION.summary,
      rationale: QUESTION.rationale,
      expires_at: new Date(Date.now() + ASK_SECS * 1000).toISOString(),
      expires_in_secs: ASK_SECS,
    };
    this.set({ pending: p, status: this.status({ pending: p }) });
    this.clearExpiry();
    this.expiry = setTimeout(() => this.settle(p, "expired"), ASK_SECS * 1000);
    void this.speak(QUESTION.spoken);
  }

  /** The operator answers out loud; Jev classifies it in the pipeline. */
  private async answer(yes: boolean) {
    const p = this.state.pending;
    if (!p || this.muted || this.state.link !== "online") return;
    const said = yes ? "Yeah, go ahead." : "No, leave it.";
    this.say("user", said);
    this.phase("thinking");
    await sleep(700);
    this.settle(p, yes ? "executed" : "cancelled", said);
    void this.speak(yes ? "Closed infra 77 with your note." : "Okay, I left it alone.");
  }

  private settle(p: PendingProposal, status: "executed" | "cancelled" | "expired", said?: string) {
    if (this.state.pending?.id !== p.id) return;
    this.clearExpiry();
    const at = new Date().toISOString();
    const events: AuditEntry["events"] = said
      ? [{ at, event: "user_turn", detail: { verdict: status === "executed" ? "positive" : "negative", classifier: "jev", text: said.toLowerCase().replace(/\.$/, "") } }]
      : [];
    events.push(status === "expired" ? { at, event: "expired" } : { at, event: "finished", detail: { status: status === "executed" ? "success" : "cancelled" } });
    const row: AuditEntry = {
      id: p.id,
      session_id: this.state.status?.session_id ?? "",
      ability: p.ability,
      operation: p.operation,
      target: p.target,
      summary: status === "executed" ? QUESTION.done : p.summary,
      autonomy: "confirm",
      status,
        created_at: at,
      events,
    };
    const outcomes =
      status === "executed"
        ? { ...this.state.outcomes, [p.id]: { proposal_id: p.id, status: "success" as const, summary: QUESTION.done, target: p.target, applied: ["close", "comment"] } }
        : this.state.outcomes;
    this.set({
      pending: null,
      status: this.status({ pending: null }),
      outcomes,
      audit: [row, ...(this.state.audit ?? [])],
      auditTotal: this.state.auditTotal + 1,
    });
  }

  private clearExpiry() {
    if (this.expiry) clearTimeout(this.expiry);
    this.expiry = null;
  }
}

export function isMock(desk: Desk): desk is MockDesk {
  return desk instanceof MockDesk;
}

export type { Link };
