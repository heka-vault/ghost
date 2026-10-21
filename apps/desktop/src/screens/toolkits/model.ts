/**
 * Owner: toolkits. Pure helpers: the level order, what a toolkit's draft looks like and when
 * it is dirty, counts in words, and how the API's problem sentences map back onto controls.
 */
import type { GrantView, McpServerInfo, ToolkitUpdate, ToolkitView } from "@ghost/client-ts";

/** Most to least cautious. A toolkit's level may sit at the policy's or anywhere left of it. */
export const LEVELS = ["deny", "confirm", "announce", "quiet"] as const;
export type Level = (typeof LEVELS)[number];

export const levelIndex = (level: string) => LEVELS.indexOf(level as Level);

/** Whether `level` is as cautious as `ceiling` or more: what a toolkit may set for a write. */
export const withinCeiling = (level: string, ceiling: string) => levelIndex(level) >= 0 && levelIndex(level) <= levelIndex(ceiling);

/** The levels a write may be set to under its policy level, most cautious first. */
export const allowedLevels = (ceiling: string): Level[] => LEVELS.slice(0, Math.max(0, levelIndex(ceiling)) + 1);

/** A toolkit name as the API accepts it: lowercase letters, digits, _ and -, up to 40 characters. */
export const NAME_RE = /^[a-z0-9][a-z0-9_-]{0,39}$/;

/** "Month close" for month-close, "GitHub heka" for github_heka: the row's words; the key stays as mono detail. */
const WORDS: Record<string, string> = { github: "GitHub", gitlab: "GitLab", mcp: "MCP", api: "API", url: "URL", pr: "PR", id: "ID" };
export function humanize(name: string): string {
  const words = name.split(/[_\-.]+/).filter(Boolean).map((w) => WORDS[w.toLowerCase()] ?? w.toLowerCase());
  if (words.length === 0) return name;
  const [first, ...rest] = words;
  return [WORDS[first.toLowerCase()] ? first : first[0].toUpperCase() + first.slice(1), ...rest].join(" ");
}

export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "2 servers · 3 skills", or what there is to say when one side is empty. */
export function summary(t: Pick<ToolkitUpdate, "grants" | "skills">): string {
  if (!t.grants.length && !t.skills.length) return "reaches nothing yet";
  return [t.grants.length ? count(t.grants.length, "server") : null, t.skills.length ? count(t.skills.length, "skill") : null].filter(Boolean).join(" · ");
}

/** What a scope kind's items are called: "repository" / "repositories". */
export function kindWords(kind: string): { one: string; many: string } {
  const KNOWN: Record<string, [string, string]> = { repository: ["repository", "repositories"], directory: ["folder", "folders"], workspace: ["workspace", "workspaces"] };
  const k = KNOWN[kind];
  return k ? { one: k[0], many: k[1] } : { one: kind, many: `${kind}s` };
}

/* ------------------------------------------------------------------------- drafts */

/** The editable copy of a toolkit: what PUT /toolkits/{name} takes. */
export type Draft = ToolkitUpdate;

export const toDraft = (t: ToolkitView): Draft => ({ description: t.description, skills: [...t.skills], grants: t.grants.map(cleanGrant) });

export const emptyDraft = (description = ""): Draft => ({ description, skills: [], grants: [] });

/** A grant with its lists copied and its levels as a plain object: safe to edit. */
export function cleanGrant(g: GrantView): GrantView {
  return { server: g.server, reads: g.reads ? [...g.reads] : null, writes: g.writes ? [...g.writes] : null, items: g.items ? [...g.items] : null, levels: { ...g.levels } };
}

/** A stable string for a draft, order-insensitive where order carries no meaning, so two drafts compare. */
export function draftKey(d: Draft): string {
  const list = (xs: string[] | null | undefined) => (xs == null ? "*" : [...xs].sort().join(","));
  const grants = [...d.grants]
    .sort((a, b) => a.server.localeCompare(b.server))
    .map((g) => `${g.server}{r:${list(g.reads)};w:${list(g.writes)};i:${list(g.items)};l:${Object.entries(g.levels).sort().map(([k, v]) => `${k}=${v}`).join(",")}}`)
    .join(";");
  return `${d.description.trim()}\n${list(d.skills)}\n${grants}`;
}

export const sameDraft = (a: Draft, b: Draft) => draftKey(a) === draftKey(b);

/** A draft as the API takes it: trimmed description, no empty level maps, lists in a stable order. */
export function toUpdate(d: Draft): ToolkitUpdate {
  return {
    description: d.description.trim(),
    skills: [...new Set(d.skills)],
    grants: d.grants.map((g) => ({ server: g.server, reads: g.reads ? [...new Set(g.reads)] : null, writes: g.writes ? [...new Set(g.writes)] : null, items: g.items ? [...new Set(g.items)] : null, levels: { ...g.levels } })),
  };
}

/* ------------------------------------------------------------------------ problems */

/** A problem without its "toolkit <name>: " prefix: the designer already says which toolkit this is. */
export const problemText = (p: string) => p.replace(/^toolkit\s+[^:]+:\s*/, "");

/** The problem the API raised about one write's level, if any: "<server>.<op> = <level> is looser…". */
export function levelProblem(problems: string[], server: string, op: string): string | null {
  const re = new RegExp(`(^|\\s)${escape(server)}\\.${escape(op)}\\s*=\\s*`);
  return problems.map(problemText).find((p) => re.test(p)) ?? null;
}

/** The problems about one server that aren't about a level: an undeclared server, an item outside its scope, a write it doesn't declare. */
export function serverProblems(problems: string[], server: string): string[] {
  return problems.map(problemText).filter((p) => (p.includes(` ${server} `) || p.includes(`${server}'s`) || p.endsWith(` ${server}`) || p.startsWith(`server ${server}`)) && !new RegExp(`${escape(server)}\\.[a-z0-9_]+\\s*=`).test(p));
}

/** Skills the API says it doesn't have. */
export function missingSkills(problems: string[]): Set<string> {
  return new Set(problems.map(problemText).flatMap((p) => (/^no skill named (.+)$/.exec(p)?.[1] ? [/^no skill named (.+)$/.exec(p)![1]] : [])));
}

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/* -------------------------------------------------------------------------- servers */

/** The reads a server offers the model, in the order the server lists them. */
export const readsOf = (s: McpServerInfo) => s.tools.filter((t) => t.offered);

/** Servers a draft can still add: declared in the file and not yet granted. */
export const addable = (servers: McpServerInfo[], d: Draft) => servers.filter((s) => !d.grants.some((g) => g.server === s.name));

/** Where a toolkit is stored: the policy file, as the Servers screen links it. The query keeps the mock scenario. */
export function policyFileHref(): string {
  const q = new URLSearchParams(window.location.search);
  q.delete("show");
  q.delete("open");
  q.set("policy", "toml");
  return `${window.location.pathname}?${q.toString()}#/policy`;
}
