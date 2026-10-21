/**
 * Owner: policy. What the gate knows (GET /sources, ADR 0022): the lines, built in, and one
 * source per connected MCP server, with its operations at their resolved levels, the reads
 * it offers, and what a line can be scoped to. The policy screen edits levels from this
 * list; the lines screens read their scope choices from it.
 */
import type { OperationInfo, SourceInfo } from "@ghost/client-ts";

import type { ServerFixture } from "./mcp";
import type { Scenario } from "../scenarios";

const c = (name: string, description: string, risk: string, autonomy: string, props: string[], required: string[] = []): OperationInfo => ({
  name,
  description,
  risk,
  autonomy,
  properties: Object.fromEntries(props.map((p) => [p, { type: "string" }])),
  required,
});

export const CONVERSATIONS: SourceInfo = {
  name: "conversations",
  kind: "built-in",
  description: "Your lines: persona, voice, memory, scope (ADR 0014)",
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
    c("switch", "Patch the operator through to another line.", "low", "announce", ["name"], ["name"]),
  ],
  read_tools: [{ name: "list_conversations", description: "The saved conversations (lines) the operator can be patched through to, with their names and aliases." }],
  scope_kind: null,
  scope_items: [],
};

/** The declaration's description for an operation, as ghost_core.mcp builds it. */
const DESCRIBED: Record<string, string> = {
  comment: "Add a comment to an issue.",
  set_labels: "Set an issue's labels to the full new list (read it first: labels replaces them all).",
  close: "Close an issue (state_reason: completed, not_planned or duplicate).",
  reopen: "Reopen a closed issue.",
  add_note: "Add a note to today's page.",
  create_event: "Create an event on the primary calendar.",
};

/** A connected server as a source: its offered writes are operations, its offered tools are reads. */
export function sourceOf(f: ServerFixture): SourceInfo {
  const { info } = f;
  return {
    name: info.name,
    kind: "server",
    description: f.prompt.split(". ")[0].replace(/\.?$/, "."),
    operations: info.writes
      .filter((w) => w.offered)
      .map((w) => ({ name: w.operation, description: DESCRIBED[w.operation] ?? `${w.tool} on ${info.name}.`, risk: w.risk, autonomy: w.autonomy, properties: w.properties, required: w.required })),
    read_tools: info.tools.filter((t) => t.offered).map((t) => ({ name: t.name, description: t.description })),
    scope_kind: info.scope?.kind ?? null,
    scope_items: info.scope ? [...info.scope.items] : [],
  };
}

export default function sources(_s: Scenario, servers: ServerFixture[]): SourceInfo[] {
  return [structuredClone(CONVERSATIONS), ...servers.filter((f) => f.info.connected).map(sourceOf)];
}
