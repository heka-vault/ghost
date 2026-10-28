/** Owner: skills. Pure helpers: where a skill comes from in words, what it needs, links. */
import type { SkillInfo } from "@ghost/client-ts";

/** Where Ghost's own skills live; the daemon reports the absolute path, the screen writes it from ~. */
export const GHOST_SKILLS_DIR = "~/.config/ghost/skills";

export const isGhosts = (s: SkillInfo) => s.source === "ghost";
export const isHome = (s: SkillInfo) => s.source === "home";

/** "~/projects/heka" → "heka": a folder's name, when a source is a path. */
export const rootName = (source: string) => tildePath(source).replace(/\/+$/, "").split("/").pop() || source;

/** Where a skill comes from, in a word or two: "Ghost's" or "yours" (~/.agents/skills); anything else by its folder's name. */
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

/** The daemon's words as a sentence: a capital first, a period last. One that starts with a
 *  name ("anthropics/skills has 20 skills", "skills.sh answered 502") keeps the name as written. */
export function sentence(s: string): string {
  const t = s.trim();
  if (!t) return t;
  const named = /^[\w-]+[./][\w-]/.test(t);
  return (named ? t : t[0].toUpperCase() + t.slice(1)) + (/[.!?]$/.test(t) ? "" : ".");
}

/** A commit as people write it: the first seven characters. */
export const shortCommit = (sha: string) => sha.slice(0, 7);

/** "812 bytes", "11.9 KB", "1.2 MB". */
export function sizeWords(bytes: number): string {
  if (bytes < 1000) return `${bytes} bytes`;
  if (bytes < 1_000_000) return `${(bytes / 1000).toFixed(1)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

/** "218,410 installs". */
export const installsWords = (n: number) => (n > 0 ? `${n.toLocaleString("en-US")} ${n === 1 ? "install" : "installs"}` : "new");

/** Whether a refusal means the name is already installed, so adding again with `replace` would go through. */
export const saysInstalled = (message: string) => /already installed/i.test(message);
