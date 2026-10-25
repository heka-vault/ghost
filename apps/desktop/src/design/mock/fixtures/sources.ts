/**
 * Owner: policy. What the gate knows (GET /sources, ADR 0022): the agents and the
 * departments, built in, and one source per connected MCP server, with its operations at
 * their resolved levels, the reads it offers, and its allowlist. The policy screen edits
 * levels from this list.
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
  description: "Your agents: hire, retire, persona, voice, memory (ADR 0014)",
  operations: [
    c("create", "Hire a new agent: a conversation with its own persona and job, in a department. Use it at once, without asking, when the operator says 'save this as X' or 'hire someone for X'; it switches them to it.", "low", "announce", ["name", "aliases", "department", "job", "voice", "speed", "persona", "memory"], ["name"]),
    c("retire", "Retire an agent: it leaves the roster and voice routing but keeps its transcript and memory, and can be rehired.", "low", "announce", ["conversation"]),
    c("rehire", "Bring a retired agent back onto the roster.", "low", "announce", ["conversation"]),
    c("move", "Move an agent to another department: it keeps its name, persona, memory and transcript and works with that department's tools, rules and memory from then on.", "medium", "confirm", ["conversation", "department"], ["department"]),
    c("rename", "Rename an agent, change what it answers to, or restate its job.", "low", "announce", ["conversation", "name", "aliases", "job"]),
    c("set_voice", "Change an agent's voice or speaking speed; no voice means the department's.", "low", "announce", ["conversation", "voice", "speed"]),
    c("set_persona", "Set who this agent is, in the operator's own words, verbatim. Never paraphrase.", "medium", "confirm", ["conversation", "text"], ["text"]),
    c("remember", "Pin something the operator asked you to remember, in their words. Never summarized away.", "low", "announce", ["conversation", "text"], ["text"]),
    c("forget", "Drop a pinned note that contains the given words.", "medium", "confirm", ["conversation", "text"], ["text"]),
    c("compact", "Replace this agent's rolling memory with a fresh summary you write: facts, decisions, open items. Only when the operator asks.", "low", "announce", ["conversation", "summary"], ["summary"]),
    c("reset", "Forget everything an agent knows: its memory, notes and transcript. Keeps the name, job and voice.", "high", "confirm", ["conversation"]),
    c("delete", "Remove an agent entirely, its folder included. Retire is the usual way.", "high", "confirm", ["conversation"]),
    c("switch", "Transfer the operator to a department or an agent.", "low", "announce", ["name"], ["name"]),
  ],
  read_tools: [{ name: "list_agents", description: "The departments and the agents in each, with names, aliases and jobs: who the operator can be transferred to." }],
  scope_kind: null,
  scope_items: [],
  department: null,
};

/** The departments as a source (ADR 0024): a rule appended verbatim, a shared fact, the docs and the journal. */
export const DEPARTMENTS: SourceInfo = {
  name: "departments",
  kind: "built-in",
  description: "Your departments: a rule added, a shared fact (ADR 0024)",
  operations: [
    c("add_rule", "Add a standing rule to a department's AGENTS.md, in the operator's own words, verbatim. Never paraphrase, never rewrite what is there.", "medium", "confirm", ["department", "text"], ["text"]),
    c("remember", "Note a fact every agent in a department should know: one line in the department's shared memory.", "low", "announce", ["department", "text"], ["text"]),
  ],
  read_tools: [
    { name: "read_department_doc", description: "A department's documents: with no file, the list; with one, its text." },
    { name: "department_journal", description: "What a department's agents have done, newest first: each change made through Ghost, by whom and to what." },
  ],
  scope_kind: null,
  scope_items: [],
  department: null,
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
    department: info.department ?? null,
  };
}

export default function sources(_s: Scenario, servers: ServerFixture[]): SourceInfo[] {
  return [structuredClone(CONVERSATIONS), structuredClone(DEPARTMENTS), ...servers.filter((f) => f.info.connected).map(sourceOf)];
}
