/**
 * The mock's copy of ghost_core::permission_choices (ADR 0070): what a waiting change's card
 * may offer to make the decision stick, and the rules set that way. One tool of one app, by
 * its own name; the words from the tool's name and the app's key only. Dev only.
 */
import type { AlwaysOffer, PermissionChoiceView, PermissionsView } from "@ghost/client-ts";

import { levelOf } from "./permissions";

export const HELD = "Your rules ask for this every time: change that in Permissions.";

const APP = /^[A-Za-z0-9_\-/]+$/;
const TOOL = /^[A-Za-z0-9_\-.]+$/;

/** `“issue write” in github`, as `scope_words` builds it. */
export function scopeWords(app: string, tool: string): string {
  const words = tool
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .toLowerCase()
    .match(/[a-z0-9]+/g) ?? [];
  let t = "";
  for (const w of words.slice(0, 6)) {
    if (t.length + w.length + 1 > 40) break;
    t = t ? `${t} ${w}` : w;
  }
  return `“${t || "this tool"}” in ${app === "ghost" ? "Ghost" : app.slice(0, 40)}`;
}

/** The offer for a change of `app.tool` under these permissions; null when no rule can name it. */
export function alwaysOffer(p: PermissionsView, app: string, tool: string): AlwaysOffer | null {
  if (!APP.test(app) || !TOOL.test(tool)) return null;
  const rule = `${app}.${tool}`;
  const after = levelOf({ ...p, allow: [...p.allow, rule] }, app, tool, false);
  const allow = after !== "ask" && after !== "block";
  return allow ? { rule, scope: scopeWords(app, tool), allow } : { rule, scope: scopeWords(app, tool), allow, held: HELD };
}

export function choiceSentence(choice: "allow" | "block", scope: string): string {
  return choice === "allow" ? `${scope} runs without asking` : `${scope} never runs`;
}

const daysAgo = (d: number) => new Date(Date.now() - d * 86_400_000).toISOString();

/** The rules a scenario starts with having set from a decision: `always` has two. */
export function choicesFor(name: string): PermissionChoiceView[] {
  if (name !== "always") return [];
  return [
    { id: 2, rule: "github.add_issue_labels", choice: "allow", sentence: choiceSentence("allow", scopeWords("github", "add_issue_labels")), set_at: daysAgo(0.2), proposal_id: "8a1d22", in_file: true },
    { id: 1, rule: "gmail.delete_email", choice: "block", sentence: choiceSentence("block", scopeWords("gmail", "delete_email")), set_at: daysAgo(3), proposal_id: "77c0e1", in_file: true },
  ];
}
