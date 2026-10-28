/**
 * Owner: apps. Pure helpers for the Apps screens (ADR 0034): an app's state and kind in the
 * operator's words, tool names as labels, which rule decides a tool, what choosing a level
 * sends, and opening a sign-in page in the browser.
 */
import type { AppAction, AppView, PermissionLevel, PermissionsView } from "@ghost/client-ts";

import type { Tone } from "../../ui";

export type Level = NonNullable<PermissionLevel>;

export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** A daemon message as a sentence: capitalized, ending in a period. */
export function sentence(s: string): string {
  const t = s.replace(/^request:\s*/, "").trim();
  if (!t) return t;
  return t[0].toUpperCase() + t.slice(1) + (/[.!?]$/.test(t) ? "" : ".");
}

const WORDS: Record<string, string> = { id: "ID", url: "URL", api: "API", pr: "PR", ui: "UI", me: "me", github: "GitHub", oauth: "OAuth" };

/** "add_issue_comment" → "Add issue comment", "getMe" → "Get me". The exact name stays as mono detail. */
export function toolLabel(name: string): string {
  const words = name
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .split(/[_\-.\s]+/)
    .filter(Boolean)
    .map((w) => WORDS[w.toLowerCase()] ?? w.toLowerCase());
  if (!words.length) return name;
  return words[0][0].toUpperCase() + words[0].slice(1) + (words.length > 1 ? ` ${words.slice(1).join(" ")}` : "");
}

/** The two letters on an app's tile when it has no icon: “GH” for GitHub, “Mo” for Mobbin. */
export function monogram(title: string): string {
  const parts = title.replace(/[()]/g, " ").split(/[\s\-_.]+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  const w = parts[0] ?? "?";
  const caps = w.match(/[A-Z]/g);
  if (caps && caps.length >= 2) return caps.slice(0, 2).join("");
  return w[0].toUpperCase() + (w[1] ?? "");
}

/** Only an https icon is ever loaded (catalog text and icons are untrusted). */
export const safeIcon = (url: string | null | undefined): string | null => (url && /^https:\/\/[^\s"'<>]+$/i.test(url) ? url : null);

/** An app's state in words, and the tone of its badge. */
export function stateWords(a: AppView): { label: string; tone: Tone } {
  if (a.sign_in_url) return { label: "Waiting for you to sign in", tone: "warn" };
  switch (a.state) {
    case "connected":
      return { label: "Connected", tone: "ok" };
    case "connecting":
      return { label: "Connecting…", tone: "info" };
    case "needs_sign_in":
      return { label: "Needs you to sign in again", tone: "warn" };
    default:
      return { label: "Not connected", tone: "danger" };
  }
}

/** How an app is reached, as the operator set it up. */
export function kindWords(kind: AppView["kind"]): string {
  return { sign_in: "Signed in with your account", key: "Uses a password", local: "Runs on this computer", custom: "Added by hand" }[kind] ?? kind;
}

/** What each way to connect means, under its label in the connect dialog. */
export function optionHelp(kind: "sign_in" | "key" | "local"): string {
  return {
    sign_in: "Ghost opens your browser to sign in. Nothing to paste.",
    key: "Paste the password the app gives you. Ghost keeps it in its vault and never shows it again.",
    local: "Ghost starts it on this computer when it starts, with your user’s access to your files.",
  }[kind];
}

/** “13 tools · 2 blocked · 1 asks first”, or why there is nothing to count. */
export function summaryWords(a: AppView): string {
  if (a.state !== "connected") return "";
  const blocked = a.actions.filter((t) => t.level === "block").length;
  const asks = a.actions.filter((t) => t.level === "ask").length;
  return [count(a.actions.length, "tool"), blocked ? `${blocked} blocked` : "", asks ? `${asks} ${asks === 1 ? "asks" : "ask"} first` : ""].filter(Boolean).join(" · ");
}

/* ------------------------------------------------------------- rules, as the gate reads them */

const LISTS = ["block", "ask", "quiet", "allow"] as const;
const RULE = /^\s*([A-Za-z0-9_*?\-/]+)(?:\.([A-Za-z0-9_*?\-.]+))?\s*(?:\(([^()]*)\))?\s*$/;

interface Parsed {
  app: string;
  tool: string;
  conditional: boolean;
}

function parse(rule: string): Parsed | null {
  const m = RULE.exec(rule);
  return m ? { app: m[1], tool: m[2] ?? "*", conditional: !!m[3]?.trim() } : null;
}

const glob = (value: string, pattern: string) =>
  new RegExp(`^${pattern.toLowerCase().replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".")}$`).test(value.toLowerCase());

const names = (p: Parsed, app: string, tool: string) => glob(app, p.app) && glob(tool, p.tool);

/** Whether a rule covers every tool of an app (`github`, `github.*(repo!=site)`). */
export function coversWholeApp(rule: string): boolean {
  return parse(rule)?.tool === "*";
}

/** Rules with argument patterns that cover a whole app: said once on its page, not on every tool. */
export function appConditionalRules(perms: PermissionsView, app: string): { rule: string; list: Level }[] {
  const out: { rule: string; list: Level }[] = [];
  for (const list of LISTS) {
    for (const rule of perms[list]) {
      const p = parse(rule);
      if (p?.conditional && p.tool === "*" && glob(app, p.app)) out.push({ rule, list });
    }
  }
  return out;
}

/** The list the rule is in, if any. */
export function listOf(perms: PermissionsView, rule: string): Level | null {
  return LISTS.find((l) => perms[l].includes(rule)) ?? null;
}

/** The level the mode gives a change no rule names. */
export const modeLevel = (mode: PermissionsView["mode"]): Level => (mode === "ask" ? "ask" : mode === "read-only" ? "block" : "allow");

/**
 * The unconditional rule that decides a tool, most careful list first, as the gate resolves
 * it before a call; null when the mode decides. A read only ever follows a block.
 */
export function decidingRule(perms: PermissionsView, app: string, tool: string, reads: boolean): { rule: string; list: Level } | null {
  for (const list of reads ? (["block"] as const) : LISTS) {
    for (const rule of perms[list]) {
      const p = parse(rule);
      if (p && !p.conditional && names(p, app, tool)) return { rule, list };
    }
  }
  return null;
}

/** Rules with argument patterns that name the tool: each decides some calls. */
export function conditionalRules(perms: PermissionsView, app: string, tool: string): { rule: string; list: Level }[] {
  const out: { rule: string; list: Level }[] = [];
  for (const list of LISTS) {
    for (const rule of perms[list]) {
      const p = parse(rule);
      if (p?.conditional && names(p, app, tool)) out.push({ rule, list });
    }
  }
  return out;
}

/**
 * What choosing a level on a tool sends. The tool's own rule (`github.merge_pull_request`)
 * moves into that list; choosing what the tool would get with no rule of its own (the mode,
 * for a change; Allow, for a read) clears its rule instead, so the file only holds exceptions.
 */
export function choose(perms: PermissionsView, action: AppAction, app: string, level: Level): { rule: string; level: Level | null } {
  const own = action.rule;
  const without: PermissionsView = { ...perms, block: perms.block.filter((r) => r !== own), ask: perms.ask.filter((r) => r !== own), quiet: perms.quiet.filter((r) => r !== own), allow: perms.allow.filter((r) => r !== own) };
  const other = decidingRule(without, app, action.name, action.reads);
  const base = other ? other.list : action.reads ? "allow" : modeLevel(perms.mode);
  return { rule: own, level: level === base && !other ? null : level };
}

/** “block” → “blocked”, for a rule's list in a sentence. */
export const listVerb: Record<Level, string> = { block: "blocked", ask: "asks first", quiet: "runs quietly", allow: "allowed" };

/* -------------------------------------------------------------------------- the browser */

/**
 * Open a sign-in page in the operator's browser: through Tauri's opener inside the app, a new
 * tab in a browser build. Only http(s) addresses; anything else is refused.
 */
export async function openExternal(url: string): Promise<boolean> {
  if (!/^https?:\/\//i.test(url)) return false;
  if ("__TAURI_INTERNALS__" in window) {
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(url);
      return true;
    } catch {
      return false;
    }
  }
  // With noopener the browser hands back no window, so there is nothing to check.
  window.open(url, "_blank", "noopener,noreferrer");
  return true;
}
