/**
 * Owner: onboarding. Wizard state that outlives a step: which step is open (in the URL, so
 * every step has an address), what was skipped and whether Ghost has heard the wake phrase
 * (in localStorage, so setup resumes where the operator stopped), and the simulated model
 * download (module-level, so it keeps going while the operator moves between steps).
 */
import { useSyncExternalStore } from "react";

import type { OnboardingModel, OnboardingStepId } from "../../design/mock/fixtures/onboarding";

export type StepId = OnboardingStepId;

export interface StepDef {
  id: StepId;
  /** The rail's label. For the six tasks it is also the step's title, word for word. */
  label: string;
  /** Counts toward “N of 6 done” (welcome and done don't). */
  task: boolean;
}

export const STEPS: StepDef[] = [
  { id: "welcome", label: "Welcome", task: false },
  { id: "model", label: "Connect a model", task: true },
  { id: "github", label: "Connect GitHub", task: true },
  { id: "microphone", label: "Set up the microphone", task: true },
  { id: "models", label: "Get the voice models", task: true },
  { id: "service", label: "Start Ghost", task: true },
  { id: "try", label: "Say “Hey Ghost”", task: true },
  { id: "done", label: "Done", task: false },
];

export const stepLabel = (id: StepId) => STEPS.find((s) => s.id === id)?.label ?? id;

export const isStep = (v: string | null | undefined): v is StepId => !!v && STEPS.some((s) => s.id === v);
export const stepIndex = (id: StepId) => STEPS.findIndex((s) => s.id === id);

/* ------------------------------------------------------------------- URL: ?step=&state= */

function params(): URLSearchParams {
  return new URLSearchParams(window.location.search);
}

/** `?step=github` before the hash. Read once at mount; the wizard owns it after that. */
export function stepFromUrl(): StepId | null {
  const s = params().get("step");
  return isStep(s) ? s : null;
}

/** `?state=downloading` etc.: a designed state of a mock-only step, for screenshots. */
export function demoState(): string | null {
  return params().get("state");
}

/** Keep the address in step with the wizard, without a history entry per step. */
export function writeStepToUrl(step: StepId | null) {
  const p = params();
  if (step) p.set("step", step);
  else {
    p.delete("step");
    p.delete("state");
  }
  const q = p.toString();
  const url = `${window.location.pathname}${q ? `?${q}` : ""}${window.location.hash}`;
  window.history.replaceState(window.history.state, "", url);
}

/* ------------------------------------------------------------------ resume (localStorage) */

/** How Ghost woke during setup. Only "voice" proves the microphone and the wake phrase work. */
export type Heard = "voice" | "click" | null;

export interface Saved {
  step: StepId | null;
  skipped: StepId[];
  heard: Heard;
  /** Settings were saved that only apply after Ghost restarts. */
  restartNeeded: boolean;
}

const EMPTY: Saved = { step: null, skipped: [], heard: null, restartNeeded: false };

export function storageKey(scenario: string | null): string {
  // In mock mode each scenario resumes on its own, so one scenario's clicks don't leak into another's screenshots.
  // v3: v2 could hold a “heard” saved from a wake that was already running when the step opened.
  return scenario ? `ghost.welcome.v3:${scenario}` : "ghost.welcome.v3";
}

export function loadSaved(key: string): Saved {
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return EMPTY;
    const v = JSON.parse(raw) as Partial<Saved>;
    return {
      step: isStep(v.step) ? v.step : null,
      skipped: Array.isArray(v.skipped) ? v.skipped.filter(isStep) : [],
      heard: v.heard === "voice" || v.heard === "click" ? v.heard : null,
      restartNeeded: !!v.restartNeeded,
    };
  } catch {
    return EMPTY;
  }
}

export function storeSaved(key: string, v: Saved) {
  try {
    window.localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* private mode: setup still works, it just won't resume */
  }
}

/* --------------------------------------------------------- simulated model download (mock) */

export type DownloadPhase = "idle" | "downloading" | "done" | "failed";

export interface DownloadState {
  phase: DownloadPhase;
  /** Megabytes received per model. */
  got: Record<string, number>;
  /** MB per second, for “about 9 s left”. */
  rate: number;
}

let dl: DownloadState = { phase: "idle", got: {}, rate: 0 };
let models: OnboardingModel[] = [];
let timer: ReturnType<typeof setInterval> | null = null;
const listeners = new Set<() => void>();

function emit(next: DownloadState) {
  dl = next;
  listeners.forEach((l) => l());
}

const TICK = 150;

function tick() {
  const got = { ...dl.got };
  let left = (dl.rate * TICK) / 1000;
  for (const m of models) {
    const have = got[m.id] ?? 0;
    if (have >= m.sizeMb) continue;
    const take = Math.min(left, m.sizeMb - have);
    got[m.id] = have + take;
    left -= take;
    if (left <= 0) break;
  }
  const finished = models.every((m) => (got[m.id] ?? 0) >= m.sizeMb);
  if (finished && timer) {
    clearInterval(timer);
    timer = null;
  }
  emit({ ...dl, got, phase: finished ? "done" : "downloading" });
}

export const download = {
  /** Set up once per page from the fixture. `initial` is a designed state from ?state=. */
  init(list: OnboardingModel[], rate: number, initial: string | null) {
    if (models.length) return;
    models = list;
    const total = (id: string) => list.find((m) => m.id === id)?.sizeMb ?? 0;
    if (initial === "downloading") {
      emit({ phase: "downloading", got: { kokoro: total("kokoro"), parakeet: Math.round(total("parakeet") * 0.34) }, rate });
      timer = setInterval(tick, TICK);
    } else if (initial === "paused") {
      emit({ phase: "idle", got: { kokoro: total("kokoro"), parakeet: Math.round(total("parakeet") * 0.39) }, rate });
    } else if (initial === "failed") {
      emit({ phase: "failed", got: { kokoro: total("kokoro"), parakeet: 412 }, rate });
    } else if (initial === "downloaded") {
      emit({ phase: "done", got: Object.fromEntries(list.map((m) => [m.id, m.sizeMb])), rate });
    } else {
      emit({ phase: "idle", got: {}, rate });
    }
  },
  start() {
    if (timer || dl.phase === "done") return;
    emit({ ...dl, phase: "downloading" });
    timer = setInterval(tick, TICK);
  },
  /** Stop and keep what arrived; Start picks up from there. */
  cancel() {
    if (timer) clearInterval(timer);
    timer = null;
    emit({ ...dl, phase: "idle" });
  },
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => listeners.delete(cb);
  },
  get: () => dl,
};

export function useDownload(): DownloadState {
  return useSyncExternalStore(download.subscribe, download.get, download.get);
}

/* --------------------------------------------------------------------------- formatting */

/** 340 MB, 1.26 GB. */
export function formatMb(mb: number): string {
  if (mb >= 1000) return `${(mb / 1000).toFixed(mb >= 10_000 ? 0 : 2).replace(/\.?0+$/, "")} GB`;
  return `${Math.round(mb)} MB`;
}

/** “about 9 s left”, “about 2 min left”. */
export function timeLeft(mb: number, rate: number): string {
  if (rate <= 0) return "";
  const s = Math.ceil(mb / rate);
  if (s < 60) return `about ${s} s left`;
  return `about ${Math.ceil(s / 60)} min left`;
}

/** /home/you/.cache/ghost → ~/.cache/ghost: paths are shown from ~, never a real home directory. */
export function tildify(path: string): string {
  return path.replace(/^\/home\/[^/]+(?=\/|$)/, "~").replace(/^\/root(?=\/|$)/, "~");
}

/** "hey ghost" → “Hey Ghost”. */
export function phraseCase(p: string): string {
  return p
    .trim()
    .split(/\s+/)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

export const splitList = (v: string | undefined) =>
  (v ?? "")
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
