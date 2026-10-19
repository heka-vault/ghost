/** Owner: abilities. Installed abilities with their tools and operations. */
import type { AbilityInfo, OperationInfo } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";

const repoNum = { repo: { type: "string", description: "owner/repo" }, number: { type: "integer" } };
const op = (name: string, description: string, risk: string, autonomy: string, extra: Record<string, unknown> = {}, required: string[] = ["repo", "number"]): OperationInfo => ({
  name,
  description,
  risk,
  autonomy,
  properties: { ...repoNum, ...extra },
  required,
});

/** An ability before its scope fields are filled in by `abilities()` below. */
type BareAbility = Omit<AbilityInfo, "scope_kind" | "scope_items">;

export const GITHUB_ISSUES: BareAbility = {
  name: "github_issues",
  version: "0.1.0",
  description: "Triage GitHub issues: read what needs sign-off, label, comment, close.",
  enabled: true,
  loaded: true,
  passive: false,
  operations: [
    op("approve", "Approve: remove 'needs-signoff', add 'approved', optional comment.", "low", "announce", { comment: { type: "string" } }),
    op("request_changes", "Request changes: remove 'needs-signoff', add 'changes-requested', and comment with what must change.", "low", "announce", { comment: { type: "string" } }, ["repo", "number", "comment"]),
    op("comment", "Add a comment to an issue.", "low", "announce", { comment: { type: "string" } }, ["repo", "number", "comment"]),
    // Levels mirror fixtures/policy.ts skills: the mock re-applies them on every PUT /policy,
    // so the screen shows the same truth before and after a save.
    op("set_labels", "Add and/or remove labels.", "low", "quiet", { add: { type: "array" }, remove: { type: "array" } }),
    op("close", "Close an issue, with an optional comment.", "high", "confirm", { comment: { type: "string" } }),
    op("reopen", "Reopen an issue.", "medium", "confirm"),
  ],
  read_tools: [
    { name: "list_awaiting_signoff", description: "List open issues waiting for the operator's sign-off, newest first." },
    { name: "get_issue", description: "Read one issue: title, state, labels, and a trimmed body." },
    { name: "list_comments", description: "Recent comments on an issue, oldest first, trimmed." },
  ],
  interceptors: [],
  slots: [],
  listeners: 0,
  commands: [],
};

const c = (name: string, description: string, risk: string, autonomy: string, props: string[], required: string[] = []): OperationInfo => ({
  name,
  description,
  risk,
  autonomy,
  properties: Object.fromEntries(props.map((p) => [p, { type: "string" }])),
  required,
});

export const CONVERSATIONS: BareAbility = {
  name: "conversations",
  version: "built-in",
  description: "Saved conversations: persona, voice, memory (ADR 0014)",
  enabled: true,
  loaded: true,
  passive: false,
  operations: [
    c("create", "Start a new saved conversation (a line the operator can be patched to later). Use it at once, without asking, when the operator says 'save this as X' or 'start a conversation called X'; it switches them to it.", "low", "announce", ["name", "aliases", "voice", "speed"], ["name"]),
    c("rename", "Rename a conversation or change what it answers to.", "low", "announce", ["conversation", "name", "aliases"]),
    c("set_voice", "Change the voice or speaking speed of a conversation.", "low", "announce", ["conversation", "voice", "speed"]),
    c("set_persona", "Set who this conversation is, in the operator's own words, verbatim. Never paraphrase.", "medium", "confirm", ["conversation", "text"], ["text"]),
    c("remember", "Pin something the operator asked you to remember, in their words. Never summarized away.", "low", "announce", ["conversation", "text"], ["text"]),
    c("forget", "Drop a pinned note that contains the given words.", "medium", "confirm", ["conversation", "text"], ["text"]),
    c("compact", "Replace this conversation's rolling memory with a fresh summary you write: facts, decisions, open items. Only when the operator asks.", "low", "announce", ["conversation", "summary"], ["summary"]),
    c("reset", "Forget everything in a conversation: its memory, notes and transcript. Keeps the name and voice.", "high", "confirm", ["conversation"]),
    c("delete", "Remove a saved conversation entirely. Ghost itself cannot be deleted.", "high", "confirm", ["conversation"]),
    c("switch", "Patch the operator through to another conversation.", "low", "announce", ["name"], ["name"]),
  ],
  read_tools: [{ name: "list_conversations", description: "The saved conversations (lines) the operator can be patched through to, with their names and aliases." }],
  interceptors: [],
  slots: [],
  listeners: 0,
  commands: [],
};

const EXTRA: BareAbility[] = [
  {
    name: "wake_onnx",
    version: "0.3.1",
    description: "On-device wake word model. Listens to idle audio locally; nothing leaves the machine until it fires.",
    enabled: true,
    loaded: true,
    passive: true,
    operations: [],
    read_tools: [],
    interceptors: [],
    slots: ["audio_in"],
    listeners: 1,
    commands: ["test", "calibrate"],
  },
  {
    name: "calendar",
    version: "0.2.0",
    description: "Reads today’s calendar so Ghost can say what’s next and when you’re free.",
    enabled: true,
    loaded: true,
    passive: false,
    operations: [],
    read_tools: [
      { name: "today", description: "Today’s events with times and attendees." },
      { name: "next_free_slot", description: "The next free half hour today." },
    ],
    interceptors: [],
    slots: [],
    listeners: 0,
    commands: [],
  },
  {
    name: "release_watch",
    version: "0.1.4",
    description: "Watches the release milestone and speaks up when something new lands against it.",
    enabled: true,
    loaded: true,
    passive: false,
    operations: [
      { name: "move_to_milestone", description: "Move an issue to a different milestone.", risk: "medium", autonomy: "confirm", properties: { ...repoNum, milestone: { type: "string" } }, required: ["repo", "number", "milestone"] },
    ],
    read_tools: [{ name: "milestone_status", description: "Open and closed counts for the current release milestone." }],
    interceptors: ["release_summary"],
    slots: ["after_stt"],
    listeners: 2,
    commands: ["status"],
  },
  {
    name: "voice_id",
    version: "0.1.0",
    description: "Learns your voice so Ghost answers only you. Listens to idle audio on this machine to keep its model current.",
    enabled: false,
    loaded: false,
    passive: true,
    operations: [],
    read_tools: [],
    interceptors: [],
    slots: [],
    listeners: 0,
    commands: [],
  },
  {
    name: "slack_dm",
    version: "0.0.9",
    description: "Sends you a Slack DM with a summary when a session ends.",
    enabled: false,
    loaded: false,
    passive: false,
    operations: [],
    read_tools: [],
    interceptors: [],
    slots: [],
    listeners: 0,
    commands: [],
  },
];

/**
 * Extra URL options for this screen's states (dev only, read once at load):
 *   &abilities=none     only the built-in conversations ability (a fresh install with no packages)
 *   &abilities=restart  the operator turned Calendar off and Slack DM on; Ghost hasn't restarted
 *   &abilities=blocked  busy data, but Voice ID is in the enable list and not under
 *                       [abilities] passive (the policy file was edited by hand): Ghost
 *                       won't start like this
 */
function variant(): string | null {
  try {
    return new URLSearchParams(window.location.search).get("abilities");
  } catch {
    return null;
  }
}

export type MockAbility = AbilityInfo;

/** The repositories the operator allows (GHOST_REPOS in fixtures/settings.ts): the allowlist a scope is a subset of. */
export const REPOS = ["ghost-mock/app", "ghost-mock/infra"];
export const BUSY_REPOS = [...REPOS, "ghost-mock/docs", "heka-vault/ghost"];

export default function abilities(s: Scenario): MockAbility[] {
  const v = variant();
  // ADR 0019: how an ability can be scoped (`scope_kind`, null when it can't) and the whole
  // allowlist a line's scope picks from (`scope_items`).
  const scoped = (a: BareAbility): MockAbility => ({
    ...a,
    scope_kind: a.name === "github_issues" ? "repository" : null,
    scope_items: a.name === "github_issues" ? [...(s.data === "busy" ? BUSY_REPOS : REPOS)] : [],
  });
  if (v === "none") return [scoped(structuredClone(CONVERSATIONS))];
  const base = [scoped(structuredClone(GITHUB_ISSUES)), scoped(structuredClone(CONVERSATIONS))];
  const all = s.data === "busy" ? [...base, ...structuredClone(EXTRA).map(scoped)] : base;
  if (v === "blocked") {
    // The busy policy has no enable list, so the wish falls back to `enabled`.
    for (const a of all) if (a.name === "voice_id") a.enabled = true;
  }
  if (v === "restart") {
    // GET /abilities reports what is loaded; the wish lives in the enable list. The mock
    // flips `enabled` on save, so the screen treats `enabled` as the wish and `loaded` as fact.
    for (const a of all) {
      if (a.name === "calendar") a.enabled = false;
      if (a.name === "slack_dm") a.enabled = true;
    }
  }
  return all;
}
