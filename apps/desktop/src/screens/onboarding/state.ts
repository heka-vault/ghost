/**
 * Owner: onboarding. The first run's steps and what outlives a step: which step is open (in the
 * address, so every step has one), what was skipped, how Ghost woke and whether it answered (in
 * localStorage, so setup resumes where the operator stopped). Pure: `progressOf` decides what is
 * done from what Ghost itself reports, and is tested on its own.
 */

export type StepId = "welcome" | "model" | "hear" | "wake" | "voice" | "app" | "agent" | "ask" | "done";

export interface StepDef {
  id: StepId;
  /** The rail's label, and the step's title word for word. */
  label: string;
  /** Counts toward “N of 7 done” (welcome and done don't). */
  task: boolean;
  /** What the step needs from outside this computer: said on the rail, so nobody waits on a network they don't have. */
  network: "none" | "maybe" | "needs";
}

export const STEPS: StepDef[] = [
  { id: "welcome", label: "Welcome", task: false, network: "none" },
  { id: "model", label: "Connect a model", task: true, network: "maybe" },
  { id: "hear", label: "Let Ghost hear you", task: true, network: "none" },
  { id: "wake", label: "Say “Hey Ghost”", task: true, network: "none" },
  { id: "voice", label: "Choose its voice", task: true, network: "none" },
  { id: "app", label: "Connect an app", task: true, network: "needs" },
  { id: "agent", label: "Hire an agent", task: true, network: "none" },
  { id: "ask", label: "Ask it something", task: true, network: "maybe" },
  { id: "done", label: "Done", task: false, network: "none" },
];

export const TASKS = STEPS.filter((s) => s.task);
export const stepLabel = (id: StepId) => STEPS.find((s) => s.id === id)?.label ?? id;
export const isStep = (v: string | null | undefined): v is StepId => !!v && STEPS.some((s) => s.id === v);
export const stepIndex = (id: StepId) => STEPS.findIndex((s) => s.id === id);
/**
 * A task's number on the rail, 1 to 7, the same 7 the count says; null for the welcome and
 * done pages, which are not tasks and carry no number (2026-11-13: "0 of 7 done" sat above
 * nine numbered circles).
 */
export const taskNumber = (id: StepId): number | null => {
  const at = TASKS.findIndex((t) => t.id === id);
  return at < 0 ? null : at + 1;
};

/* --------------------------------------------------------------------------- what is done */

/** What Ghost reports, reduced to what the steps decide on. Null: not known (Ghost not answering). */
export interface Facts {
  /** GET /model's `ready`. */
  modelReady: boolean | null;
  /** The last model check reached the main model. */
  modelChecked: boolean | null;
  /** GET /setup/models' `ready`: the speech models are on this computer. */
  speechReady: boolean | null;
  /** Echo cancellation is running (ghost.mic exists and is in use). */
  echo: boolean | null;
  /** Apps connected and answering, the stand-in GitHub left out. */
  apps: number | null;
  /** Agents hired, Ghost and the retired left out. */
  agents: number | null;
  /** GHOST_VOICE_MODE is written in the settings. */
  voiceSet: boolean | null;
}

export type TaskState = "done" | "skipped" | "open" | "unknown";

export interface Progress {
  state: Record<StepId, TaskState>;
  done: number;
  total: number;
  /** The first task still open (skipped ones last), or "done". */
  next: StepId;
  /** Ghost can answer a spoken question: a model, ears and its name heard. */
  canTalk: boolean;
}

/**
 * One truth per fact: each task is done only when Ghost says so. A model is done when its
 * connection is ready (a check that reached it is shown, never required: a local model on a
 * slow box is still a model). Hearing is done when the speech models are here and either echo
 * cancellation runs or Ghost has woken on the operator's voice, which proves the microphone.
 * The wake only by voice, never a click. The voice once a mode is written or chosen here. An
 * app, an agent: any. The question once Ghost answered one asked while this step was open.
 */
export function progressOf(f: Facts, saved: Pick<Saved, "skipped" | "heard" | "answered" | "voiceChosen">): Progress {
  const yes = (v: boolean | null): boolean | null => v;
  const tasks: Record<Exclude<StepId, "welcome" | "done">, boolean | null> = {
    model: yes(f.modelReady),
    hear: f.speechReady === null ? null : f.speechReady && (!!f.echo || saved.heard === "voice"),
    wake: saved.heard === "voice" ? true : f.speechReady === null ? null : false,
    voice: saved.voiceChosen || f.voiceSet === true ? true : f.voiceSet === null ? null : false,
    app: f.apps === null ? null : f.apps > 0,
    agent: f.agents === null ? null : f.agents > 0,
    ask: saved.answered,
  };
  const state = { welcome: "done", done: "open" } as Record<StepId, TaskState>;
  for (const [id, v] of Object.entries(tasks) as [StepId, boolean | null][]) {
    state[id] = v === true ? "done" : v === null ? "unknown" : saved.skipped.includes(id) ? "skipped" : "open";
  }
  const done = TASKS.filter((t) => state[t.id] === "done").length;
  const open = TASKS.find((t) => state[t.id] === "open") ?? TASKS.find((t) => state[t.id] === "skipped");
  if (done === TASKS.length) state.done = "done";
  return { state, done, total: TASKS.length, next: open?.id ?? "done", canTalk: tasks.model === true && tasks.wake === true };
}

/* ------------------------------------------------------------------ resume (localStorage) */

/** How Ghost woke during setup. Only "voice" proves the microphone and the wake phrase work. */
export type Heard = "voice" | "click" | null;

export interface Saved {
  step: StepId | null;
  skipped: StepId[];
  heard: Heard;
  /** When it woke on the operator's voice, for “It woke on its name at 3:04 PM”. */
  heardAt: string | null;
  /** Ghost answered a question asked while the last step was open. */
  answered: boolean;
  /** The operator chose a voice mode here (local is a choice too, so it can't be read off the settings). */
  voiceChosen: boolean;
  /** The operator finished or closed setup: the shell stops sending them here. */
  finished: boolean;
}

export const EMPTY: Saved = { step: null, skipped: [], heard: null, heardAt: null, answered: false, voiceChosen: false, finished: false };

export function storageKey(scenario: string | null): string {
  // v4: the steps of the real first run (2026-11-12). In mock mode each scenario resumes on its own.
  return scenario ? `ghost.welcome.v4:${scenario}` : "ghost.welcome.v4";
}

export function parseSaved(raw: string | null): Saved {
  if (!raw) return EMPTY;
  try {
    const v = JSON.parse(raw) as Partial<Saved>;
    return {
      step: isStep(v.step) ? v.step : null,
      skipped: Array.isArray(v.skipped) ? v.skipped.filter(isStep) : [],
      heard: v.heard === "voice" || v.heard === "click" ? v.heard : null,
      heardAt: typeof v.heardAt === "string" ? v.heardAt : null,
      answered: v.answered === true,
      voiceChosen: v.voiceChosen === true,
      finished: v.finished === true,
    };
  } catch {
    return EMPTY;
  }
}

export function loadSaved(key: string): Saved {
  try {
    return parseSaved(window.localStorage.getItem(key));
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

/* ------------------------------------------------------------------------- the address */

function params(): URLSearchParams {
  return new URLSearchParams(window.location.search);
}

/** `?step=wake` before the hash. Read once at mount; the wizard owns it after that. */
export function stepFromUrl(): StepId | null {
  const s = params().get("step");
  return isStep(s) ? s : null;
}

/** `?state=heard` etc.: a designed state, for screenshots under the mock only. */
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

/* --------------------------------------------------------------------------- formatting */

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

/** The ladder of what to try when Ghost doesn't hear its name, most likely first, from what is true now. */
export function wakeHelp(f: { listeningOff: boolean; device: string | null | undefined; echo: boolean; fuzzy: number; phrases: string[] }): { id: string; text: string }[] {
  const out: { id: string; text: string }[] = [];
  if (f.listeningOff) out.push({ id: "listen", text: "Listening is off: the microphone is closed, so nothing reaches the wake detector. Turn listening on." });
  if (!f.echo || (f.device && f.device !== "ghost.mic")) out.push({ id: "mic", text: "Check the microphone: pick ghost.mic, or the one you talk into, and make sure it isn’t muted." });
  if (f.fuzzy >= 0.85) out.push({ id: "fuzzy", text: "Let it match more loosely: a strict match misses a phrase said quickly or with an accent." });
  if (f.phrases.length > 1) out.push({ id: "other", text: `Try ${f.phrases.slice(1).map((p) => `“${phraseCase(p)}”`).join(" or ")} instead.` });
  out.push({ id: "hearing", text: "Record yourself in the Hearing test: it plays back what Ghost hears and scores how well it reads it." });
  return out;
}

/** The maker's own words, first sentence only: data, shown as plain text. */
export function firstSentence(t: string): string {
  const s = t.trim();
  const m = s.match(/^.{20,160}?[.!?](\s|$)/);
  return (m ? m[0] : s.length > 160 ? `${s.slice(0, 157)}…` : s).trim();
}
