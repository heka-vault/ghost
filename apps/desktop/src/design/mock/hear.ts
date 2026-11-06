/**
 * The mock daemon's Hearing test (ADR 0042, `ghostd::hear`): the recorder, held takes,
 * takes that count down and end on silence, timed silent takes, a made-up check per take,
 * the take list, sessions and score runs, and the `hear` event twenty times a second while
 * the recorder is open (or a run goes). Refusals use the daemon's own sentences.
 *
 * `?hear=recording|countdown|check|problem|scoring` puts the recorder in that state, for
 * screenshots: a held take going when it opens, a take across the room counting down, a
 * check with a cut-off waiting, the last take that couldn't be checked, a run being scored.
 */
import type { GhostEvent, HearCheck, HearPlan, HearPlanTake, HearRunInfo, HearScoring, HearSession, HearSessionSummary, HearState, HearView, HearWarning, HearWord, Phase, TakeEnding } from "@ghost/client-ts";

import { speechEnvelope } from "../../ui/VoiceWave";
import { HttpError } from "./errors";
import { type HearFixture, HEAR_PLAN, type MockHearRun, type MockHearSession, recordedTake, reportFor } from "./fixtures/hear";

export type HearDesign = "recording" | "countdown" | "check" | "problem" | "scoring" | null;

export interface HearHooks {
  emit(e: GhostEvent): void;
  /** The status phase: `recording` while the recorder holds the mic, back to passive after. */
  setPhase(p: Phase): void;
  /** Why the recorder can't open now (a session, the models), or null. */
  unavailable(): string | null;
}

interface Going {
  take: HearPlanTake;
  block: string;
  away: boolean;
  ending: TakeEnding;
  /** When the countdown ends and the take starts (ms). */
  startsAt: number;
  /** When recording began (ms), once it has. */
  recordingAt: number | null;
  /** A quiet take's last word (ms after recordingAt); a timed take's length. */
  endsAfter: number;
}

const POST_MS = 500;
const CHECK_MS = 650;
const WALK_SECS = 5;
const COUNTDOWN_SECS = 3;
const SILENT_COUNTDOWN_SECS = 2;

const WARNING_TEXT: Record<string, (w?: number) => string> = {
  too_quiet: (p) => `Too quiet (peak ${Math.round(p ?? -40)} dBFS): is the mic muted, or are you far from it?`,
  clipping: () => "Clipping: back off a little, or turn the mic gain down.",
  no_speech: () => "No speech heard.",
  speech_in_silence: () => "Speech in a silent take: say nothing in these.",
  started_early: () => "You were already speaking when the take began: the first word may be cut.",
  cut_off: () => "You were still speaking when the take ended: the last word may be cut.",
  missed_end: (n) => `Ghost didn’t hear the last ${n ?? 1} word${n === 1 ? "" : "s"}.`,
};
const REDO = new Set(["no_speech", "started_early", "cut_off", "clipping", "speech_in_silence"]);
const warning = (kind: string, n?: number): HearWarning => ({ kind, text: WARNING_TEXT[kind]?.(n) ?? kind, redo: REDO.has(kind) });

/** What a mock "hears" wrong: a name or two Parakeet really does miss. */
const MISHEARD: Record<string, string> = { mobbin: "mobin", heka: "hecka", quotient: "quotion", phonon: "phone on", triage: "tree age" };

const words = (text: string) =>
  text
    .toLowerCase()
    .replace(/\.\.\./g, " ")
    .replace(/[^a-z0-9' -]/g, " ")
    .replace(/-/g, " ")
    .split(/\s+/)
    .filter(Boolean);

/** A made-up check for a take, deterministic per take id: some names misheard, a cut-off on A04 and F03, made-up words on H02. */
export function fakeCheck(session: string, take: HearPlanTake, seconds: number, cutOff = false): HearCheck {
  const said = words(take.text ?? "");
  const silent = take.kind === "silence";
  const cut = cutOff || take.id === "A04" || take.id === "F03";
  const list: HearWord[] = said.map((w, i) => {
    if (cut && i === said.length - 1) return { op: "missed", said: w, heard: null };
    const m = MISHEARD[w];
    return m ? { op: "misheard", said: w, heard: m } : { op: "same", said: w, heard: w };
  });
  if (take.id === "B03") list.splice(4, 0, { op: "extra", said: null, heard: "uh" });
  if (silent && take.id === "H02") list.push({ op: "extra", said: null, heard: "thank" }, { op: "extra", said: null, heard: "you" });
  const warnings: HearWarning[] = [];
  if (cut) warnings.push(warning("cut_off"));
  if (silent && list.length) warnings.push(warning("speech_in_silence"));
  if (take.id === "B02") warnings.push(warning("too_quiet", -37));
  const matched = list.filter((w) => w.op === "same").length;
  const heard = list.filter((w) => w.heard).map((w) => w.heard).join(" ");
  const speaking = !silent || list.length > 0;
  return {
    session,
    take: take.id,
    seconds: Math.round(seconds * 10) / 10,
    rms: silent ? -52 : -24,
    peak: silent ? -40 : take.id === "B02" ? -37 : -10,
    speech_start: speaking ? (silent ? 2.1 : 0.5) : null,
    speech_end: speaking ? (cut ? Math.round(seconds * 10) / 10 : Math.round((seconds - 0.7) * 10) / 10) : null,
    heard,
    words: list,
    matched,
    said: said.length,
    warnings,
  };
}

export class MockHear {
  private plan: HearPlan;
  private edited = false;
  private sessions: MockHearSession[];
  private runs: MockHearRun[];
  private rawDevice: string | null;
  private open: { session: string } | null = null;
  private opening = false;
  private state: HearState = "closed";
  private going: Going | null = null;
  private stopAt: number | null = null;
  private lastCheck: HearCheck | null = null;
  private scoring: HearScoring | null = null;
  private playingUntil = 0;
  private problem: string | null = null;
  private ticker: ReturnType<typeof setInterval> | null = null;
  private waiters: ((c: HearCheck | Error) => void)[] = [];
  private design: HearDesign;
  private lastIdle = Date.now();

  constructor(
    f: HearFixture,
    private hooks: HearHooks,
    design: HearDesign = null,
  ) {
    this.plan = structuredClone(f.plan);
    this.sessions = structuredClone(f.sessions);
    this.runs = structuredClone(f.runs);
    this.rawDevice = f.rawDevice;
    this.design = design;
    if (design === "scoring" && this.sessions[0]) {
      this.beginRun(this.sessions[this.sessions.length - 1].name, "aec");
    }
  }

  // ------------------------------------------------------------------ the view and its event

  private view(): HearView {
    const v: HearView = {
      state: this.opening ? "opening" : this.state,
      session: this.open?.session ?? null,
      take: this.going?.take.id ?? null,
      ending: this.going?.ending ?? null,
      countdown: null,
      elapsed: 0,
      raw_device: this.open ? this.rawDevice : null,
      unavailable: this.open || this.opening ? null : this.hooks.unavailable(),
      scoring: this.scoring ? { ...this.scoring } : null,
      playing: Date.now() < this.playingUntil,
      checked: this.open ? (this.lastCheck?.take ?? null) : null,
      problem: this.problem,
    };
    const g = this.going;
    if (g && this.state === "countdown") v.countdown = Math.max(1, Math.ceil((g.startsAt - Date.now()) / 1000));
    if (g?.recordingAt != null) v.elapsed = Math.round(((this.stopAt ?? Date.now()) - g.recordingAt + 500) / 10) / 100;
    return v;
  }

  /** The mic's level now: room tone between takes, a voice while one is spoken. */
  private level(): { level: number; speaking: boolean } {
    const g = this.going;
    const noise = 0.05 + Math.random() * 0.03;
    if (!g || g.recordingAt == null || this.state !== "recording") return { level: this.open ? noise : 0, speaking: false };
    if (g.take.kind === "silence") return { level: noise + (g.take.id === "H02" && Math.random() < 0.3 ? 0.2 : 0), speaking: false };
    const t = (Date.now() - g.recordingAt) / 1000 - 0.4;
    const voiced = t > 0 && (g.ending === "hold" || t * 1000 < g.endsAfter);
    const lv = voiced ? 0.15 + speechEnvelope(t) * 0.75 : noise;
    return { level: Math.round(lv * 1000) / 1000, speaking: voiced && lv > 0.25 };
  }

  private publish() {
    const { level, speaking } = this.level();
    this.hooks.emit({ type: "hear", hear: this.view(), level, speaking });
  }

  private tick = () => {
    this.advance();
    this.publish();
    if (!this.open && !this.opening && !this.scoring && Date.now() >= this.playingUntil) this.stopTicker();
  };
  private startTicker() {
    if (!this.ticker) this.ticker = setInterval(this.tick, 50);
  }
  private stopTicker() {
    if (this.ticker) clearInterval(this.ticker);
    this.ticker = null;
  }

  /** The take's clock: a countdown that ends, a quiet take that ends on silence, a timed one that runs out. */
  private advance() {
    const g = this.going;
    const now = Date.now();
    if (!g) return;
    if (this.state === "countdown" && now >= g.startsAt) {
      this.state = "recording";
      g.recordingAt = now;
    }
    if (this.state === "recording" && g.ending !== "hold" && g.recordingAt != null) {
      const end = g.ending === "quiet" ? g.endsAfter + 2000 : g.endsAfter;
      if (now - g.recordingAt >= end) void this.finish();
    }
    if (this.open && this.state === "ready" && now - this.lastIdle > 10 * 60_000) void this.close();
  }

  private async finish(): Promise<HearCheck | Error> {
    const g = this.going;
    if (!g || !this.open || g.recordingAt == null) return new HttpError(409, "no take is going");
    const session = this.open.session;
    this.stopAt = Date.now();
    this.state = "stopping";
    this.publish();
    await sleep(g.ending === "hold" ? POST_MS : 0);
    if (this.going !== g) return new HttpError(409, "the recorder closed");
    this.state = "checking";
    this.publish();
    await sleep(CHECK_MS);
    if (this.going !== g) return new HttpError(409, "the recorder closed");
    const seconds = (this.stopAt - g.recordingAt) / 1000 + (g.ending === "hold" ? 1 : 0.5);
    const check = fakeCheck(session, g.take, seconds);
    this.lastCheck = check;
    this.save(session, g, check);
    this.going = null;
    this.stopAt = null;
    this.state = "ready";
    this.problem = null;
    this.lastIdle = Date.now();
    this.publish();
    for (const w of this.waiters.splice(0)) w(check);
    return check;
  }

  private save(name: string, g: Going, c: HearCheck) {
    let s = this.sessions.find((x) => x.name === name);
    if (!s) {
      s = { name, updated: null, takes: [] };
      this.sessions.unshift(s);
    }
    const t = recordedTake(this.plan, g.take.id, {
      seconds: c.seconds,
      peak: c.peak,
      speech_start: c.speech_start,
      speech_end: c.speech_end,
      ending: g.ending,
      warnings: c.warnings.map((w) => w.kind),
      raw: !!this.rawDevice,
      time: new Date().toISOString(),
    });
    s.takes = [...s.takes.filter((x) => x.id !== t.id), t];
    s.updated = t.time;
  }

  // ------------------------------------------------------------------ the recorder

  private planTake(id: string): { take: HearPlanTake; block: string; away: boolean } {
    for (const b of this.plan.blocks) {
      const t = b.takes.find((x) => x.id === id);
      if (t) return { take: t, block: b.block, away: !!b.away };
    }
    throw new HttpError(404, `no take ${id} in the take list`);
  }

  private newSessionName(): string {
    const base = new Date().toISOString().slice(0, 10);
    for (let n = 1; n < 1000; n++) {
      const name = n === 1 ? base : `${base}-${n}`;
      if (!this.sessions.some((s) => s.name === name)) return name;
    }
    return `${base}-x`;
  }

  private async openRecorder(session: string | null, fresh = false): Promise<HearView> {
    if (session && !this.sessions.some((s) => s.name === session)) throw new HttpError(404, `no session ${session}`);
    if (this.open && !session && fresh) {
      if (this.busy()) throw new HttpError(409, "a take is going");
      session = this.newSessionName();
      this.sessions.unshift({ name: session, updated: null, takes: [] });
    }
    if (this.open) {
      if (session && session !== this.open.session) {
        if (this.busy()) throw new HttpError(409, "a take is going");
        this.dropEmpty(this.open.session);
        this.open = { session };
      }
      this.publish();
      return this.view();
    }
    if (this.opening) throw new HttpError(409, "the recorder is opening");
    const why = this.hooks.unavailable();
    if (why) throw new HttpError(503, why);
    this.opening = true;
    this.startTicker();
    this.publish();
    await sleep(350);
    this.opening = false;
    const name = session ?? this.newSessionName();
    if (!this.sessions.some((s) => s.name === name)) this.sessions.unshift({ name, updated: null, takes: [] });
    this.open = { session: name };
    this.state = "ready";
    this.lastIdle = Date.now();
    this.hooks.setPhase("recording");
    this.startTicker();
    this.applyDesign(name);
    this.publish();
    return this.view();
  }

  /** ?hear=…: the state a screenshot wants, once, as the recorder opens. */
  private applyDesign(session: string) {
    const d = this.design;
    this.design = null;
    const s = this.sessions.find((x) => x.name === session)!;
    const todo = this.plan.blocks.flatMap((b) => b.takes).find((t) => !s.takes.some((x) => x.id === t.id));
    if (d === "recording" && todo) {
      const { take, block, away } = this.planTake(todo.id);
      this.going = { take, block, away, ending: "hold", startsAt: Date.now(), recordingAt: Date.now() - 2300, endsAfter: 0 };
      this.state = "recording";
    }
    if (d === "countdown") {
      const f = this.plan.blocks.find((b) => b.away)?.takes[0];
      if (f) {
        const { take, block } = this.planTake(f.id);
        this.going = { take, block, away: true, ending: "quiet", startsAt: Date.now() + 7400, recordingAt: null, endsAfter: 2400 };
        this.state = "countdown";
      }
    }
    if (d === "check") {
      const id = s.takes.some((t) => t.id === "A04") ? todo?.id : "A04";
      if (id) {
        const { take, block, away } = this.planTake(id);
        const g: Going = { take, block, away, ending: "hold", startsAt: 0, recordingAt: 0, endsAfter: 0 };
        const c = fakeCheck(session, take, 4.6, true);
        this.lastCheck = c;
        this.save(session, g, c);
      }
    }
    if (d === "problem") this.problem = "Take A03 could not be checked: the models are not loaded.";
  }

  private busy() {
    return this.state === "countdown" || this.state === "recording" || this.state === "stopping" || this.state === "checking";
  }

  private dropEmpty(name: string) {
    const s = this.sessions.find((x) => x.name === name);
    if (s && s.takes.length === 0) this.sessions = this.sessions.filter((x) => x !== s);
  }

  private async close(): Promise<HearView> {
    if (this.open) {
      for (const w of this.waiters.splice(0)) w(new HttpError(409, "the recorder closed"));
      this.dropEmpty(this.open.session);
    }
    this.open = null;
    this.going = null;
    this.stopAt = null;
    this.state = "closed";
    this.lastCheck = null;
    this.playingUntil = 0;
    this.hooks.setPhase("passive");
    this.publish();
    return this.view();
  }

  private requireOpen(): { session: string } {
    if (!this.open) throw new HttpError(409, "the recorder is not open");
    return this.open;
  }

  private start(id: string, ending: TakeEnding): HearView {
    this.requireOpen();
    if (this.scoring) throw new HttpError(409, "a score run is going: wait for it to finish");
    this.playingUntil = 0;
    const { take, block, away } = this.planTake(id);
    const silent = take.kind === "silence";
    if (silent && ending !== "timed") throw new HttpError(422, `take ${id} is silent: it is timed`);
    if (!silent && ending === "timed") throw new HttpError(422, `take ${id} has a line: hold the key, or let it end on silence`);
    if (this.busy()) throw new HttpError(409, "a take is going");
    const now = Date.now();
    const spoken = 1200 + words(take.text ?? "").length * 330;
    if (ending === "hold") {
      this.going = { take, block, away, ending, startsAt: now, recordingAt: now, endsAfter: 0 };
      this.state = "recording";
    } else {
      const secs = ending === "timed" ? SILENT_COUNTDOWN_SECS : COUNTDOWN_SECS + (away ? WALK_SECS : 0);
      this.going = { take, block, away, ending, startsAt: now + secs * 1000, recordingAt: null, endsAfter: ending === "timed" ? (take.seconds ?? 6) * 1000 : spoken };
      this.state = "countdown";
    }
    this.publish();
    return this.view();
  }

  private async stop(): Promise<HearCheck> {
    this.requireOpen();
    if (this.state === "countdown") throw new HttpError(409, "the take has not started: cancel it instead");
    if (this.state === "stopping" || this.state === "checking") {
      const r = await new Promise<HearCheck | Error>((res) => this.waiters.push(res));
      if (r instanceof Error) throw r;
      return r;
    }
    if (this.state !== "recording") throw new HttpError(409, "no take is going");
    const r = await this.finish();
    if (r instanceof Error) throw r;
    return r;
  }

  private cancel(): HearView {
    this.requireOpen();
    if (this.state === "checking") throw new HttpError(409, "the take is being checked");
    if (!this.busy()) throw new HttpError(409, "no take is going");
    for (const w of this.waiters.splice(0)) w(new HttpError(409, "the take was canceled"));
    this.going = null;
    this.stopAt = null;
    this.state = "ready";
    this.lastIdle = Date.now();
    this.publish();
    return this.view();
  }

  private skip(id: string): HearSession {
    const { session } = this.requireOpen();
    if (this.going?.take.id === id) throw new HttpError(409, "that take is going");
    this.planTake(id);
    let s = this.sessions.find((x) => x.name === session);
    if (!s) {
      s = { name: session, updated: null, takes: [] };
      this.sessions.unshift(s);
    }
    const t = recordedTake(this.plan, id, { state: "skipped", seconds: null, peak: null, speech_start: null, speech_end: null, ending: null, raw: false, time: new Date().toISOString() });
    s.takes = [...s.takes.filter((x) => x.id !== id), t];
    s.updated = t.time;
    return this.sessionView(session);
  }

  // ------------------------------------------------------------------ sessions, playback, runs

  private sessionView(name: string): HearSession {
    const s = this.sessions.find((x) => x.name === name);
    if (!s) throw new HttpError(404, `no session ${name}`);
    return { name: s.name, takes: structuredClone(s.takes) };
  }

  private summaries(): HearSessionSummary[] {
    return this.sessions
      .filter((s) => s.takes.length > 0 || s.name === this.open?.session)
      .map((s) => ({
        name: s.name,
        recorded: s.takes.filter((t) => t.state === "recorded").length,
        skipped: s.takes.filter((t) => t.state === "skipped").length,
        raw: s.takes.filter((t) => t.raw).length,
        flagged: s.takes.filter((t) => (t.warnings ?? []).some((w) => REDO.has(w))).length,
        seconds: s.takes.reduce((n, t) => n + (t.seconds ?? 0), 0),
        updated: s.updated,
      }))
      .sort((a, b) => (b.updated ?? "").localeCompare(a.updated ?? "") || b.name.localeCompare(a.name));
  }

  private play(session: string, id: string, channel: string) {
    if (this.busy()) throw new HttpError(409, "not while a take is going");
    const s = this.sessions.find((x) => x.name === session);
    const t = s?.takes.find((x) => x.id === id && x.state === "recorded");
    if (!t) throw new HttpError(404, `take ${id} has no ${channel} recording`);
    if (channel === "raw" && !t.raw) throw new HttpError(404, `take ${id} has no raw recording`);
    this.playingUntil = Date.now() + (t.seconds ?? 3) * 1000;
    this.startTicker();
    this.publish();
  }

  private beginRun(session: string, channel: "aec" | "raw"): HearRunInfo {
    const s = this.sessions.find((x) => x.name === session);
    if (!s || !s.takes.some((t) => t.state === "recorded")) throw new HttpError(404, `session ${session} has no takes`);
    if (this.scoring) throw new HttpError(409, "a run is going");
    const d = new Date();
    const pad = (n: number) => String(n).padStart(2, "0");
    let id = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
    while (this.runs.some((r) => r.info.id === id)) id += "b";
    const total = s.takes.filter((t) => t.state === "recorded").length;
    const info: HearRunInfo = { id, session, channel, created: `${id.slice(0, 10)} ${pad(d.getHours())}:${pad(d.getMinutes())}`, commit: "cc2808c (with changes)", takes: 0, wer: { stage: null, turn: null, file: null }, note: "" };
    this.scoring = { id, session, channel, done: 0, total };
    this.startTicker();
    const step = () => {
      if (!this.scoring || this.scoring.id !== id) return;
      this.scoring.done += 1;
      if (this.scoring.done >= total) {
        this.scoring = null;
        const r = Math.random() * 0.02;
        this.runs.unshift({ info: { ...info, takes: total, wer: { stage: 0.071 + r, turn: 0.048 + r / 2, file: 0.043 } } });
        this.publish();
        return;
      }
      setTimeout(step, 420);
    };
    setTimeout(step, 420);
    this.publish();
    return info;
  }

  private findRun(id: string): MockHearRun {
    const r = this.runs.find((x) => x.info.id === id);
    if (!r) throw new HttpError(404, `no run ${id}`);
    return r;
  }

  private savePlan(p: HearPlan) {
    if (!p || !Array.isArray(p.blocks)) throw new HttpError(422, "the take list has no blocks");
    if (p.blocks.length === 0) throw new HttpError(422, "the take list needs at least one block");
    const seenBlocks = new Set<string>();
    const seen = new Set<string>();
    for (const b of p.blocks) {
      if (!b.block?.trim()) throw new HttpError(422, "every block needs a name");
      if (seenBlocks.has(b.block)) throw new HttpError(422, `block ${b.block} is in the take list twice`);
      seenBlocks.add(b.block);
      if (!b.takes.length) throw new HttpError(422, `block ${b.block} has no takes`);
      for (const t of b.takes) {
        if (!/^[A-Za-z0-9_-]{1,16}$/.test(t.id)) throw new HttpError(422, `take id ${JSON.stringify(t.id)} is 1 to 16 letters, digits, - or _`);
        if (seen.has(t.id)) throw new HttpError(422, `take ${t.id} is in the plan twice`);
        seen.add(t.id);
        if (t.kind === "silence" && !(t.seconds && t.seconds > 0)) throw new HttpError(422, `take ${t.id} is silent: give it a length in seconds`);
        if (t.kind !== "silence" && !t.text?.trim()) throw new HttpError(422, `take ${t.id} has no line`);
        for (const term of t.terms ?? []) if (!t.text?.toLowerCase().includes(term.toLowerCase())) throw new HttpError(422, `take ${t.id}: the term ${JSON.stringify(term)} is not in its line`);
      }
    }
    this.plan = structuredClone(p);
    this.edited = true;
    return { plan: structuredClone(this.plan), edited: true };
  }

  // ------------------------------------------------------------------ routes

  /** The answer for a /hear route, or undefined when the path isn't one. */
  async handle(method: string, path: string, query: URLSearchParams, b: Record<string, unknown>): Promise<unknown> {
    if (!path.startsWith("/hear")) return undefined;
    const str = (v: unknown) => (typeof v === "string" ? v : "");
    const channel = (v: unknown) => {
      const c = typeof v === "string" && v ? v : "aec";
      if (c !== "aec" && c !== "raw") throw new HttpError(422, `channel is aec or raw, not ${JSON.stringify(c)}`);
      return c;
    };
    let r: RegExpExecArray | null;
    if (method === "GET" && path === "/hear") return this.view();
    if (method === "POST" && path === "/hear/open") return this.openRecorder(str(b.session) || null, b.new === true);
    if (method === "POST" && path === "/hear/close") return this.close();
    if (method === "POST" && path === "/hear/take/start") return this.start(str(b.take), (str(b.ending) || "hold") as TakeEnding);
    if (method === "POST" && path === "/hear/take/stop") return this.stop();
    if (method === "POST" && path === "/hear/take/cancel") return this.cancel();
    if (method === "POST" && path === "/hear/take/skip") return this.skip(str(b.take));
    if (method === "GET" && path === "/hear/check") {
      if (!this.open || !this.lastCheck) throw new HttpError(404, "no take has been checked");
      return this.lastCheck;
    }
    if (path === "/hear/plan") {
      if (method === "GET") return { plan: structuredClone(this.plan), edited: this.edited };
      if (method === "PUT") return this.savePlan(b as unknown as HearPlan);
      if (method === "DELETE") {
        this.plan = structuredClone(HEAR_PLAN);
        this.edited = false;
        return { plan: structuredClone(this.plan), edited: false };
      }
    }
    if (method === "GET" && path === "/hear/sessions") return this.summaries();
    if ((r = /^\/hear\/sessions\/([^/]+)$/.exec(path))) {
      const name = decodeURIComponent(r[1]);
      if (method === "GET") return this.sessionView(name);
      if (method === "DELETE") {
        if (this.open?.session === name) throw new HttpError(409, "the session is open in the recorder");
        if (this.scoring?.session === name) throw new HttpError(409, "the session is being scored");
        if (!this.sessions.some((s) => s.name === name)) throw new HttpError(404, `no session ${name}`);
        this.sessions = this.sessions.filter((s) => s.name !== name);
        return { ok: true };
      }
    }
    if (method === "POST" && path === "/hear/play") return this.play(str(b.session), str(b.take), channel(b.channel)), { ok: true };
    if (method === "POST" && path === "/hear/play/stop") {
      this.playingUntil = 0;
      this.publish();
      return { ok: true };
    }
    if (method === "GET" && path === "/hear/runs") return this.runs.map((x) => x.info);
    if (method === "POST" && path === "/hear/runs") {
      if (!this.open) throw new HttpError(409, "open the recorder first: a run holds the mic");
      if (this.busy()) throw new HttpError(409, "a take is going");
      return this.beginRun(str(b.session), channel(b.channel));
    }
    if ((r = /^\/hear\/runs\/([^/]+)(\/note)?$/.exec(path))) {
      const id = decodeURIComponent(r[1]);
      const run = this.findRun(id);
      if (method === "GET" && !r[2]) {
        const base = query.get("baseline");
        const baseline = base ? this.findRun(base).info : null;
        return { info: run.info, baseline, report: reportFor(run.info, baseline) };
      }
      if (method === "PUT" && r[2]) {
        const note = str(b.note);
        if (note.length > 500) throw new HttpError(422, "a note is 500 characters at most");
        run.info = { ...run.info, note };
        return run.info;
      }
      if (method === "DELETE" && !r[2]) {
        this.runs = this.runs.filter((x) => x !== run);
        return { ok: true };
      }
    }
    return undefined;
  }

  dispose() {
    this.stopTicker();
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
