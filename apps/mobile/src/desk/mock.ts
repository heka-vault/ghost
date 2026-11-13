/**
 * An in-memory desk for building the phone before the WebRTC transport exists. It behaves
 * like the real one at the level the screens see: the link comes up, Start opens a session,
 * Ghost asks before a confirm-level change, a spoken yes or no settles it, the audit log
 * fills in. The voice side has no mic yet, so the Mock panel drives it through `act`.
 */
import type { AuditEntry, OutcomeView, PendingProposal, QuestionView, Status, Turn, WaitingItemView } from "@ghost/client-ts";

import type { ConversationView } from "@ghost/client-ts";

import { MOCK_ADDRESS } from "../phone/pairing";
import { stateOf } from "../phone/store";
import type { Desk, DeskState, Link, Route } from "./types";
import { type LiveLink, NO_LINK } from "./waiting";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();
const id = () => Math.random().toString(16).slice(2) + Math.random().toString(16).slice(2);

const ASK_SECS = 45;

/** What Ghost asks in the mock, and what the gate writes when it is settled. */
const QUESTION = {
  source: "github_heka",
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
    source: "github_heka",
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
    entry("ghost-mock/app#199", "reopen", "On ghost-mock/app#199: reopened the issue.", "executed", 60 * 3 + 5, { conversation: "heka-workshop" }),
    entry("ghost-mock/app#212", "set_labels", "On ghost-mock/app#212: labels: add bug, priority-high.", "executed", 60 * 3 + 6, { conversation: "q3-audit" }),
    entry("ghost-mock/app#199", "close", "On ghost-mock/app#199: closed the issue.", "executed", 60 * 3 + 9),
    entry("ghost-mock/infra#64", "close", "On ghost-mock/infra#64: close the issue, with a comment: “Duplicate of #61.”", "expired", 60 * 26, {
      autonomy: "confirm",
      events: [{ at: ago(60 * 26), event: "expired" }],
    }),
  ];
}

export type MockAct = "ask" | "yes" | "no" | "talk" | "drop" | "deskDown" | "deskUp" | "deskSession" | "agentAsks";

const MOCK_LINES: ConversationView[] = [
  { id: "a0", slug: "ghost", name: "Ghost", aliases: [], job: "", voice: "af_heart", speed: 1, own_voice: false, created_at: "2026-10-01T09:00:00Z", last_active_at: "2026-10-20T13:00:00Z", retired_at: null, active: false, is_ghost: true, look: null },
  { id: "a1", slug: "heka-workshop", name: "Heka workshop", aliases: ["workshop"], job: "the workshop", voice: "am_adam", speed: 1, own_voice: true, created_at: "2026-10-18T09:00:00Z", last_active_at: "2026-10-20T11:00:00Z", retired_at: null, active: false, is_ghost: false, look: null },
  { id: "a2", slug: "quotient-research", name: "Quotient research", aliases: [], job: "", voice: "bf_emma", speed: 1, own_voice: true, created_at: "2026-10-19T09:00:00Z", last_active_at: "2026-10-19T18:00:00Z", retired_at: null, active: false, is_ghost: false, look: null },
  // Two more agents, each its own key on the dial pad (ADR 0035).
  { id: "a3", slug: "accounting", name: "Accounting", aliases: ["the books"], job: "the books", voice: "bm_george", speed: 1.05, own_voice: true, created_at: "2026-10-02T09:00:00Z", last_active_at: "2026-10-20T09:00:00Z", retired_at: null, active: false, is_ghost: false, look: null },
  { id: "a4", slug: "q3-audit", name: "Q3 audit", aliases: ["the audit"], job: "the auditors’ questions", voice: "af_heart", speed: 1.05, own_voice: false, created_at: "2026-10-14T09:00:00Z", last_active_at: "2026-10-19T15:00:00Z", retired_at: null, active: false, is_ghost: false, look: null },
];

export class MockDesk implements Desk {
  private state: DeskState;
  private listeners = new Set<() => void>();
  private expiry: ReturnType<typeof setTimeout> | null = null;
  private muted = false;

  constructor() {
    const address = stateOf().pairing?.addresses[0] ?? MOCK_ADDRESS;
    this.state = { link: "connecting", via: "Wi-Fi", address, lastCall: null, audio: { route: "speaker", available: ["speaker", "earpiece"] }, conversations: MOCK_LINES, status: null, pending: null, turns: [], audit: null, auditTotal: 0, outcomes: {}, waiting: null, questions: [], live: { ...NO_LINK, state: "connecting" } };
    const audit = seedAudit();
    const { items, questions } = seedWaiting();
    setTimeout(() => this.set({ link: "online", status: this.status({ waiting: items.length }), audit, auditTotal: audit.length, waiting: items, questions, live: this.live({ state: "connected", address, via: "Wi-Fi" }) }), 700);
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
      apps: ["github"],
      api_version: "1",
      conversation: "ghost",
      models_ready: true,
      pending: null,
      phase: "passive",
      session_id: null,
      session_origin: null,
      ending: false,
      waiting: 0,
      not_listening: null,
      uptime_secs: 0,
      version: "2026.10.0",
      build: { kind: "dev", commit: "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678", dirty: false },
      wake_mode: "transcript",
      ...this.state?.status,
      ...patch,
    };
  }

  private phase(phase: Status["phase"]) {
    this.set({ status: this.status({ phase }) });
  }

  private live(patch: Partial<LiveLink>): LiveLink {
    return { ...NO_LINK, ...this.state.live, enabled: true, heardAt: new Date().toISOString(), ...patch };
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

  async start(conversation?: string, _item?: string) {
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

  async acknowledge(key: string) {
    await sleep(150);
    this.set({ waiting: (this.state.waiting ?? []).map((i) => (i.key === key ? { ...i, state: "acknowledged", acknowledged_at: new Date().toISOString() } : i)) });
  }

  async setDeskListening(listening: boolean) {
    await sleep(150);
    this.set({ status: this.status({ not_listening: listening ? null : { until: null } }) });
  }

  async snooze(key: string, minutes: number) {
    await sleep(150);
    const until = new Date(Date.now() + minutes * 60000).toISOString();
    this.set({ waiting: (this.state.waiting ?? []).map((i) => (i.key === key ? { ...i, state: "snoozed", snoozed_until: until } : i)) });
  }

  /** The desk takes an answer and says so on the stream: the question settles, its item leaves the list. */
  private settleQuestion(key: string, state: QuestionView["state"], answer: string | null) {
    const at = new Date().toISOString();
    this.set({
      questions: this.state.questions.map((q) => (q.key === key ? { ...q, state, answer, choice: q.choices.find((c) => c.toLowerCase() === answer?.toLowerCase()) ?? null, answered_via: "phone", answered_at: at } : q)),
      waiting: (this.state.waiting ?? []).filter((i) => i.key !== key),
    });
  }

  async answer(key: string, text: string) {
    this.reachable();
    await sleep(250);
    const q = this.state.questions.find((x) => x.key === key);
    if (!q || q.state !== "open") throw new Error(`Already ${q?.state ?? "settled"}.`);
    if (q.choices_only && !q.choices.some((c) => c.toLowerCase() === text.trim().toLowerCase())) throw new Error(`Only ${q.choices.join(" or ")}.`);
    this.settleQuestion(key, "answered", text);
  }

  async decline(key: string) {
    this.reachable();
    await sleep(200);
    this.settleQuestion(key, "declined", null);
  }

  async confirmChange(key: string, _changeId: string) {
    this.reachable();
    await sleep(300);
    this.settleQuestion(key, "answered", "Yes");
  }

  async cancelChange(key: string, _changeId: string) {
    this.reachable();
    await sleep(200);
    this.settleQuestion(key, "answered", "No");
  }

  async answerRing(_ringId: string) {
    await sleep(200);
    return this.state.link === "online";
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
        return this.sayYesOrNo(true);
      case "no":
        return this.sayYesOrNo(false);
      case "talk":
        if (!this.inSession() || this.muted) return;
        this.say("user", "What else is on my plate today?");
        this.phase("thinking");
        await sleep(1400);
        return this.speak("Just the flaky runner on infra 77. Everything else is waiting on other people.");
      case "drop":
        this.set({ link: "reconnecting", live: { ...this.state.live, state: "reconnecting" } });
        await sleep(6000);
        return this.set({ link: "online", live: this.live({ state: "connected" }) });
      case "deskDown":
        return this.set({ link: "offline", live: { ...this.state.live, state: "offline" } });
      case "agentAsks": {
        const q = question("question:" + id().slice(0, 12), "Q3 audit", "agent:q3-audit", "Can I send the auditors the October bank statements?", ["Yes", "No", "Only the summary"], false, 0);
        const item: WaitingItemView = { key: q.key, source: q.source, title: `Q3 audit asks: ${q.question}`, state: "new", first_seen: q.asked_at, changed_at: q.asked_at, age_secs: 0, question: q };
        return this.set({ questions: [...this.state.questions, q], waiting: [...(this.state.waiting ?? []), item] });
      }
      case "deskSession": {
        // Someone woke Ghost at the desk; toggles.
        if (this.state.status?.session_origin === "desk") return this.set({ status: this.status({ phase: "passive", session_id: null, session_origin: null }) });
        if (this.inSession()) return;
        return this.set({ status: this.status({ phase: "listening", session_id: id(), session_origin: "desk" }) });
      }
      case "deskUp":
        this.set({ link: "connecting", live: { ...this.state.live, state: "connecting" } });
        await sleep(700);
        return this.set({ link: "online", live: this.live({ state: "connected" }) });
    }
  }

  private async ask() {
    if (!this.inSession() || this.state.pending) return;
    this.phase("thinking");
    await sleep(900);
    const p: PendingProposal = {
      id: id(),
      source: QUESTION.source,
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
  private async sayYesOrNo(yes: boolean) {
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
      source: p.source,
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

function question(key: string, askedBy: string, source: string, text: string, choices: string[], only: boolean, minutesAgo: number, change: string | null = null): QuestionView {
  const at = new Date(Date.now() - minutesAgo * 60000).toISOString();
  return {
    key,
    kind: change ? "change" : "question",
    in_run: false,
    asked_by: askedBy,
    source,
    question: text,
    detail: null,
    choices,
    choices_only: only,
    urgency: "notify",
    asked_at: at,
    expires_at: new Date(Date.now() + 86400000).toISOString(),
    state: "open",
    answer: null,
    choice: null,
    answered_via: null,
    answered_at: null,
    change_id: change,
  };
}

/** What the mock desk's waiting list starts with (ADR 0029, ADR 0051): an item, a question, a change. */
function seedWaiting(): { items: WaitingItemView[]; questions: QuestionView[] } {
  const ago = (m: number) => new Date(Date.now() - m * 60000).toISOString();
  const ask = question("question:0b00bdd56307", "Accounting", "agent:accounting", "Should the October invoice go out today?", ["Yes", "No"], false, 25);
  const change = question("change:5f0c9a", "Ghost", "gate", "On heka-vault/ghost#17: close the issue.", ["Yes", "No"], true, 40, "5f0c9a");
  const items: WaitingItemView[] = [
    { key: "ci:1", source: "watch:ci", title: "CI failed on main: test (ubuntu-latest)", state: "new", first_seen: ago(12), changed_at: ago(12), age_secs: 720, link: "https://github.com/heka-vault/ghost/actions" },
    { key: ask.key, source: ask.source, title: `Accounting asks: ${ask.question}`, state: "delivered", first_seen: ask.asked_at, changed_at: ask.asked_at, age_secs: 1500, delivered_via: "phone", delivered_at: ago(24), question: ask },
    { key: change.key, source: "gate", title: "Waiting for your yes: On heka-vault/ghost#17: close the issue.", state: "delivered", first_seen: ago(40), changed_at: ago(40), age_secs: 2400, delivered_via: "digest", delivered_at: ago(30), question: change },
  ];
  return { items, questions: [ask, change] };
}
