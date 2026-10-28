/**
 * Owner: policy. Pure helpers for Permissions (ADR 0034): the four lists and the mode in the
 * operator's words, a rule read back as a sentence, and where a daemon error points in the file.
 */
import type { PermissionLevel, PermissionsView } from "@ghost/client-ts";

export type Level = NonNullable<PermissionLevel>;
export type Mode = PermissionsView["mode"];

/** The lists, most careful first: the order the gate reads them in. */
export const LISTS: { list: Level; title: string; help: string; empty: string }[] = [
  { list: "block", title: "Blocked", help: "Not offered to Ghost, and refused if it asks. Reads too.", empty: "Nothing blocked." },
  { list: "ask", title: "Ask first", help: "Ghost asks before it makes one of these changes. Your plain request is the yes.", empty: "Nothing asks first beyond the mode." },
  { list: "quiet", title: "Quiet", help: "Runs without a word; Ghost mentions it only if you ask, or if it failed.", empty: "Nothing runs quietly." },
  { list: "allow", title: "Allowed", help: "Runs and Ghost says so, even when the mode would ask or block.", empty: "No exceptions to the mode." },
];

export const MODES: { value: Mode; label: string; help: string }[] = [
  { value: "open", label: "Open", help: "Ghost uses its apps and tells you what it changed." },
  { value: "ask", label: "Ask before changes", help: "Every change waits for your yes, unless a rule allows it." },
  { value: "read-only", label: "Read only", help: "Ghost looks things up and changes nothing, unless a rule allows it." },
];

/** Ghost's own sources: rules name them like apps. */
export const OWN: Record<string, string> = { conversations: "your agents", rules: "Ghost’s rules", skills: "your skills", outputs: "what Ghost makes" };

const RULE = /^\s*([A-Za-z0-9_*?\-/]+)(?:\.([A-Za-z0-9_*?\-.]+))?\s*(?:\(([^()]*)\))?\s*$/;

/** "merge_pull_request" → "Merge pull request". */
function toolWords(tool: string): string {
  const w = tool.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/[_\-.]+/g, " ").trim().toLowerCase();
  return w ? w[0].toUpperCase() + w.slice(1) : tool;
}

/** A rule as a sentence: `github.issue_write(state=closed)` → “Issue write in GitHub, when state is closed”. */
export function ruleWords(rule: string, title: (app: string) => string): string {
  const m = RULE.exec(rule);
  if (!m) return rule;
  const [, app, tool, args] = m;
  const who = app.includes("*") || app.includes("?") ? `apps named ${app}` : title(app);
  let what = !tool || tool === "*" ? `Everything in ${who}` : /[*?]/.test(tool) ? `Tools in ${who} named ${tool}` : `${toolWords(tool)} in ${who}`;
  const conds = (args ?? "")
    .split(",")
    .map((c) => c.trim())
    .filter(Boolean)
    .map((c) => {
      const neg = c.includes("!=");
      const [arg, value] = c.split(neg ? "!=" : "=").map((x) => x.trim());
      return `${arg} ${neg ? "isn’t" : "is"} ${value}`;
    });
  if (conds.length) what += `, when ${conds.join(" and ")}`;
  return what;
}

/** The app a rule names, when it names one plainly (no glob). */
export function ruleApp(rule: string): string | null {
  const m = RULE.exec(rule);
  return m && !/[*?]/.test(m[1]) ? m[1] : null;
}

/** "line 14" → 14: where a daemon error points in the file. */
export function errorLine(message: string): number | null {
  const m = /\bline (\d+)/i.exec(message);
  return m ? Number(m[1]) : null;
}

/** A daemon message as a sentence: capitalized, ending in a period, without the “request:” source. */
export function sentence(s: string): string {
  const t = s.replace(/^request:\s*/, "").trim();
  if (!t) return t;
  return t[0].toUpperCase() + t.slice(1) + (/[.!?]$/.test(t) ? "" : ".");
}
