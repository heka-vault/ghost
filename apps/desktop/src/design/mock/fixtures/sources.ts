/**
 * Owner: policy. What the gate knows (GET /sources, ADR 0034, ADR 0035): Ghost's own
 * operations (the agents, the rules and the outputs, built in) and one source per connected app, its changes as
 * operations and its reads as read tools. Each operation's `autonomy` is the level the
 * permissions give it by name, in the gate's words; the mock works it out on every read.
 */
import type { OperationInfo, SourceInfo } from "@ghost/client-ts";

import type { AppFixture } from "./apps";
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
    c("create", "Hire a new agent: a conversation with its own persona and job. Use it at once, without asking, when the operator says 'save this as X' or 'hire someone for X'; it switches them to it. Leave voice out to speak in Ghost's.", "low", "announce", ["name", "aliases", "job", "voice", "speed", "persona", "memory"], ["name"]),
    c("retire", "Retire an agent: it leaves the roster and voice routing but keeps its transcript and memory, and can be rehired.", "low", "announce", ["conversation"]),
    c("rehire", "Bring a retired agent back onto the roster.", "low", "announce", ["conversation"]),
    c("rename", "Rename an agent, change what it answers to, or restate its job.", "low", "announce", ["conversation", "name", "aliases", "job"]),
    c("set_voice", "Change an agent's voice or speaking speed; no voice means Ghost's.", "low", "announce", ["conversation", "voice", "speed"]),
    c("set_persona", "Set who this agent is, in the operator's own words, verbatim. Never paraphrase.", "medium", "confirm", ["conversation", "text"], ["text"]),
    c("remember", "Pin something the operator asked you to remember, in their words. Never summarized away.", "low", "announce", ["conversation", "text"], ["text"]),
    c("forget", "Drop a pinned note that contains the given words.", "medium", "confirm", ["conversation", "text"], ["text"]),
    c("compact", "Replace this agent's rolling memory with a fresh summary you write: facts, decisions, open items. Only when the operator asks.", "low", "announce", ["conversation", "summary"], ["summary"]),
    c("reset", "Forget everything an agent knows: its memory, notes and transcript. Keeps the name, job and voice.", "high", "confirm", ["conversation"]),
    c("delete", "Remove an agent entirely, its folder included. Retire is the usual way.", "high", "confirm", ["conversation"]),
    c("switch", "Transfer the operator to an agent.", "low", "announce", ["name"], ["name"]),
  ],
  read_tools: [{ name: "list_agents", description: "The agents, with names, aliases and jobs: who the operator can be transferred to. Retired agents are not listed." }],
};

/** Ghost's rules as a source (ADR 0035): one operation, a rule appended to AGENTS.md verbatim. */
export const RULES: SourceInfo = {
  name: "rules",
  kind: "built-in",
  description: "Ghost’s rules: AGENTS.md, in every agent’s prompt (ADR 0035)",
  operations: [
    c("add", "Add a standing rule for every agent to Ghost's AGENTS.md, in the operator's own words, verbatim: 'add a rule: always use UK dates'. Never paraphrase, never rewrite what is there.", "medium", "announce", ["text"], ["text"]),
  ],
  read_tools: [],
};

/** What Ghost makes (ADR 0033): a file in out/, never run. */
export const OUTPUTS: SourceInfo = {
  name: "outputs",
  kind: "built-in",
  description: "What Ghost makes: files in out/ (ADR 0033)",
  operations: [
    c("write", "Make a file for the operator (a page, a summary, a table, a small script) in Ghost's outputs folder. Ghost never runs it.", "low", "announce", ["name", "content"], ["name", "content"]),
    c("replace", "Replace a file already in Ghost's outputs with a new version.", "medium", "announce", ["name", "content"], ["name", "content"]),
  ],
  read_tools: [{ name: "list_outputs", description: "The files Ghost has made in its outputs folder." }],
};

/** A connected app as a source: its changes are operations, its reads are read tools. */
export function sourceOf(f: AppFixture): SourceInfo {
  return {
    name: f.view.name,
    kind: "app",
    description: f.view.title,
    operations: f.tools.filter((t) => !t.reads).map((t) => ({ name: t.name, description: t.description, risk: "medium", autonomy: "announce", properties: {}, required: [] })),
    read_tools: f.tools.filter((t) => t.reads).map((t) => ({ name: t.name, description: t.description })),
  };
}

/** Ghost's own sources; the mock adds one per connected app when it answers. */
export default function sources(_s: Scenario): SourceInfo[] {
  return [structuredClone(CONVERSATIONS), structuredClone(RULES), structuredClone(OUTPUTS)];
}
