/**
 * Owner: onboarding. The first run's steps and what outlives a step: which step is open (in the
 * address, so every step has one), what was skipped, how Ghost woke and whether it answered (in
 * localStorage, so setup resumes where the operator stopped). Pure: `progressOf` decides what is
 * done from what Ghost itself reports, `routeAfter` where Continue and Skip go, and both are
 * tested on their own.
 *
 * Two parts (2026-11-14, the owner: “reduce the steps it takes to get to the first ‘hey ghost’
 * and them seeing Ghost in the floating room”): **Meet Ghost**, three beats and nothing else
 * before them (a model, its ears, “Hey Ghost” answered over the desktop), then **Make it yours**
 * (its voice, an app, an agent, a first real question), each optional. A beat that can finish
 * by itself is a glance, not a form: a healthy microphone skips its step.
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
  /** Meet Ghost (the three beats before the moment) or Make it yours (after it, optional). */
  part: "meet" | "yours" | null;
}

export const STEPS: StepDef[] = [
  { id: "welcome", label: "Welcome", task: false, network: "none", part: null },
  { id: "model", label: "Connect a model", task: true, network: "maybe", part: "meet" },
  { id: "hear", label: "Let Ghost hear you", task: true, network: "none", part: "meet" },
  { id: "wake", label: "Say “Hey Ghost”", task: true, network: "none", part: "meet" },
  { id: "voice", label: "Choose its voice", task: true, network: "none", part: "yours" },
  { id: "app", label: "Connect an app", task: true, network: "needs", part: "yours" },
  { id: "agent", label: "Hire an agent", task: true, network: "none", part: "yours" },
  { id: "ask", label: "Ask it something", task: true, network: "maybe", part: "yours" },
  { id: "done", label: "Done", task: false, network: "none", part: null },
];
/** The three beats before the moment. */
export const MEET = STEPS.filter((s) => s.part === "meet");
export const partOf = (id: StepId) => STEPS.find((s) => s.id === id)?.part ?? null;

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
  /**
   * The microphone needs nothing from the operator (ADR 0064's reading): an input Ghost chose
   * or was given that is there, hearing, and listening is on. Absent or null: not known.
   */
  micHealthy?: boolean | null;
  /** There is no microphone at all: the moment is typed. */
  micNone?: boolean | null;
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
  /** The first meeting happened: Ghost answered the first “Hey Ghost” (or typed first line). */
  met: boolean;
}

/**
 * One truth per fact: each task is done only when Ghost says so. A model is done when its
 * connection is ready (a check that reached it is shown, never required: a local model on a
 * slow box is still a model). Hearing is done when the speech models are here and either echo
 * cancellation runs or Ghost has woken on the operator's voice, which proves the microphone.
 * The wake only by voice, never a click. The voice once a mode is written or chosen here. An
 * app, an agent: any. The question once Ghost answered one asked while this step was open.
 */
export function progressOf(f: Facts, saved: Pick<Saved, "skipped" | "heard" | "answered" | "voiceChosen"> & Partial<Pick<Saved, "met">>): Progress {
  const yes = (v: boolean | null): boolean | null => v;
  const tasks: Record<Exclude<StepId, "welcome" | "done">, boolean | null> = {
    model: yes(f.modelReady),
    hear: f.speechReady === null ? null : f.speechReady && (!!f.echo || saved.heard === "voice" || f.micHealthy === true),
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
  return { state, done, total: TASKS.length, next: open?.id ?? "done", canTalk: tasks.model === true && tasks.wake === true, met: !!saved.met };
}

/* ------------------------------------------------------------------------- where next */

const after = (id: StepId): StepId => STEPS[Math.min(STEPS.length - 1, stepIndex(id) + 1)].id;

/**
 * Where Continue and Skip go from a step. Nothing is asked before the moment that can finish by
 * itself: a model already ready passes its step, and a healthy microphone (or none at all,
 * where the moment is typed) passes Let Ghost hear you. After a moment with no model the first
 * run goes back to the model step, and once a model is given, on to a first real question. A
 * person who went on to Make it yours while Ghost's ears downloaded comes back to the moment
 * as soon as they are here.
 */
export function routeAfter(from: StepId, f: Facts, s: Pick<Saved, "met" | "waitingForEars">): StepId {
  if (s.waitingForEars && !s.met && f.speechReady === true && partOf(from) === "yours") return "wake";
  switch (from) {
    case "welcome":
      return f.modelReady === true ? routeAfter("model", f, s) : "model";
    case "model":
      if (s.met) return "ask";
      return f.micHealthy === true || f.micNone === true ? "wake" : "hear";
    case "wake":
      return s.met && f.modelReady === false ? "model" : "voice";
    default:
      return after(from);
  }
}

/** Who a newcomer is, for the measured path: what their computer already offers Ghost. */
export type Newcomer = "found" | "key" | "nothing";

/**
 * What a step asks of a newcomer before it lets them on, as the screen has it: the clicks
 * (a select counts as one) and the typed fields. The path to the moment is measured from this
 * (test/onboarding.test.ts, and the PR's table), so a required action added before the wake
 * beat fails a test. The wake beat itself asks for nothing but the words.
 */
export function requiredAt(step: StepId, who: Newcomer): { clicks: number; fields: number } {
  switch (step) {
    case "welcome":
      return { clicks: 1, fields: 0 }; // Get started
    case "model":
      // found: the model it found, Save and continue; key: the provider, the key, Save and
      // continue; nothing: Skip, meet Ghost first.
      return who === "found" ? { clicks: 2, fields: 0 } : who === "key" ? { clicks: 2, fields: 1 } : { clicks: 1, fields: 0 };
    case "hear":
      return { clicks: 1, fields: 0 }; // Continue
    default:
      return { clicks: 0, fields: 0 };
  }
}

/** The steps from Welcome to the wake beat, and what they ask, for one newcomer. */
export function pathToMoment(f: Facts, who: Newcomer): { steps: StepId[]; clicks: number; fields: number } {
  const steps: StepId[] = [];
  let at: StepId = "welcome";
  let clicks = 0;
  let fields = 0;
  for (let i = 0; at !== "wake" && i < STEPS.length; i++) {
    steps.push(at);
    const r = requiredAt(at, who);
    clicks += r.clicks;
    fields += r.fields;
    at = routeAfter(at, f, { met: null, waitingForEars: false });
  }
  return { steps: [...steps, "wake"], clicks, fields };
}

/* ------------------------------------------------------------------------ the downloads */

/**
 * How long the speech models have left, in words, from two readings of the download: the
 * first run shows it while the person sets up the rest. Null until there is a rate.
 */
export function timeLeft(samples: readonly { at: number; bytes: number }[], total: number): string | null {
  if (samples.length < 2) return null;
  const a = samples[0];
  const b = samples[samples.length - 1];
  const secs = (b.at - a.at) / 1000;
  const rate = secs > 0 ? (b.bytes - a.bytes) / secs : 0;
  if (rate <= 0) return null;
  const left = Math.max(0, total - b.bytes) / rate;
  if (left < 60) return "under a minute left";
  const m = Math.round(left / 60);
  if (m < 60) return `about ${m} min left`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `about ${h} h ${r} min left` : `about ${h} h left`;
}

/**
 * The microphone needs nothing from the person (ADR 0064): a reading with no problem, no
 * fallback and a known case, and listening on. Unprotected (speakers with nothing taking
 * Ghost's voice out) still hears; the echo canceller is offered beside the moment, never
 * before it.
 */
export function micHealthyOf(m: { case: string; problem: string | null; fell_back: string | null } | null | undefined, listeningOff: boolean): boolean | null {
  if (!m) return null;
  return !listeningOff && !m.problem && !m.fell_back && m.case !== "unknown";
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
  /** The first meeting happened, and how it opened: the name said, or a typed first line. */
  met: "voice" | "typed" | null;
  /** When, for “It came forward at 3:04 PM”. */
  metAt: string | null;
  /** Where Ghost answered: over the desktop (the floating room) or in this window. */
  metWhere: "floating" | "window" | null;
  /** The first run turned the floating room on once (the shell keeps the same, `floating::offer`). */
  floatOffered: boolean;
  /** They went on to Make it yours while Ghost's ears downloaded: come back when they're here. */
  waitingForEars: boolean;
}

export const EMPTY: Saved = {
  step: null,
  skipped: [],
  heard: null,
  heardAt: null,
  answered: false,
  voiceChosen: false,
  finished: false,
  met: null,
  metAt: null,
  metWhere: null,
  floatOffered: false,
  waitingForEars: false,
};

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
      met: v.met === "voice" || v.met === "typed" ? v.met : null,
      metAt: typeof v.metAt === "string" ? v.metAt : null,
      metWhere: v.metWhere === "floating" || v.metWhere === "window" ? v.metWhere : null,
      floatOffered: v.floatOffered === true,
      waitingForEars: v.waitingForEars === true,
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

/**
 * A PipeWire input's node as a person reads it: `alsa_input.usb-Seeed_Studio_reSpeaker_XVF3800_4-Mic_Array_1149…-00.iec958-stereo#0`
 * is "Seeed Studio reSpeaker XVF3800 4-Mic Array, channel 1" (2026-11-13: the raw node ran six
 * lines on the hear step). `ghost.mic` and anything not shaped like an ALSA node stay as they are.
 */
export function inputName(node: string): string {
  const m = /^alsa_input\.(.+?)(?:#(\d+))?$/.exec(node.trim());
  if (!m) return node;
  // A sound card on the board (`pci-0000_00_1f.3`): the computer's own microphone.
  if (/^pci-[0-9a-f_.:]+$/i.test(m[1].split(".analog")[0].split(".iec")[0]) || /^pci-[0-9a-f]{4}_/i.test(m[1])) return "the built-in microphone";
  let name = m[1].replace(/^(usb|pci|platform)-/, "");
  name = name.split(".")[0]; // the profile: iec958-stereo, analog-stereo
  name = name.replace(/_[0-9A-Fa-f]{8,}(-\d+)?$/, "").replace(/-\d+$/, ""); // the serial
  name = name.replace(/_/g, " ").trim() || node;
  return m[2] !== undefined ? `${name}, channel ${Number(m[2]) + 1}` : name;
}
