/**
 * The site's hero mock (heyghost.app, docs/site.md): the `floating-site` turn and nothing else,
 * for `site/src/components/FloatDemo.tsx`. The desktop's `?mock=` keeps the whole mock daemon
 * (`./daemon.ts` and every fixture, about 250 KB minified); the float on the site needs one
 * scene, so it gets this, a few kilobytes, and a transport that answers only what the floating
 * room (`src/floating`) asks for: the status, the session's turns, the questions, a call's
 * result and the event stream. Anything else is a 404.
 *
 * It must not drift from the product's mock: `test/floating-site-mock.test.ts` plays both on
 * fake timers and holds them to the same events at the same times (status as the floating view
 * reads it, turns, activity, the yes, Ghost's voice level, the words being said) and the same
 * answers. Change the scene in `MockDaemon.playFloatingSite`, and this follows, or the test fails.
 */
import type { ActivityView, CallPreview, GhostEvent, PendingProposal, Status, Transport, Turn } from "@ghost/client-ts";
import { wordEnds } from "@ghost/ui";

import siteTurnJson from "./fixtures/floating-site-turn.json";

/** The site's turn with sound (site/audio/turn.json, rendered): the floating-site loop's clock. */
export interface SiteTurn {
  loop_ms: number;
  wake: number;
  think: number[];
  sleep: number;
  lines: { id: string; who: "you" | "ghost"; text: string; at: number; ms: number }[];
  voice: { window_ms: number; levels: number[] };
}
export const SITE_TURN = siteTurnJson as SiteTurn;

type Listener = (e: GhostEvent | Status) => void;

const now = () => new Date().toISOString();
const newId = () => Array.from({ length: 32 }, () => Math.floor(Math.random() * 16).toString(16)).join("");

/** The README hero's places, reached while Ghost thinks: tool, when it goes out and comes back (share of the wait). */
const SIX: readonly [string, number, number][] = [
  ["github_pull_requests_list", 0, 0.8],
  ["calendar_events_today", 0.06, 0.6],
  ["ghost_recall", 0.12, 0.5],
  ["ghost_ask_agent", 0.2, 1],
  ["ghost_docs_search", 0.28, 0.7],
  ["linear_issues_search", 0.34, 0.9],
];

/** A believable result for a call, as the product's mock answers `GET /calls/{id}` (its length is the light coming back). */
function resultFor(name: string): string {
  if (/issue_list|search/.test(name))
    return JSON.stringify({ issues: [{ number: 212, title: "Invoice PDFs round euro totals down a cent", labels: ["billing"] }, { number: 208, title: "Annual plan shows the monthly price on the receipt", labels: ["billing", "needs-review"] }, { number: 77, title: "Nightly ledger export failed twice", repo: "ghost-mock/infra" }] }, null, 2);
  if (/_(write|close|create|update|delete)$/.test(name)) return JSON.stringify({ ok: true, status: "success", summary: "closed the issue" }, null, 2);
  return JSON.stringify({ ok: true }, null, 2);
}

class NotHere extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
  }
}

/** One scene of the in-memory daemon: the `floating-site` turn, looping on the sound's clock. */
export class SiteDaemon {
  private listeners = new Set<Listener>();
  private timers = new Set<ReturnType<typeof setTimeout>>();
  private voiceTimer: ReturnType<typeof setInterval> | null = null;
  private bootedAt = Date.now();
  private turns: Turn[] = [];
  private activity: ActivityView[] = [];
  private pending: PendingProposal | null = null;
  private calls = new Map<string, CallPreview>();
  private replyNo = -1;
  private played = false;
  private loopAt: number | null = null;
  private status: Status = {
    phase: "passive",
    session_id: null,
    models_ready: true,
    uptime_secs: 0,
    version: "site",
    build: { kind: "release", commit: null, dirty: false },
    api_version: "1",
    wake_mode: "transcript",
    // The scenario's connected apps, as the product's mock lists them (the test holds this).
    apps: ["github", "mobbin"],
    pending: null,
    conversation: null,
    session_origin: null,
    session_typed: false,
    ending: false,
    waiting: 0,
    not_listening: null,
    debug_audio: null,
  };

  constructor(private readonly onLoop?: () => void) {}

  subscribe(l: Listener): () => void {
    this.listeners.add(l);
    queueMicrotask(() => {
      l(this.snapshot());
      for (const activity of this.activity) l({ type: "activity", activity });
    });
    this.startVoice();
    this.play();
    return () => this.listeners.delete(l);
  }

  /** The few routes the floating view reads. */
  handle(method: string, path: string): unknown {
    if (method === "GET" && path === "/status") return this.snapshot();
    if (method === "GET" && path === "/session/turns") return this.turns;
    if (method === "GET" && path === "/questions") return { questions: [] };
    const called = /^\/calls\/([^/]+)$/.exec(path);
    if (called && method === "GET") {
      const got = this.calls.get(decodeURIComponent(called[1]));
      if (!got) throw new NotHere(404, "that call's result is no longer kept");
      return got;
    }
    throw new NotHere(404, "not on the site");
  }

  dispose() {
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
    if (this.voiceTimer) clearInterval(this.voiceTimer);
    this.voiceTimer = null;
    this.loopAt = null;
    this.listeners.clear();
  }

  // ------------------------------------------------------------------ the scene
  private play() {
    if (this.played) return;
    this.played = true;
    const t = SITE_TURN;
    const line = (id: string) => {
      const l = t.lines.find((x) => x.id === id);
      if (!l) throw new Error(`floating-site-turn.json has no line "${id}"`);
      return l;
    };
    const [request, reply, ask, yes, done, bye] = ["request", "reply", "ask", "yes", "done", "bye"].map(line);
    let n = 0;
    const loop = () => {
      this.loopAt = Date.now();
      this.onLoop?.();
      const id = () => `site${n++}`;
      const at = (ms: number, f: () => void) => this.later(ms, f);
      const call = (ms: number, name: string, cid: string) => at(ms, () => this.act("call", { name, id: cid }));
      const back = (ms: number, name: string, cid: string) => at(ms, () => this.act("result", { name, id: cid }));
      at(t.wake, () => this.wake());
      at(request.at, () => this.act("you_started"));
      at(request.at + request.ms, () => {
        this.act("you_stopped");
        this.addTurn("user", "Get me ready for the launch review.", "voice");
        this.heard("Get me ready for the launch review.");
        this.pushStatus({ phase: "thinking" });
      });
      const from = request.at + request.ms + 200;
      const span = reply.at - 300 - from;
      for (const [name, out, home] of SIX) {
        const cid = id();
        call(from + Math.round(out * span), name, cid);
        back(from + Math.round(home * span), name, cid);
      }
      at(reply.at, () => this.reply(reply.text, reply.ms));
      const g = id();
      call(reply.at + reply.ms + 50, "github_issue_list", g);
      back(ask.at - 50, "github_issue_list", g);
      at(ask.at, () => {
        this.propose({
          source: "github",
          operation: "github_issue_write",
          target: "ghost-mock/app#151",
          summary: "Close app#151 as a duplicate of #148",
          rationale: "Same login bug; 148 has the fix in review.",
        });
        this.reply(ask.text, ask.ms);
      });
      at(yes.at, () => {
        if (this.pending) this.act("you_started");
      });
      at(yes.at + yes.ms, () => {
        if (!this.pending) return;
        this.act("you_stopped");
        this.addTurn("user", yes.text, "voice");
        this.heard(yes.text);
      });
      at(done.at, () => {
        if (this.pending) this.confirm(done.text, done.ms);
      });
      at(bye.at, () => {
        this.pushStatus({ ending: true });
        this.reply(bye.text, bye.ms);
      });
      at(t.loop_ms, loop);
    };
    loop();
  }

  /** Ghost's voice level, twenty a second while it speaks: the rendered clip's own, window by window. */
  private startVoice() {
    if (this.voiceTimer) return;
    let speaking = false;
    this.voiceTimer = setInterval(() => {
      const on = this.status.phase === "speaking";
      if (on) {
        const v = SITE_TURN.voice;
        const level = this.loopAt === null ? 0 : (v.levels[Math.floor((Date.now() - this.loopAt) / v.window_ms)] ?? 0);
        this.emit({ type: "voice", level });
      } else if (speaking) this.emit({ type: "voice", level: 0 });
      speaking = on;
    }, 50);
  }

  // ------------------------------------------------------------------ what the daemon does
  private emit(e: GhostEvent) {
    for (const l of this.listeners) l(e);
  }
  private later(ms: number, f: () => void) {
    const t = setTimeout(() => {
      this.timers.delete(t);
      f();
    }, ms);
    this.timers.add(t);
  }
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
  private wake() {
    if (this.status.session_id) return;
    this.turns = [];
    this.replyNo = -1;
    this.pushStatus({ session_id: newId(), phase: "listening", conversation: "ghost", session_origin: "desk", session_typed: false });
    this.addTurn("assistant", "Ghost here.", "system");
  }
  private addTurn(role: Turn["role"], text: string, via: Turn["via"]) {
    const t: Turn = { role, text, via, at: now(), session_id: this.status.session_id ?? "" };
    this.turns = [...this.turns, t];
    this.emit({ type: "turn", turn: t });
  }
  private act(kind: ActivityView["kind"], extra: Partial<ActivityView> = {}) {
    if (!this.status.session_id) return;
    const activity: ActivityView = { kind, session_id: this.status.session_id, at: now(), names: [], ...extra };
    this.activity = [...this.activity, activity];
    this.emit({ type: "activity", activity });
    if (kind === "result" && extra.id && extra.name && !this.calls.has(extra.id)) {
      const read = !/_(write|close|create|update|delete)$/.test(extra.name);
      this.calls.set(extra.id, { id: extra.id, session_id: activity.session_id, name: extra.name, read, at: activity.at, preview: resultFor(extra.name), dropped: 0 });
    }
  }
  private heard(text: string) {
    if (!this.status.session_id) return;
    this.emit({ type: "heard", heard: { session_id: this.status.session_id, at: now(), text, reopened: false } });
  }
  /** Which words of a reply are heard: one `saying` per clause, spread over the time it takes. */
  private stamp(text: string, ms: number) {
    const sid = this.status.session_id;
    if (!sid) return;
    const no = ++this.replyNo;
    let words = 0;
    let chars = 0;
    for (const clause of text.split(/(?<=[,.;:?!])\s+/)) {
      const from = words;
      words += wordEnds(clause).length;
      const to = words;
      const at = (chars / Math.max(1, text.length)) * ms;
      chars += clause.length;
      if (to > from)
        this.later(Math.round(at) + 60, () => {
          if (this.status.session_id === sid && this.status.phase === "speaking") this.emit({ type: "saying", saying: { session_id: sid, reply: no, from, to } });
        });
    }
  }
  private reply(text: string, ms: number) {
    this.addTurn("assistant", text, "system");
    this.pushStatus({ phase: "speaking" });
    this.stamp(text, ms);
    this.later(ms, () => {
      if (!this.status.session_id || this.status.phase !== "speaking") return;
      if (this.status.ending) return this.sleepNow();
      this.pushStatus({ phase: "listening" });
    });
  }
  private propose(p: Omit<PendingProposal, "id" | "expires_at" | "expires_in_secs">) {
    this.pending = { ...p, id: newId(), expires_at: new Date(Date.now() + 60_000).toISOString(), expires_in_secs: 60 };
    this.emit({ type: "pending", pending: this.pending });
    this.pushStatus();
  }
  private settle() {
    this.pending = null;
    this.emit({ type: "pending", pending: null });
    this.pushStatus();
  }
  private confirm(said: string, ms: number) {
    const p = this.pending;
    if (!p) return;
    this.settle();
    this.emit({ type: "outcome", outcome: { proposal_id: p.id, status: "success", summary: p.summary, target: p.target, applied: [p.operation], state_now: "closed", error: null, failed_step: null }, via: "ui" });
    this.reply(said, ms);
  }
  private sleepNow() {
    if (this.pending) this.settle();
    this.activity = [];
    this.pushStatus({ session_id: null, phase: "passive", conversation: null, session_origin: null, session_typed: false, ending: false });
    this.emit({ type: "conversation", active: null, changed: null });
  }
}

/** The site's float: its transport and a way to stop every timer. `onLoop` is called as each turn starts. */
export function createSiteMock(onLoop?: () => void): { transport: Transport; dispose: () => void } {
  const daemon = new SiteDaemon(onLoop);
  const json = (data: unknown, status = 200) => new Response(JSON.stringify(data ?? null), { status, headers: { "content-type": "application/json" } });
  const transport: Transport = {
    baseUrl: "http://mock.ghost",
    fetch: async (input, init) => {
      const req = input instanceof Request ? input : new Request(String(input), init);
      try {
        return json(daemon.handle(req.method.toUpperCase(), new URL(req.url).pathname));
      } catch (e) {
        return json({ detail: e instanceof Error ? e.message : String(e) }, e instanceof NotHere ? e.status : 500);
      }
    },
    events: { subscribe: (onEvent) => daemon.subscribe(onEvent) },
  };
  return { transport, dispose: () => daemon.dispose() };
}
