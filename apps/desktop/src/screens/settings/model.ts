/**
 * Owner: settings. What the Settings screen knows about each key: its plain name, where it
 * lives on the page, whether the daemon picks it up live, and how to check it. Pure data and
 * functions; no React.
 */
import type { DevicesView, SettingsView } from "@ghost/client-ts";

/**
 * Sections, in page order. The id is also the `?section=` deep link, which lands on the
 * section's page. `apps` and `skills` were sections of the one long page; they are pages of
 * their own now, and their old links land there.
 */
export type SectionId = "setup" | "about" | "secrets" | "model" | "apps" | "skills" | "voice" | "audio" | "models" | "phone" | "privacy" | "sounds";

export const SECTION_TITLE: Record<SectionId, string> = {
  setup: "Finish setup",
  about: "About Ghost",
  secrets: "Passwords",
  model: "Model",
  apps: "Apps",
  skills: "Skills",
  voice: "Voice and wake",
  audio: "Microphone and speakers",
  models: "Voice models",
  phone: "Phone",
  privacy: "Privacy",
  sounds: "Sounds",
};

/** The pages this screen draws (SETTINGS_SCREEN_PAGES in app/routes.tsx). */
export type PageId = "general" | "model" | "passwords" | "sound" | "phone";

/** The settings page each section is on (#/settings/<page>). */
export const SECTION_PAGE: Record<SectionId, string> = {
  setup: "general",
  about: "general",
  privacy: "general",
  model: "model",
  secrets: "passwords",
  apps: "apps",
  skills: "skills",
  voice: "sound",
  audio: "sound",
  models: "sound",
  sounds: "sound",
  phone: "phone",
};
export const isSectionId = (v: string | null): v is SectionId => !!v && v in SECTION_PAGE;

/** Ghost's voice, edited on Sound with the rest: it lives on Ghost's agent, not in settings.env. */
export const GHOST_VOICE = "ghost:voice";

export const sectionDomId = (id: SectionId) => `set-${id}`;

/** Every non-secret key the screen edits. Order matters only for the dirty summary. */
export const VALUE_KEYS = [
  "ANTHROPIC_BASE_URL",
  "GHOST_LLM_MODEL",
  "GHOST_INPUT_DEVICE",
  "GHOST_WAKE_PHRASES",
  "GHOST_WAKE_FUZZY",
  "GHOST_JEV",
] as const;
export type ValueKey = (typeof VALUE_KEYS)[number];

export const KEY_LABEL: Record<string, string> = {
  ANTHROPIC_BASE_URL: "Model endpoint",
  GHOST_LLM_MODEL: "Model",
  GHOST_INPUT_DEVICE: "Microphone",
  GHOST_WAKE_PHRASES: "Wake phrases",
  GHOST_WAKE_FUZZY: "Wake sensitivity",
  GHOST_JEV: "How Ghost hears your yes",
  [GHOST_VOICE]: "Ghost’s voice",
  ANTHROPIC_AUTH_TOKEN: "Anthropic token",
  GITHUB_TOKEN: "GitHub token",
  HF_TOKEN: "Hugging Face token",
  OPENROUTER_API_KEY: "OpenRouter key",
  TYPESAFE_API_KEY: "TypeSafe key",
  ELEVENLABS_API_KEY: "ElevenLabs key",
};

export const KEY_SECTION: Record<string, SectionId> = {
  ANTHROPIC_BASE_URL: "model",
  GHOST_LLM_MODEL: "model",
  GHOST_INPUT_DEVICE: "audio",
  GHOST_WAKE_PHRASES: "voice",
  GHOST_WAKE_FUZZY: "voice",
  GHOST_JEV: "voice",
  [GHOST_VOICE]: "voice",
};

/** Keys the daemon applies without a restart (ghostd `apply_settings`). Everything else waits. */
export const LIVE_KEYS = new Set(["GHOST_WAKE_PHRASES", "GHOST_WAKE_FUZZY"]);

/** Secrets, in the order the page shows them: what Ghost needs first, then the optional ones. */
export const SECRET_ORDER = ["ANTHROPIC_AUTH_TOKEN", "GITHUB_TOKEN", "TYPESAFE_API_KEY", "OPENROUTER_API_KEY", "HF_TOKEN", "ELEVENLABS_API_KEY"];
export const isOptionalSecret = (name: string) => !!SECRET_INFO[name]?.optional;

export const SECRET_INFO: Record<string, { help: string; optional?: boolean; placeholder: string }> = {
  ANTHROPIC_AUTH_TOKEN: { help: "Lets Ghost think and talk. Not needed if your own endpoint takes no key.", placeholder: "sk-ant-…" },
  GITHUB_TOKEN: { help: "For a GitHub app you added by hand whose config names ${GITHUB_TOKEN}. Apps you connect under Apps keep their own key.", optional: true, placeholder: "github_pat_…" },
  TYPESAFE_API_KEY: { help: "Lets Jev hear your yes. Used first when both Jev keys are set.", optional: true, placeholder: "ts_…" },
  OPENROUTER_API_KEY: { help: "Reaches Jev, and the model too when the endpoint setting is openrouter.", optional: true, placeholder: "sk-or-…" },
  HF_TOKEN: { help: "Speeds up the first download of the voice models.", optional: true, placeholder: "hf_…" },
  ELEVENLABS_API_KEY: { help: "Ghost speaks with an ElevenLabs voice when the voice engine is elevenlabs. Only what Ghost says is sent.", optional: true, placeholder: "sk_…" },
};

/** A key Ghost keeps for an app it connected (ADR 0034): GHOST_APP_<APP>_<KEY>. */
export const isAppSecret = (name: string) => name.startsWith("GHOST_APP_");

/** "GHOST_APP_MOBBIN_KEY" → "Mobbin key": an app's key in words; the name stays as detail. */
export function secretLabel(name: string): string {
  if (KEY_LABEL[name]) return KEY_LABEL[name];
  if (!isAppSecret(name)) return name;
  const words = name.slice("GHOST_APP_".length).toLowerCase().split("_").filter(Boolean);
  if (!words.length) return name;
  return words[0][0].toUpperCase() + words[0].slice(1) + (words.length > 1 ? ` ${words.slice(1).join(" ")}` : "");
}

export const APP_SECRET_INFO = { help: "Set when you connected the app. Replace it if the app asks for a new one.", placeholder: "Paste the new key" };

export const DEFAULT_MODEL = "claude-sonnet-5-5";
export const DEFAULT_FUZZY = 0.9;

export const truthy = (v: string | undefined) => ["1", "true", "yes", "on"].includes((v ?? "").trim().toLowerCase());

/** "a, b,c" -> ["a", "b", "c"]: trimmed, no empties, no duplicates (case-insensitive). */
export function splitList(v: string | undefined): string[] {
  const out: string[] = [];
  for (const raw of (v ?? "").split(",")) {
    const t = raw.trim();
    if (t && !out.some((o) => o.toLowerCase() === t.toLowerCase())) out.push(t);
  }
  return out;
}
export const joinList = (items: string[]) => items.join(",");

/**
 * GHOST_SKILL_ROOTS is paths separated by commas, `~` allowed. The screen edits it one path
 * per line: lines in, the comma list out (trimmed, no empties, no duplicates).
 */
export const splitLines = (text: string): string[] => splitList(text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean).join(","));
/** A root that isn't a full path can't be found: "~/projects/heka" or "/srv/heka", never "heka". */
export const badRoot = (root: string) => !/^(~|\/)/.test(root);

export function parseFuzzy(v: string | undefined): number {
  const n = Number.parseFloat(v ?? "");
  return Number.isFinite(n) ? Math.min(1, Math.max(0, n)) : DEFAULT_FUZZY;
}

/** The wake threshold in the operator's words. Higher thresholds need a closer match. */
export function fuzzyWords(t: number): string {
  if (t <= 0) return "Exact words only";
  if (t >= 0.9) return "Strict";
  if (t >= 0.75) return "Balanced";
  return "Forgiving";
}

export const JEV_OPTIONS = [
  { value: "auto", label: "Automatic" },
  { value: "on", label: "Always Jev" },
  { value: "off", label: "Word list only" },
] as const;

export type Values = Record<string, string>;

export interface Draft {
  values: Values;
  /** Secret name -> the new value typed this session. Never read back from the daemon. */
  secrets: Record<string, string>;
}

const isSet = (s: SettingsView | null, name: string) => !!s?.secrets.find((x) => x.name === name)?.set;

export const JEV_NEEDS_KEY = "Jev needs a TypeSafe or OpenRouter key. Add one under Passwords, or choose Automatic.";

/** Which Jev path is live (or would be after a restart) for these values and secrets. */
export function jevNow(mode: string, s: SettingsView | null, draftSecrets: Record<string, string>): { label: string; ok: boolean } {
  const has = (k: string) => isSet(s, k) || !!draftSecrets[k];
  const key = has("TYPESAFE_API_KEY") ? "TypeSafe" : has("OPENROUTER_API_KEY") ? "OpenRouter" : null;
  const m = mode || "auto";
  if (m === "off") return { label: "The word list hears your yes.", ok: true };
  if (key) return { label: `Jev hears your yes, through ${key}.`, ok: true };
  if (m === "on") return { label: JEV_NEEDS_KEY, ok: false };
  return { label: "No Jev key yet, so the word list hears your yes.", ok: true };
}

/** Field errors for the current draft, by key. Empty when it can be saved. */
export function validate(d: Draft, s: SettingsView | null): Record<string, string> {
  const e: Record<string, string> = {};
  const url = (d.values.ANTHROPIC_BASE_URL ?? "").trim();
  if (url && !/^https?:\/\/\S+$/.test(url)) e.ANTHROPIC_BASE_URL = "Use a full URL, like https://api.anthropic.com or http://localhost:8080.";
  if ((d.values.GHOST_JEV || "auto") === "on" && !jevNow("on", s, d.secrets).ok) e.GHOST_JEV = jevNow("on", s, d.secrets).label;
  if (d.values.GHOST_WAKE_PHRASES != null && d.values.GHOST_WAKE_PHRASES !== "" && splitList(d.values.GHOST_WAKE_PHRASES).length === 0)
    e.GHOST_WAKE_PHRASES = "Add at least one phrase.";
  return e;
}

/** Whether a draft value differs from the stored one. Sensitivity compares as a number ("0.8" is "0.80"). */
export function valueChanged(k: string, stored: string | undefined, draft: string | undefined): boolean {
  const a = stored ?? "";
  const b = draft ?? "";
  if (a === b) return false;
  if (k === "GHOST_WAKE_FUZZY" && a.trim() !== "" && b.trim() !== "") return parseFuzzy(a) !== parseFuzzy(b);
  return true;
}

/** Keys whose draft differs from what the daemon reported. */
export function changedKeys(d: Draft, s: SettingsView | null): string[] {
  const out: string[] = [];
  for (const k of VALUE_KEYS) if (valueChanged(k, s?.values[k], d.values[k])) out.push(k);
  for (const [k, v] of Object.entries(d.secrets)) if (v) out.push(k);
  return out;
}

const PROPER = new Set(["Anthropic", "Hugging", "Jev", "Ghost’s"]);

/** "Wake phrases, model and GitHub token" */
export function listWords(keys: string[]): string {
  const names = keys.map((k, i) => {
    const l = KEY_LABEL[k] ?? k;
    // Names keep their capitals (GitHub token, Anthropic token, TypeSafe key); common words don't.
    const first = l.split(" ")[0];
    const proper = /[A-Z]/.test(first.slice(1)) || PROPER.has(first);
    return i === 0 || proper ? l : l.charAt(0).toLowerCase() + l.slice(1);
  });
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** Lowercase the first letter unless the first word is a name (GitHub, Anthropic, Hugging Face, Ghost’s). */
export function lowerFirst(s: string) {
  const first = s.split(" ")[0];
  return /[A-Z]/.test(first.slice(1)) || PROPER.has(first) ? s : s.charAt(0).toLowerCase() + s.slice(1);
}

/** Ghost can answer: an Anthropic token or a model endpoint. Same test as the shell's banner. */
export const modelReady = (s: SettingsView | null, v: Values) => isSet(s, "ANTHROPIC_AUTH_TOKEN") || !!(v.ANTHROPIC_BASE_URL ?? "").trim();
export const echoReady = (d: DevicesView | null) => !!d?.echo_cancel_active;
export { isSet };

/** Input devices the operator can pick, plus ghost.mic once echo cancellation exists. */
export function micOptions(d: DevicesView | null, current: string): { value: string; label: string }[] {
  const opts: { value: string; label: string }[] = [{ value: "", label: "System default" }];
  if (d?.echo_cancel_source_present) opts.push({ value: "ghost.mic", label: "ghost.mic · cancels Ghost’s own voice" });
  for (const dev of d?.devices ?? []) {
    if (dev.max_input_channels <= 0 || dev.name === "ghost.mic" || dev.name === "default") continue;
    opts.push({ value: dev.name, label: dev.name });
  }
  if (current && !opts.some((o) => o.value === current)) opts.push({ value: current, label: `${current} · not found` });
  return opts;
}

/** The microphone Ghost is listening on now, in words. The daemon reports null for the system default. */
export const micInUse = (d: DevicesView | null) => (d?.input_device ? String(d.input_device) : "System default");

/** True when the saved microphone isn't the one Ghost opened at its last start. */
export const micDiffers = (d: DevicesView | null, saved: string | undefined) => !!d && (d.input_device ? String(d.input_device) : "") !== (saved ?? "").trim();

/** A path as the design system writes it: the operator's home collapsed to ~. The daemon reports absolute paths. */
export function tildePath(path: string | null | undefined): string {
  if (!path) return "";
  return path.replace(/^(?:\/home\/[^/]+|\/Users\/[^/]+|\/root)(?=\/|$)/, "~");
}

/* ------------------------------------------------- draft kept across navigation */
/** Non-secret edits survive leaving the page for this window's life. Secrets are never stored. */
const DRAFT_KEY = `ghost.settings.draft${new URLSearchParams(window.location.search).get("mock") ? `.mock-${new URLSearchParams(window.location.search).get("mock")}` : ""}`;

export function storeDraft(values: Record<string, string> | null) {
  try {
    if (values && Object.keys(values).length) sessionStorage.setItem(DRAFT_KEY, JSON.stringify(values));
    else sessionStorage.removeItem(DRAFT_KEY);
  } catch {
    /* storage off: the draft simply isn't kept */
  }
}

/**
 * The restart pill for a saved microphone Ghost hasn't opened yet, once dismissed, stays
 * dismissed for that device for this window's life. A different saved device brings it back.
 */
const MIC_DISMISSED_KEY = `${DRAFT_KEY}.mic-restart-dismissed`;

export function storeMicDismissed(saved: string) {
  try {
    sessionStorage.setItem(MIC_DISMISSED_KEY, saved);
  } catch {
    /* storage off: the pill simply returns next visit */
  }
}

export function micDismissedFor(saved: string | undefined): boolean {
  try {
    return sessionStorage.getItem(MIC_DISMISSED_KEY) === (saved ?? "").trim();
  } catch {
    return false;
  }
}

export function readDraft(): Record<string, string> | null {
  try {
    const raw = sessionStorage.getItem(DRAFT_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as unknown;
    if (!v || typeof v !== "object") return null;
    const out: Record<string, string> = {};
    for (const k of VALUE_KEYS) {
      const x = (v as Record<string, unknown>)[k];
      if (typeof x === "string") out[k] = x;
    }
    return out;
  } catch {
    return null;
  }
}
