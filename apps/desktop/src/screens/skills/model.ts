/** Owner: skills. Pure helpers: where a skill comes from in words, what it needs, links. */
import type { SkillInfo } from "@ghost/client-ts";

/** Where Ghost's own skills live; the daemon reports the absolute path, the screen writes it from ~. */
export const GHOST_SKILLS_DIR = "~/.config/ghost/skills";

export const isGhosts = (s: SkillInfo) => s.source === "ghost";
export const isHome = (s: SkillInfo) => s.source === "home";

/** "~/projects/heka" → "heka": a folder's name, when a source is a path. */
export const rootName = (source: string) => tildePath(source).replace(/\/+$/, "").split("/").pop() || source;

/** Where a skill comes from, in a word or two: "Ghost's", "yours" (~/.agents/skills), or the department's name (ADR 0024). */
export const sourceWord = (s: SkillInfo) => (isGhosts(s) ? "Ghost’s" : isHome(s) ? "yours" : s.source.includes("/") ? rootName(s.source) : s.source.replace(/-/g, " ").replace(/^\w/, (c) => c.toUpperCase()));

/** A path as the design system writes it: the operator's home collapsed to ~. */
export function tildePath(path: string | null | undefined): string {
  if (!path) return "";
  return path.replace(/^(?:\/home\/[^/]+|\/Users\/[^/]+|\/root)(?=\/|$)/, "~");
}

/** "quickbooks": the app a requirement names (ADR 0034). An older SKILL.md may still write "mcp:quickbooks". */
export const appOf = (requirement: string) => requirement.replace(/^(?:mcp|app):/, "");

const WORDS: Record<string, string> = { github: "GitHub", gitlab: "GitLab", api: "API", url: "URL", pr: "PR", id: "ID" };

/** "github" → "GitHub", "month-close" → "Month close": a name as words. */
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
export const needsWords = (missing: string[]) => `needs ${listWords(missing.map(appOf))}`;

export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Whether a refusal means the name is already installed, so adding again with `replace` would go through. */
export const saysInstalled = (message: string) => /already installed/i.test(message);

/** The Departments screen: a project's skills come in through a department's root (ADR 0024). The query keeps the mock scenario. */
export function settingsHref(): string {
  const q = new URLSearchParams(window.location.search);
  q.delete("show");
  return `${window.location.pathname}${q.size ? `?${q.toString()}` : ""}#/departments`;
}
