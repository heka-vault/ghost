/**
 * Owner: policy. Try a call against the permissions: what the gate would do with it and which
 * rule decides, the way the gate reads them (ADR 0034): every matching rule, the most careful
 * wins (block, then ask, then quiet, then allow); with none, a read runs and a change takes the
 * mode; Ghost's own app runs quietly under Open (ADR 0045). An explanation for the operator, not
 * the gate: the gate in the daemon is still what decides.
 */
import type { PermissionsView } from "@ghost/client-ts";

import type { Level } from "./model";

const ORDER: Level[] = ["block", "ask", "quiet", "allow"];
const RULE = /^\s*([A-Za-z0-9_*?\-/]+)(?:\.([A-Za-z0-9_*?\-.]+))?\s*(?:\(([^()]*)\))?\s*$/;

function glob(pattern: string, s: string): boolean {
  const re = new RegExp(`^${pattern.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".")}$`);
  return re.test(s);
}

/** "state=closed, repo=x" → { state: "closed", repo: "x" }. */
export function parseArgs(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const part of text.split(",")) {
    const [k, ...v] = part.split("=");
    if (k?.trim() && v.length) out[k.trim()] = v.join("=").trim().replace(/^["']|["']$/g, "");
  }
  return out;
}

/** Whether a rule names this call. */
export function matches(rule: string, app: string, tool: string, args: Record<string, string>): boolean {
  const m = RULE.exec(rule);
  if (!m) return false;
  const [, ra, rt, rargs] = m;
  if (!glob(ra, app)) return false;
  if (rt && rt !== "*" && !glob(rt, tool)) return false;
  for (const c of (rargs ?? "").split(",").map((x) => x.trim()).filter(Boolean)) {
    const neg = c.includes("!=");
    const [k, v] = c.split(neg ? "!=" : "=").map((x) => x.trim());
    const have = args[k];
    if (neg ? have === v : have !== v) return false;
  }
  return true;
}

export interface Tried {
  level: Level;
  /** The rule that decides, or null when the mode or a default does. */
  rule: string | null;
  /** Every other rule that names the call, with its list: what the most careful beat. */
  beaten: { rule: string; list: Level }[];
  why: "rule" | "read" | "mode" | "own";
}

export function tryCall(perms: Pick<PermissionsView, "mode" | "block" | "ask" | "quiet" | "allow">, app: string, tool: string, reads: boolean, args: Record<string, string> = {}): Tried {
  const hits = ORDER.flatMap((list) => perms[list].filter((r) => matches(r, app, tool, args)).map((rule) => ({ rule, list })));
  if (hits.length) return { level: hits[0].list, rule: hits[0].rule, beaten: hits.slice(1), why: "rule" };
  if (reads) return { level: "allow", rule: null, beaten: [], why: "read" };
  if (app === "ghost" && perms.mode === "open") return { level: "quiet", rule: null, beaten: [], why: "own" };
  return { level: perms.mode === "ask" ? "ask" : perms.mode === "read-only" ? "block" : "allow", rule: null, beaten: [], why: "mode" };
}

const VERB: Record<Level, string> = { block: "Blocked", ask: "Asks you first", quiet: "Runs quietly", allow: "Runs, and Ghost tells you" };

/** The answer in one sentence. */
export function triedWords(t: Tried, ruleWords: (rule: string) => string, reads: boolean): string {
  const verb = reads && t.level === "allow" ? "Runs" : VERB[t.level];
  switch (t.why) {
    case "rule":
      return `${verb}: your rule “${ruleWords(t.rule ?? "")}” decides it.`;
    case "read":
      return "Runs: it only looks something up, and no rule blocks it.";
    case "own":
      return "Runs quietly: it is one of Ghost’s own, and the item it leaves on Today is the announcement.";
    case "mode":
      return `${verb}: no rule names it, so the mode decides.`;
  }
}
