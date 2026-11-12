/**
 * Owner: settings. Outside tools (ADR 0048): the words and the checks the page shares with its
 * tests. A tool's name is what Ghost says when something comes from it ("From Claude Code"),
 * so it is short and plain: lowercase letters, digits and hyphens.
 */

export const NAME_MAX = 40;

/** What the operator types, as the name Ghost keeps: "Claude Code" is `claude-code`. */
export function toolName(typed: string): string {
  return typed
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, NAME_MAX)
    .replace(/-+$/, "");
}

/** The name as Ghost says it: `claude-code` is "Claude Code". The daemon sends the same. */
export function toolTitle(name: string): string {
  return name
    .split("-")
    .filter(Boolean)
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

/** Why a typed name won't do, in a sentence, or null. */
export function nameProblem(typed: string, taken: string[]): string | null {
  const name = toolName(typed);
  if (!typed.trim()) return null; // nothing typed yet is not an error
  if (!name) return "Use letters or digits, like Claude Code or cursor.";
  if (taken.includes(name)) return `${toolTitle(name)} is already connected. Remove it first for a new token.`;
  return null;
}
