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

/** Under the tidy's switch: where the words go, or that nothing does. */
export function tidyLine(tidyWith: string | null | undefined): string {
  return tidyWith ? `Your words go to ${tidyWith} to be tidied before they’re typed.` : "No model is set up, so Ghost types what it heard.";
}

/** OpenRouter's row for the tidy (ADR 0078): where the words go, or what adding its password does. */
export const OPENROUTER_TITLE = "Tidy with OpenRouter (fastest)";
export function openRouterLine(v: Pick<DictationKeyView, "openrouter" | "tidy_model">): string {
  return v.openrouter
    ? `Your OpenRouter password is set, so the tidy asks ${v.tidy_model || "OpenRouter’s quick model"} there instead of your model. It has 3 seconds, like any tidy.`
    : "Add an OpenRouter password and the tidy goes there instead of your model: a small, quick model answers well inside the tidy’s 3 seconds, where a program started for each dictation often can’t.";
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
