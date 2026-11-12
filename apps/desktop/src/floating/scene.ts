/**
 * What the floating room shows, as a pure function of what the daemon and the shell said
 * (ADR 0059, docs/design-system.md, The floating room). Plain TypeScript: the page feeds it,
 * the tests drive it.
 *
 * Nothing here reads the microphone: the operator's turn is a sign from the session's activity
 * (`you_started`), never a level and never words while they speak (#73, ADR 0055).
 */
import { appOf, type Activity } from "@ghost/ui";

import type { Posture } from "../app/posture";

export type Scene =
  /** Nothing at all, not even the line: the screen is being shared or recorded. */
  | "hidden"
  /** At rest: the line at the bottom edge (or the struck microphone). */
  | "line"
  /** On hold (ADR 0021): the room is talking; Ghost waits at the edge for its name. */
  | "hold"
  /** The operator is speaking. */
  | "you"
  | "listening"
  | "thinking"
  | "speaking"
  /** A change waits for the operator's yes. */
  | "okay"
  /** An agent, a run, an outside tool or an app asked the operator something (ADR 0051). */
  | "question"
  /** Ghost has decided to hang up; its goodbye may still be playing. */
  | "ending";

export type Capture = "yes" | "no" | "unknown";

export interface SceneInput {
  /** `postureOf()` with this surface's inputs: `float` is a session that comes forward here. */
  posture: Posture;
  capture: Capture | null;
  phase: string | null;
  ending: boolean;
  youTalking: boolean;
  pending: boolean;
  question: boolean;
}

export function sceneOf(i: SceneInput): Scene {
  if (i.capture === "yes") return "hidden";
  if (i.posture !== "float") return "line";
  if (i.ending) return "ending";
  if (i.phase === "hold") return "hold";
  if (i.pending) return "okay";
  if (i.question) return "question";
  if (i.youTalking) return "you";
  if (i.phase === "thinking") return "thinking";
  if (i.phase === "speaking") return "speaking";
  return "listening";
}

/** Scenes where Ghost has come forward over the desktop (the frame loop runs, Wisp is drawn). */
export function isForward(s: Scene): boolean {
  return s !== "hidden" && s !== "line" && s !== "hold";
}

/**
 * Frames a second the scene needs: the arrival's zoom wants 60; everything else reads at 30,
 * the voice included (syllables come four to eight a second). Every frame is the whole
 * monitor's surface copied through XWayland, so this is the cost (docs/floating-room.md).
 */
export function fpsOf(s: Scene, arriving: boolean): number {
  if (!isForward(s)) return 0;
  return arriving ? 60 : 30;
}

/** The scene's word, in the label over the type. */
export function labelOf(s: Scene, o: { agent: string; using: string | null; asker: string | null; micOff: boolean }): string {
  switch (s) {
    case "you":
      return "Listening to you";
    case "listening":
      return o.micOff ? "Mic off · type in Ghost" : "Listening";
    case "thinking":
      return o.using ? `Using ${o.using}` : "Working";
    case "speaking":
      return o.agent;
    case "okay":
      return "Waiting for your yes";
    case "question":
      return o.asker ? `${o.asker} asks` : "A question for you";
    case "ending":
      return "Goodbye";
    case "hold":
      return "On hold · say “Ghost” to pick up";
    default:
      return "";
  }
}

/** One line of the telemetry margin: a call of this session, true to the daemon's activity. */
export interface CallRow {
  id: string;
  /** Clock time it started, HH:MM:SS. */
  at: string;
  /** `github.issue_read`, or the tool's name when no connected app matches. */
  name: string;
  app: string | null;
  /** How long it took, or null while it runs. */
  ms: number | null;
}

export interface Telemetry {
  rows: CallRow[];
  calls: number;
  open: number;
  /** Apps reached this session, in the order first used. */
  apps: string[];
  handovers: number;
  cuts: number;
}

const two = (n: number) => String(n).padStart(2, "0");
const clock = (iso: string) => {
  const d = new Date(iso);
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
};

/** The session's work from its activity: the last `keep` calls, newest last, and the counts. */
export function telemetryOf(list: readonly Activity[], apps: readonly string[], keep = 8): Telemetry {
  const rows: CallRow[] = [];
  const byId = new Map<string, CallRow & { started: number }>();
  const reached: string[] = [];
  let handovers = 0;
  let cuts = 0;
  for (const a of list) {
    if (a.kind === "call" && a.id && a.name) {
      const app = appOf(a.name, apps);
      const row = { id: a.id, at: clock(a.at), name: app ? `${app}.${a.name.slice(app.length + 1)}` : a.name, app, ms: null, started: new Date(a.at).getTime() };
      byId.set(a.id, row);
      rows.push(row);
      if (app && !reached.includes(app)) reached.push(app);
    } else if (a.kind === "result" && a.id) {
      const r = byId.get(a.id);
      if (r && r.ms == null) r.ms = Math.max(0, new Date(a.at).getTime() - r.started);
    } else if (a.kind === "handover") handovers++;
    else if (a.kind === "cut") cuts++;
  }
  const clean = rows.map(({ id, at, name, app, ms }) => ({ id, at, name, app, ms }));
  return { rows: clean.slice(-keep), calls: rows.length, open: rows.filter((r) => r.ms == null).length, apps: reached, handovers, cuts };
}

/** `412 ms`, `1.4 s`, `12 s`: a call's time in the margin. */
export function msText(ms: number | null): string {
  if (ms == null) return "…";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.round(ms / 1000)} s`;
}

/** `0:42`, `12:05`: how long the session has run. */
export function elapsedText(fromIso: string | null, now: number): string | null {
  if (!fromIso) return null;
  const s = Math.max(0, Math.floor((now - new Date(fromIso).getTime()) / 1000));
  return `${Math.floor(s / 60)}:${two(s % 60)}`;
}

/** Where Ghost's stage sits across the monitor, from the shell's choice (`place.rs`, `Side`). */
export function stageX(side: "centre" | "left" | "right" | null | undefined): number {
  return side === "left" ? 0.3 : side === "right" ? 0.7 : 0.5;
}
