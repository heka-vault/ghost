/** Owner: servers. Pure helpers: names, what a declaration says in plain words, links into the policy file. */
import type { McpServerInfo, McpToolInfo, McpWriteInfo, SourceInfo } from "@ghost/client-ts";

const WORDS: Record<string, string> = { github: "GitHub", gitlab: "GitLab", dm: "DM", id: "ID", mcp: "MCP", api: "API", ui: "UI", llm: "LLM", tts: "TTS", stt: "STT", url: "URL", pr: "PR" };

/** "github_heka" → "GitHub heka", "issue_read" → "Issue read". The key stays as mono detail. */
export function humanize(name: string): string {
  const words = name.split(/[_\-.]+/).filter(Boolean).map((w) => WORDS[w.toLowerCase()] ?? w.toLowerCase());
  if (words.length === 0) return name;
  const [first, ...rest] = words;
  return [WORDS[first.toLowerCase()] ? first : first[0].toUpperCase() + first.slice(1), ...rest].join(" ");
}

/** The transport in the operator's words: a command Ghost starts, or a URL it connects to. */
export const transportLabel = (transport: string) => (transport === "http" ? "Over HTTP" : transport === "stdio" ? "Runs a command" : humanize(transport));
/** The same, as one word beside a list row. */
export const transportWord = (transport: string) => (transport === "http" ? "HTTP" : transport === "stdio" ? "Command" : humanize(transport));

/**
 * Whether the humanized label already says the id: "Issue read" ↔ issue_read. When it
 * doesn't (camelCase, dots), the row shows the exact id so it can be found in the file.
 */
export const labelSaysId = (name: string) => humanize(name).toLowerCase().replace(/ /g, "_") === name.toLowerCase().replace(/-/g, "_");

/** What a scope kind's items are called: "repository" / "repositories". */
export function kindWords(kind: string): { one: string; many: string } {
  const KNOWN: Record<string, [string, string]> = {
    repository: ["repository", "repositories"],
    directory: ["folder", "folders"],
    workspace: ["workspace", "workspaces"],
  };
  const k = KNOWN[kind];
  return k ? { one: k[0], many: k[1] } : { one: kind, many: `${kind}s` };
}

/** The reads Ghost offers to the model, as they are (ADR 0022). */
export const readsOffered = (s: McpServerInfo): McpToolInfo[] => s.tools.filter((t) => t.offered);

/**
 * Tools the server has that no declaration covers: not a read, and not the tool a declared
 * write calls. Ghost never calls them; the operator can declare one as a write.
 */
export function notOffered(s: McpServerInfo): McpToolInfo[] {
  const called = new Set(s.writes.map((w) => w.tool));
  return s.tools.filter((t) => !t.offered && !called.has(t.name));
}

/** "3 reads · 4 writes": what is offered to look up and what is declared for the gate, or why there is nothing to count. */
export function summary(s: McpServerInfo): string {
  if (!s.connected) return "not connected";
  return `${count(readsOffered(s).length, "read")} · ${count(s.writes.length, "write")}`;
}

export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The declaration's description of a write, from the source the gate built for this server. */
export function writeDescription(source: SourceInfo | null, w: McpWriteInfo): string | null {
  return source?.operations.find((o) => o.name === w.operation)?.description ?? null;
}

/** "owner, repo and issue_number": a list of argument names as a sentence part. */
export function argumentWords(args: string[]): string {
  if (args.length <= 1) return args.join("");
  return `${args.slice(0, -1).join(", ")} and ${args[args.length - 1]}`;
}

/**
 * Where this server's levels live in What Ghost may do. The route is the hash; the source
 * rides in the query before it (`?mock=…&source=github_heka#/policy`), the same way the
 * mock scenario does, so the router's path match is untouched and the scenario survives.
 */
export function policyHref(name: string): string {
  const q = new URLSearchParams(window.location.search);
  q.delete("show");
  q.set("source", name);
  return `${window.location.pathname}?${q.toString()}#/policy`;
}

/** The policy file itself, open in the TOML editor: the one place a server is declared. */
export function policyFileHref(): string {
  const q = new URLSearchParams(window.location.search);
  q.delete("show");
  q.delete("source");
  q.set("policy", "toml");
  return `${window.location.pathname}?${q.toString()}#/policy`;
}
