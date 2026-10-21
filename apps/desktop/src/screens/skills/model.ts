/** Owner: skills. Pure helpers: where a skill comes from in words, what it needs, links. */
import type { SkillInfo } from "@ghost/client-ts";

/** Where Ghost's own skills live; the daemon reports the absolute path, the screen writes it from ~. */
export const GHOST_SKILLS_DIR = "~/.config/ghost/skills";

export const isGhosts = (s: SkillInfo) => s.source === "ghost";

/** "~/projects/heka" → "heka": the project a skill is read from, by its folder's name. */
export const rootName = (source: string) => tildePath(source).replace(/\/+$/, "").split("/").pop() || source;

/** Where a skill comes from, in a word or two: "Ghost's", or the project folder's name. */
export const sourceWord = (s: SkillInfo) => (isGhosts(s) ? "Ghost’s" : rootName(s.source));

/** A path as the design system writes it: the operator's home collapsed to ~. */
export function tildePath(path: string | null | undefined): string {
  if (!path) return "";
  return path.replace(/^(?:\/home\/[^/]+|\/Users\/[^/]+|\/root)(?=\/|$)/, "~");
}

/** "mcp:quickbooks" → "quickbooks": the server a requirement names. Anything else stays as written. */
export const serverOf = (requirement: string) => requirement.replace(/^mcp:/, "");

const WORDS: Record<string, string> = { github: "GitHub", gitlab: "GitLab", mcp: "MCP", api: "API", url: "URL", pr: "PR", id: "ID" };

/** "github_heka" → "GitHub heka": a server's name as the Servers screen says it. */
export function humanize(name: string): string {
  const words = name.split(/[_\-.]+/).filter(Boolean).map((w) => WORDS[w.toLowerCase()] ?? w.toLowerCase());
  if (words.length === 0) return name;
  const [first, ...rest] = words;
  return [WORDS[first.toLowerCase()] ? first : first[0].toUpperCase() + first.slice(1), ...rest].join(" ");
}

/** "a", "a and b", "a, b and c". */
export function listWords(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** "needs quickbooks", "needs quickbooks and calendar": the one warning on a list row. */
export const needsWords = (missing: string[]) => `needs ${listWords(missing.map(serverOf))}`;

export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Whether a refusal means the name is already installed, so adding again with `replace` would go through. */
export const saysInstalled = (message: string) => /already installed/i.test(message);

/** Settings, at its Skills group: where GHOST_SKILL_ROOTS is set. The query keeps the mock scenario. */
export function settingsHref(): string {
  const q = new URLSearchParams(window.location.search);
  q.delete("show");
  q.set("section", "skills");
  return `${window.location.pathname}?${q.toString()}#/settings`;
}
