/**
 * Owner: settings. The words of Settings › Sound › Dictation (ADR 0077 as amended 2026-11-25),
 * pure so they are tested without a browser: the key in GNOME's words made short, what the tidy
 * does with the words, and a line for each state that needs one. Ghost, key, type; never the
 * protocol's or the desktop's plumbing names.
 */
import type { DictationKeyView } from "@ghost/client-ts";

export const HOLD_OR_TAP = "Hold the key while you speak, or tap it to start and tap again to stop.";
export const BINDING = "GNOME will ask to let Ghost use the key, then to type for you.";
export const TYPES_WHERE = "Ghost types what you say where your cursor is.";

/**
 * GNOME's words for the key, short: “Press F9” is “F9”. Anything that isn't that one plain
 * shape is shown as GNOME gave it.
 */
export function keyName(words: string | null | undefined): string | null {
  const w = words?.trim();
  if (!w) return null;
  const m = /^press\s+(\S(?:.*\S)?)$/i.exec(w);
  return m && !/\s(or|and)\s/i.test(m[1]) ? m[1] : w;
}

/**
 * "Tidy with" (ADR 0078 as amended): Off, the operator's own Claude Code with one of its models,
 * or OpenRouter with one of a few quick models or another they name. OpenRouter's ids were
 * checked against `https://openrouter.ai/api/v1/models` on 2026-11-25.
 */
export const OPENROUTER_MODELS: readonly { id: string; label: string }[] = [
  { id: "anthropic/claude-haiku-5.5", label: "Claude Haiku 5.5" },
  { id: "google/gemini-3.5-flash-lite", label: "Gemini 3.5 Flash Lite" },
  { id: "meta-llama/llama-3.1-8b-instruct:nitro", label: "Llama 3.1 8B, fastest host" },
];
/** Claude Code's own names for its models, which its `--model` takes. */
export const CLAUDE_CODE_MODELS: readonly { id: string; label: string }[] = [
  { id: "haiku", label: "Haiku" },
  { id: "sonnet", label: "Sonnet" },
  { id: "opus", label: "Opus" },
];
/** The select's value for a model of OpenRouter's that isn't one of the listed. */
export const ANOTHER_OPENROUTER = "openrouter:*";

export type TidyChoiceView = Pick<DictationKeyView, "tidy_source" | "tidy_model" | "tidy_with" | "openrouter" | "subscription">;
export type TidyOption = { value: string; label: string; disabled?: boolean };
export type TidyGroup = { label: string | null; options: TidyOption[] };

/** The select's value for what the daemon says now. */
export function tidyValue(v: Pick<DictationKeyView, "tidy_source" | "tidy_model">): string {
  const model = v.tidy_model ?? "";
  if (v.tidy_source === "off") return "off";
  if (v.tidy_source === "subscription") return `subscription:${model}`;
  return OPENROUTER_MODELS.some((m) => m.id === model) ? `openrouter:${model}` : ANOTHER_OPENROUTER;
}

/**
 * The select's options, grouped: Off; Claude Code's three when it can tidy (it is Ghost's model
 * or on this computer) or is chosen now; OpenRouter's, disabled until its key is set. A
 * subscription's tidy chosen before there was a choice, with Ghost's model of another kind, is
 * shown as that.
 */
export function tidyOptions(v: TidyChoiceView): TidyGroup[] {
  const groups: TidyGroup[] = [{ label: null, options: [{ value: "off", label: "Off" }] }];
  const sub: TidyOption[] = [];
  if (v.subscription || v.tidy_source === "subscription") {
    for (const m of CLAUDE_CODE_MODELS) sub.push({ value: `subscription:${m.id}`, label: `Claude Code · ${m.label}` });
  }
  if (v.tidy_source === "subscription" && !v.tidy_model) sub.push({ value: "subscription:", label: v.tidy_with ? `Ghost’s model (${v.tidy_with})` : "Ghost’s model" });
  if (sub.length) groups.push({ label: "Your Claude Code, about 3 s", options: sub });
  const locked = !v.openrouter;
  groups.push({
    label: locked ? "OpenRouter, once its key is added" : "OpenRouter, under a second",
    options: [
      ...OPENROUTER_MODELS.map((m) => ({ value: `openrouter:${m.id}`, label: `OpenRouter · ${m.label}`, disabled: locked })),
      { value: ANOTHER_OPENROUTER, label: "Another OpenRouter model…", disabled: locked },
    ],
  });
  return groups;
}

/** What choosing `value` sends: the source and its model (`another` is the typed OpenRouter id). */
export function tidyBody(value: string, another = ""): { tidy_source: "off" | "subscription" | "openrouter"; tidy_model?: string } {
  if (value === "off") return { tidy_source: "off" };
  if (value === ANOTHER_OPENROUTER) return { tidy_source: "openrouter", tidy_model: another.trim() };
  const [source, ...rest] = value.split(":");
  const model = rest.join(":");
  return source === "subscription" ? { tidy_source: "subscription", tidy_model: model } : { tidy_source: "openrouter", tidy_model: model };
}

/** Under the select: where the words go and about how long, plainly. */
export function tidyWhere(v: Pick<DictationKeyView, "tidy_source" | "tidy_with" | "openrouter">): string {
  if (v.tidy_source === "off") return "Ghost types what it heard, as it heard it.";
  if (v.tidy_source === "openrouter")
    return v.openrouter ? "Your words go to OpenRouter, under a second." : "Add your OpenRouter key below to tidy there. Until then Ghost types what it heard.";
  if (v.tidy_with === "Claude Code") return "Your words go to Anthropic through your Claude Code, about 3 s.";
  if (v.tidy_with) return `Your words go to ${v.tidy_with} to be tidied, within 3 s.`;
  return "No model is set up, so Ghost types what it heard.";
}

/** A model id as OpenRouter writes them; null when it may be saved. */
export function tidyModelProblem(m: string): string | null {
  const t = m.trim();
  if (!t) return null; // empty: the default
  if (t.length > 100 || !/^[A-Za-z0-9._:/~-]+$/.test(t)) return "That isn’t an OpenRouter model id, like anthropic/claude-haiku-5.5.";
  return null;
}

/** A line under the section; `command`, when there is one, is in `text` and shown as code. */
export type KeyNote = { tone: "info" | "warn" | "danger"; text: string; command?: string };

/**
 * The lines a state needs, in order: why the key can't work here (and what to do instead), and
 * whether Ghost may type. Empty when all is well.
 */
export function keyNotes(v: Pick<DictationKeyView, "on" | "state" | "typing">): KeyNote[] {
  const out: KeyNote[] = [];
  if (v.state === "unavailable")
    out.push({
      tone: "info",
      text: "This desktop doesn’t let apps use a key this way. Make a keyboard shortcut of your own that runs ghostd dictate toggle; the text then goes to your clipboard.",
      command: "ghostd dictate toggle",
    });
  else if (v.state === "refused") out.push({ tone: "warn", text: "GNOME said no to the key. Turn the switch off and on to be asked again." });
  else if (v.state === "failed") out.push({ tone: "danger", text: "Ghost couldn’t ask GNOME for the key." });
  if (v.on && v.typing === "refused") out.push({ tone: "info", text: "Ghost isn’t allowed to type, so the text goes to your clipboard. Press Ctrl+V to paste it." });
  return out;
}

/** The switch is busy: a request in flight, or GNOME still asking. */
export function keyBusy(v: Pick<DictationKeyView, "state"> | null, pending: boolean): boolean {
  return pending || v?.state === "binding";
}

/** The key and Change key are shown once GNOME has given Ghost a key. */
export function keyShown(v: Pick<DictationKeyView, "on" | "state" | "key">): boolean {
  return v.on && v.state === "ready" && !!v.key;
}
