/**
 * The mock daemon's Developer live views (ADR 0073 as amended 2026-11-14), all synthetic:
 * the microphone meter's readings fifteen a second while the page holds it open, a few turns'
 * timings for the overlay, and Run Ghost's checks. No number here was measured.
 *
 * `?meter=` picks the room the meter plays:
 *   quiet   (default) a quiet room: the person now and then, taken; the room near -60 dBFS
 *   talker  a talker across the room, held as the room and let go; the person taken
 *   echo    Ghost speaking on the on-chip array (the room gate off): its echo dropped, a short
 *           word too short to interrupt, then a barge-in
 * Out of a session the meter shows the passive listener: levels, wake near misses and a hit.
 */
import type { ChecksView, GhostEvent, MeterDecisionView, MeterView, Status, TimingView } from "@ghost/client-ts";

export type MeterRoom = "quiet" | "talker" | "echo";

const LEASE_MS = 10_000;
const EVERY_MS = Math.round(1000 / 15);

function wobble(t: number, seed: number): number {
  return Math.sin(t * 0.0021 + seed) * 1.6 + Math.sin(t * 0.0137 + seed * 3) * 0.9;
}

function decision(kind: MeterDecisionView["kind"], extra: Partial<MeterDecisionView> = {}): MeterDecisionView {
  return { kind, ago_ms: 30, words: null, dbfs: null, person: null, secs: null, score: null, woke: null, ...extra };
}

/** The meter's reading at `ms` into the room's loop: pure, so a test can hold it. */
export function meterAt(room: MeterRoom, ms: number, session: boolean, listeningOff: boolean): MeterView {
  const gate = room === "echo" ? { on: false, why: "on_chip_array" as const } : { on: true, why: "this_microphone" as const };
  if (listeningOff) {
    return { open: true, state: "listening_off", level: null, peak: null, speaking: false, room_gate: { ...gate, on: gate.on, why: "no_session", room: null, person: null, line: null }, decisions: [] };
  }
  const loop = ms % 8000;
  const decisions: MeterDecisionView[] = [];
  const at = (x: number) => loop >= x && loop < x + EVERY_MS;
  let level = -61 + wobble(ms, 1);
  let speaking = false;
  if (!session) {
    if (loop > 2000 && loop < 2900) ((level = -31 + wobble(ms, 2)), (speaking = true));
    if (at(2950)) decisions.push(decision("near_miss", { score: 0.27 }));
    if (loop > 5600 && loop < 6500) ((level = -24 + wobble(ms, 3)), (speaking = true));
    if (at(6000)) decisions.push(decision("wake", { score: 0.81, woke: null }));
    if (at(6550)) decisions.push(decision("wake", { score: 0.81, woke: true }));
    return { open: true, state: "passive", level, peak: level + 6, speaking, room_gate: { ...gate, why: "no_session", room: null, person: null, line: null }, decisions };
  }
  if (room === "quiet") {
    if (loop > 1500 && loop < 3200) ((level = -24 + wobble(ms, 4)), (speaking = true));
    if (at(3300)) decisions.push(decision("taken", { words: 7, dbfs: -24.2 }));
  } else if (room === "talker") {
    level = -41 + wobble(ms, 5) * 2;
    speaking = loop % 2600 < 1700;
    if (at(1800)) decisions.push(decision("held_room", { dbfs: -40.6, person: -22.4 }));
    if (at(2700)) decisions.push(decision("let_go", { secs: 1.6, dbfs: -40.6 }));
    if (loop > 4400 && loop < 6100) ((level = -22 + wobble(ms, 6)), (speaking = true));
    if (at(6200)) decisions.push(decision("taken", { words: 5, dbfs: -22.1 }));
  } else {
    level = -36 + wobble(ms, 7) * 2.5;
    speaking = loop % 1800 < 500;
    if (at(600)) decisions.push(decision("echo"));
    if (at(2400)) decisions.push(decision("echo"));
    if (at(3900)) decisions.push(decision("too_short"));
    if (loop > 5800 && loop < 6800) ((level = -20 + wobble(ms, 8)), (speaking = true));
    if (at(6400)) decisions.push(decision("barge_in"));
    if (at(6900)) decisions.push(decision("taken", { words: 3, dbfs: -20.3 }));
  }
  const roomGate =
    room === "echo"
      ? { ...gate, room: null, person: null, line: null }
      : room === "talker"
        ? { ...gate, room: -38.2, person: -22.4, line: -35.2 }
        : { ...gate, room: -57.9, person: -24.1, line: -32.1 };
  return { open: true, state: "session", level, peak: level + 5, speaking, room_gate: roomGate, decisions };
}

/** A few turns as the daemon times them, for the overlay. */
export function seededTimings(session: string): TimingView[] {
  const at = (s: number) => new Date(Date.now() - s * 1000).toISOString();
  const base = { session_id: session, provider: "openrouter", calls: 1, tools: 0, ack: null, barge_in: null, handover: false };
  return [
    { ...base, at: at(95), turn: 1, kind: "voice", model: "claude-haiku-5-5", fast: true, closed: 0.58, stt: 0.16, first_word: 0.94, first_tool: null, tool_result: null, first_sound: 1.31, reply_end: 3.92 },
    { ...base, at: at(60), turn: 2, kind: "voice", model: "anthropic/claude-opus-5-5", fast: false, handover: true, calls: 3, tools: 1, closed: 0.61, stt: 0.18, first_word: 0.97, first_tool: 1.05, tool_result: 1.47, ack: 1.2, first_sound: 2.41, reply_end: 5.91 },
    { ...base, at: at(20), turn: 3, kind: "voice", model: "claude-haiku-5-5", fast: true, closed: 0.55, stt: 0.15, first_word: 0.88, first_tool: null, tool_result: null, first_sound: 1.22, reply_end: 2.6, barge_in: 0.42 },
  ];
}

export const MOCK_CHECKS: ChecksView = {
  ok: false,
  code: 1,
  problems: 1,
  secs: 3.4,
  lines: [
    { problem: true, warning: false, text: "app linear: not connected (sign in again: its token expired)" },
    { problem: false, warning: true, text: "passwords: warning: waits for the keyring: OPENROUTER_API_KEY" },
    { problem: false, warning: false, text: "version: 2026.11.1 (deb, release)" },
    { problem: false, warning: false, text: "llm: openrouter · anthropic/claude-opus-5-5 at https://openrouter.ai/api/v1" },
    { problem: false, warning: false, text: "wake: detector" },
    { problem: false, warning: false, text: "onnxruntime: /usr/lib/heka-ghost-desktop/libonnxruntime.so.1.24.4" },
    { problem: false, warning: false, text: "speech models: parakeet ok, kokoro ok" },
    { problem: false, warning: false, text: "microphone: a laptop's built-in microphone (the room gate is on)" },
    { problem: false, warning: false, text: "permissions: ~/.config/ghost/policy.toml" },
    { problem: false, warning: false, text: "apps: github connected (34 tools), linear not connected" },
    { problem: false, warning: false, text: "confirmation: regex" },
  ],
};

export class MockLive {
  private until = 0;
  private leaseMs = 0;
  private timer: number | null = null;
  private ticks = 0;

  constructor(
    private room: MeterRoom,
    private emit: (e: GhostEvent) => void,
    private status: () => Status,
  ) {}

  /** The session's turns so far, sent once as the window connects. */
  seed() {
    const s = this.status();
    if (!s.session_id) return;
    for (const t of seededTimings(s.session_id)) this.emit({ type: "timing", timing: t });
  }

  private tick = () => {
    if (this.ticks * EVERY_MS > this.leaseMs) {
      this.stop();
      return;
    }
    const s = this.status();
    // Its own count, not the clock: the same room plays the same way in a screenshot.
    this.ticks += 1;
    this.emit({ type: "meter", meter: meterAt(this.room, this.ticks * EVERY_MS, !!s.session_id, !!s.not_listening) });
  };

  private stop() {
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
    this.until = 0;
    this.emit({ type: "meter", meter: { open: false, state: "quiet", level: null, peak: null, speaking: false, room_gate: { on: false, why: "no_session", room: null, person: null, line: null }, decisions: [] } });
  }

  handle(method: string, path: string, b: Record<string, unknown>): unknown {
    if (method === "PUT" && path === "/developer/meter") {
      if (!b.open) {
        if (this.timer !== null) this.stop();
        return { open: false, until: null, per_second: 15 };
      }
      this.until = Date.now() + LEASE_MS;
      this.leaseMs = this.ticks * EVERY_MS + LEASE_MS;
      if (this.timer === null) {
        this.ticks = 0;
        this.timer = window.setInterval(this.tick, EVERY_MS);
      }
      return { open: true, until: new Date(this.until).toISOString(), per_second: 15 };
    }
    if (method === "POST" && path === "/developer/checks") return new Promise((r) => window.setTimeout(() => r(MOCK_CHECKS), 900));
    return undefined;
  }
}
