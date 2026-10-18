/** Owner: abilities. Pure helpers: names, wishes vs what is loaded, plain words for the plumbing. */
import type { AbilityInfo, PolicyView } from "@ghost/client-ts";

const WORDS: Record<string, string> = { github: "GitHub", gitlab: "GitLab", dm: "DM", onnx: "ONNX", id: "ID", mcp: "MCP", api: "API", ui: "UI", llm: "LLM", tts: "TTS", stt: "STT" };

/** "github_issues" → "GitHub issues", "slack_dm" → "Slack DM". The package name stays as mono detail. */
export function abilityName(name: string): string {
  if (name === "conversations") return "Conversations";
  const words = name.split(/[_-]+/).filter(Boolean).map((w) => WORDS[w.toLowerCase()] ?? w.toLowerCase());
  if (words.length === 0) return name;
  const [first, ...rest] = words;
  return [first[0].toUpperCase() + first.slice(1), ...rest].join(" ");
}

export const isBuiltIn = (a: AbilityInfo) => a.version === "built-in";

/**
 * Whether the operator wants it loaded. GET /abilities reports what is loaded right now; the
 * wish is the policy file's enable list (or, with no list, whatever Ghost loaded from
 * GHOST_ABILITIES). They differ until Ghost restarts.
 */
export function wants(a: AbilityInfo, policy: PolicyView | null, override?: boolean): boolean {
  if (isBuiltIn(a)) return true;
  if (override !== undefined) return override;
  if (policy?.enabled) return policy.enabled.includes(a.name);
  return a.enabled;
}

export type AbilityState = "built-in" | "on" | "off" | "turning-on" | "turning-off" | "blocked";

/**
 * `passiveAllowed` is whether the policy file lists it under [abilities] passive (null when the
 * file couldn't be read). A passive ability that is wanted but not allowed is "blocked": Ghost
 * refuses to start with it (loader.py raises AbilityError), whatever is loaded right now.
 */
export function stateOf(a: AbilityInfo, wish: boolean, passiveAllowed: boolean | null = null): AbilityState {
  if (isBuiltIn(a)) return "built-in";
  if (a.passive && wish && passiveAllowed === false) return "blocked";
  if (wish && a.loaded) return "on";
  if (wish) return "turning-on";
  if (a.loaded) return "turning-off";
  return "off";
}

export const STATE_LABEL: Record<AbilityState, string> = {
  "built-in": "Always on",
  on: "On",
  off: "Off",
  "turning-on": "On after restart",
  "turning-off": "Off after restart",
  blocked: "Won’t start",
};

/** Whether the operator wants it on, read back from its state. */
export const isWished = (s: AbilityState) => s === "built-in" || s === "on" || s === "turning-on" || s === "blocked";

/**
 * Plain labels for what an ability can read and change, in the operator's words. First-party
 * descriptions are written for the model ("the operator", "you write"), so for those the label
 * is hand-written and the model's words wait under “What Ghost is told”. Anything else gets its
 * humanized name ("next_free_slot" → "Next free slot") with its own description under it.
 */
const PLAIN: Record<string, Record<string, string>> = {
  github_issues: {
    list_awaiting_signoff: "Issues waiting for your sign-off",
    get_issue: "An issue’s title, state, labels and text",
    list_comments: "An issue’s recent comments",
    approve: "Approve an issue",
    request_changes: "Ask for changes on an issue",
    comment: "Comment on an issue",
    set_labels: "Add or remove labels",
    close: "Close an issue",
    reopen: "Reopen an issue",
  },
  conversations: {
    list_conversations: "Your lines and what they answer to",
    create: "Start a new line",
    rename: "Rename a line",
    set_voice: "Change a line’s voice or speed",
    set_persona: "Set who a line is, in your words",
    remember: "Pin a note you asked it to keep",
    forget: "Drop a pinned note",
    compact: "Replace a line’s memory with a summary",
    reset: "Clear a line’s memory, notes and transcript",
    delete: "Delete a line",
    switch: "Switch to another line",
  },
};

export function plainLabel(ability: string, name: string): string {
  return PLAIN[ability]?.[name] ?? abilityName(name);
}

/** True when the row's label was written by hand for the operator (first-party abilities). */
export const hasPlainLabel = (ability: string, name: string) => !!PLAIN[ability]?.[name];

/**
 * Whether the humanized label already says the id: "Next free slot" ↔ next_free_slot. When it
 * doesn't (camelCase, dots), the row shows the exact id so it can be found in the policy file.
 */
export const labelSaysId = (name: string) => abilityName(name).toLowerCase().replace(/ /g, "_") === name.toLowerCase().replace(/-/g, "_");

/**
 * One quiet line under the description for an ability that adds less than the rest, so empty
 * blocks never need their own headings. Null when it has things to read and to change.
 */
export function addsSummary(a: AbilityInfo): string | null {
  const reads = a.read_tools.length > 0;
  const changes = a.operations.length > 0;
  const plugs = a.interceptors.length + a.slots.length + a.listeners + a.commands.length > 0;
  if (reads && !changes) return plugs ? "It can look things up, but it can’t change anything." : "Reads only. It can’t change anything.";
  if (!reads && !changes) return plugs ? "Works in the background. Nothing for Ghost to read or change." : "It doesn’t add anything Ghost can use yet.";
  return null;
}

/** Pipeline slots in the operator's words (ADR 0012). */
export const SLOT_LABEL: Record<string, string> = {
  audio_in: "Microphone audio, before anything else",
  after_stt: "Your words, right after speech-to-text",
  before_llm: "What the model is about to read",
  after_llm: "The model’s reply, before Ghost says it",
  before_tts: "The words Ghost is about to speak",
  audio_out: "Ghost’s voice, on its way to the speakers",
};

/** Per-target rules for one operation, from the policy file: [["ghost-mock/infra", "deny"]]. */
export function targetRules(policy: PolicyView | null, ability: string, op: string): [string, string][] {
  const t = policy?.targets?.[ability];
  if (!t) return [];
  return Object.entries(t)
    .filter(([, ops]) => ops[op])
    .map(([target, ops]) => [target, ops[op]]);
}

/** Order for the list: installed packages in the API's order (by name), the built-in last. Stable while toggling. */
export function split(items: AbilityInfo[]): { installed: AbilityInfo[]; builtIn: AbilityInfo[] } {
  return { installed: items.filter((a) => !isBuiltIn(a)), builtIn: items.filter(isBuiltIn) };
}

export const RESTART_COMMAND = "systemctl --user restart ghostd";

/**
 * A link to this ability's section of What Ghost may do. The route is the hash; the ability
 * rides in the query before it (`?mock=…&ability=github_issues#/policy`), the same way the
 * mock scenario does, so the router's path match is untouched and the scenario survives.
 * The policy screen scrolling to and focusing that section is a shared request.
 */
export function policyHref(ability: string): string {
  const q = new URLSearchParams(window.location.search);
  q.delete("show");
  q.set("ability", ability);
  return `${window.location.pathname}?${q.toString()}#/policy`;
}

/** Join names in prose: "Calendar", "Calendar and Slack DM", "A, B and C". */
export function andList(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}
