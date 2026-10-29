/**
 * The mock's copy of ghost_core::permissions (ADR 0034): a rule is `app`, `app.tool` or
 * `app.tool(arg=value, arg!=value)` with `*` and `?` globs; the most careful list that
 * names a tool wins (block, ask, quiet, allow), then the mode. Without a call's arguments a
 * rule with argument patterns can't match, so it never decides a tool's level here; it only
 * marks the tool `conditional`. Foundation-owned, dev only.
 */
import type { PermissionLevel as Nullable, PermissionsView } from "@ghost/client-ts";

import { HttpError } from "./errors";

export const LISTS = ["block", "ask", "quiet", "allow"] as const;
export type ListName = (typeof LISTS)[number];
type PermissionLevel = NonNullable<Nullable>;

const RULE = /^\s*([A-Za-z0-9_*?\-/]+)(?:\.([A-Za-z0-9_*?\-.]+))?\s*(?:\(([^()]*)\))?\s*$/;
const COND = /^\s*([A-Za-z0-9_\-.]+)\s*(!=|=)\s*(.*?)\s*$/;

export interface Rule {
  app: string;
  tool: string;
  conditions: { arg: string; value: string; negated: boolean }[];
}

/** A string as Python's repr wrote it: how the daemon's messages quote the operator's text (kept word for word by the Rust port). */
const repr = (s: string) => (s.includes("'") && !s.includes('"') ? `"${s}"` : `'${s.replace(/'/g, "\\'")}'`);

export function parseRule(text: string): Rule {
  const m = RULE.exec(text);
  if (!m) throw new HttpError(422, `${repr(text)} is not a rule: write app, app.tool or app.tool(arg=value)`);
  const conditions: Rule["conditions"] = [];
  for (const part of (m[3] ?? "").split(",")) {
    if (!part.trim()) continue;
    const c = COND.exec(part);
    if (!c || !c[3]) throw new HttpError(422, `${repr(text)}: ${repr(part.trim())} is not arg=value or arg!=value`);
    conditions.push({ arg: c[1], value: c[3], negated: c[2] === "!=" });
  }
  return { app: m[1], tool: m[2] ?? "*", conditions };
}

export function ruleText(r: Rule): string {
  let t = r.tool === "*" ? r.app : `${r.app}.${r.tool}`;
  if (r.conditions.length) t += `(${r.conditions.map((c) => `${c.arg}${c.negated ? "!=" : "="}${c.value}`).join(", ")})`;
  return t;
}

const glob = (value: string, pattern: string) =>
  new RegExp(`^${pattern.toLowerCase().replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".")}$`).test(value.toLowerCase());

const names = (r: Rule, app: string, tool: string) => glob(app, r.app) && glob(tool, r.tool);

const rulesIn = (p: PermissionsView, list: ListName) => p[list].map(parseRule);

/** What a tool gets before any call: a read is blocked or runs; a change takes the most careful unconditional rule, then the mode. */
export function levelOf(p: PermissionsView, app: string, tool: string, reads: boolean): PermissionLevel {
  const hit = (list: ListName) => rulesIn(p, list).some((r) => !r.conditions.length && names(r, app, tool));
  if (hit("block")) return "block";
  if (reads) return "allow";
  for (const list of ["ask", "quiet", "allow"] as const) if (hit(list)) return list;
  return p.mode === "ask" ? "ask" : p.mode === "read-only" ? "block" : "allow";
}

/** True when a rule with argument patterns names the tool: it decides some calls, not the tool. */
export function conditionalFor(p: PermissionsView, app: string, tool: string): boolean {
  return LISTS.some((list) => rulesIn(p, list).some((r) => r.conditions.length > 0 && names(r, app, tool)));
}

/** The gate's own level for a level (GET /sources still reports these). */
export const AUTONOMY: Record<PermissionLevel, string> = { block: "deny", ask: "confirm", quiet: "quiet", allow: "announce" };

/** Move one rule into a list, or out of every list (`level` null), as Permissions.with_rule does. */
export function withRule(p: PermissionsView, text: string, level: PermissionLevel | null | undefined): PermissionsView {
  if (level != null && !(LISTS as readonly string[]).includes(level)) throw new HttpError(422, `${repr(String(level))} is not one of ${LISTS.join(", ")}`);
  const rule = ruleText(parseRule(text));
  const out = { ...p };
  for (const list of LISTS) out[list] = p[list].filter((r) => ruleText(parseRule(r)) !== rule);
  if (level) out[level] = [...out[level], rule];
  return out;
}

/** `[permissions]` as ghost_core::policy::write_permissions writes it. */
export function permissionsToml(p: PermissionsView): string {
  const lines = ["[permissions]", `mode = "${p.mode}"`];
  for (const list of LISTS) if (p[list].length) lines.push(`${list} = [${p[list].map((r) => JSON.stringify(r)).join(", ")}]`);
  return lines.join("\n");
}

/**
 * The policy file as the mock reads it back: every line a comment, a [table], or `key =
 * value`; `[permissions]` read for its mode and lists (arrays may span lines). Faults are
 * worded with the line, as the daemon's TOML errors are ("… (at line 4, column 1)").
 */
export function readPolicyText(text: string, path: string): PermissionsView {
  const out: PermissionsView = { path, mode: "open", block: [], ask: [], quiet: [], allow: [] };
  const lines = text.split("\n");
  let table = "";
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i].replace(/(^|\s)#.*$/, "").trim();
    if (!raw) continue;
    const t = /^\[([^\]]+)\]$/.exec(raw);
    if (t) {
      table = t[1].trim();
      continue;
    }
    const kv = /^([A-Za-z0-9_\-."*]+)\s*=\s*(.+)$/.exec(raw);
    if (!kv) throw new HttpError(422, `${path}: Expected '=' after a key in a key/value pair (at line ${i + 1}, column ${raw.length + 1})`);
    let value = kv[2];
    // An array may run over several lines.
    while (value.startsWith("[") && !value.includes("]") && i + 1 < lines.length) value += lines[++i].replace(/(^|\s)#.*$/, "").trim();
    const bad = () => new HttpError(422, `${path}: Invalid value (at line ${i + 1}, column ${raw.indexOf("=") + 3})`);
    if (!/^(".*"|'.*'|\[.*\]|\{.*\}|-?\d+(\.\d+)?|true|false)$/.test(value)) throw bad();
    if (table !== "permissions") continue;
    const key = kv[1];
    if (key === "mode") {
      const mode = value.replace(/^["']|["']$/g, "");
      if (mode !== "open" && mode !== "ask" && mode !== "read-only") throw new HttpError(422, `[permissions].mode is open, ask or read-only, not ${repr(mode)}`);
      out.mode = mode;
    } else if ((LISTS as readonly string[]).includes(key)) {
      let items: unknown;
      try {
        items = JSON.parse(value.replace(/'/g, '"').replace(/,\s*\]$/, "]"));
      } catch {
        throw bad();
      }
      if (!Array.isArray(items)) throw new HttpError(422, `${key} must be a list of rules (strings)`);
      out[key as ListName] = items.map((r) => {
        try {
          return ruleText(parseRule(String(r)));
        } catch (e) {
          throw new HttpError(422, `${key}: ${(e as Error).message}`);
        }
      });
    } else {
      throw new HttpError(422, `[permissions] has unknown key(s): ${key}`);
    }
  }
  return out;
}
