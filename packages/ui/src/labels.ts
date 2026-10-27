import type { Autonomy, PermissionLevel, Phase } from "@ghost/client-ts";

export function phaseLabel(phase: Phase): string {
  return {
    starting: "Starting",
    passive: "Waiting for “Hey Ghost”",
    listening: "Listening",
    thinking: "Thinking",
    speaking: "Speaking",
    hold: "Checked out",  // of this conversation, until addressed by name (ADR 0021)
    asking: "Asking you",  // Ghost greeted first and waits for a reply (ADR 0032)
  }[phase];
}

export function autonomyLabel(level: Autonomy | string): string {
  return (
    {
      deny: "Not allowed",
      confirm: "Ask me first",
      announce: "Do it and tell me",
      quiet: "Do it quietly",
    }[level] ?? level
  );
}

/**
 * A permission level in the operator's words (ADR 0034): what a rule, or the mode, does to a
 * change. Allow runs it and Ghost says so; quiet runs it without a word; ask waits for a yes.
 */
export function permissionLabel(level: NonNullable<PermissionLevel> | string): string {
  return ({ allow: "Allow", ask: "Ask first", quiet: "Quiet", block: "Block" } as Record<string, string>)[level] ?? level;
}

/** The gate's own level names (`GET /sources` still reports them) as the permission they come from. */
export function permissionOf(autonomy: Autonomy | string): NonNullable<PermissionLevel> {
  return (({ deny: "block", confirm: "ask", quiet: "quiet", announce: "allow" } as Record<string, NonNullable<PermissionLevel>>)[autonomy] ?? "allow");
}
