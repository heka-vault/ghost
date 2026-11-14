/**
 * The operator's own Claude Code as Ghost's model (ADR 0069): what the first run and Settings ›
 * Model say about it. No window, no client: tested on its own.
 *
 * The words are exact and modest: "Claude Code" in plain text as the name of the program Ghost
 * runs; never "Sign in with Claude", never a sign-in inside Ghost, nothing that says Anthropic
 * made or backs Ghost, nothing that says it is free or without limits.
 */
import type { ClaudeCodeView } from "@ghost/client-ts";

export const CLAUDE_CODE = "claude-code";

/** What the model step and the page offer, from what the program says of itself. */
export type ClaudeCodeOffer =
  /** Signed in: lead with it, one click. */
  | { kind: "use"; title: string; body: string; note: string | null }
  /** Installed and not usable as it is: what to do, in the operator's terminal. */
  | { kind: "fix"; title: string; body: string; note: string | null };

export function claudeCodeOffer(cc: ClaudeCodeView | null | undefined): ClaudeCodeOffer | null {
  if (!cc) return null; // not installed: not shown at all
  switch (cc.state) {
    case "plan":
      return {
        kind: "use",
        title: "Claude Code is signed in on this computer",
        body: "Ghost can run it to answer you. Replies count against your Claude plan’s limits, the same ones your coding uses.",
        note: cc.note,
      };
    case "api_key":
      return {
        kind: "use",
        title: "Claude Code is signed in on this computer",
        body: "Ghost can run it to answer you. It’s signed in with an API key, so replies are billed to that key.",
        note: cc.note,
      };
    case "signed_out":
      return { kind: "fix", title: "Claude Code isn’t signed in", body: cc.line, note: cc.note };
    case "unsupported":
      return { kind: "fix", title: "This Claude Code is too old for Ghost", body: cc.line, note: cc.note };
    default:
      return null;
  }
}

/** The settings that choose it: the provider, its model left to the program's default. */
export function claudeCodeValues(keys: readonly string[]): Record<string, string | null> {
  return { ...Object.fromEntries(keys.map((k) => [k, null])), GHOST_LLM_PROVIDER: CLAUDE_CODE };
}
