/**
 * Owner: conversations. Placeholder examples. They show the shape of an answer without
 * borrowing another agent's content, so an empty field never reads as something saved.
 */

/** How an agent talks and what it's for, written the way the operator would. */
export const PERSONA_EXAMPLE = "Short answers, numbers first. Tell me what changed and what needs me, then stop.";

/** A running summary's shape: what's open, what's decided, who owns what. No agent's real memory. */
export const MEMORY_EXAMPLE = "Open: the vendor contract is with Sam for review. Decided: invoices under $500 get approved without asking.";

/** One thing to keep word for word. Generic on purpose: it must never echo a saved note. */
export const NOTE_EXAMPLE = "Reply to anything from the board the same day";

const ALIASES: Record<string, string> = {
  accounting: "billing, the books",
  legal: "compliance, counsel",
  hiring: "recruiting, candidates",
  support: "escalations, customers",
  security: "vulnerabilities, sec",
  standup: "daily, morning update",
};

/** Other names for an agent called `name`: its own if we know a good one, else a generic shape. */
export function aliasExample(name: string): string {
  const key = name.trim().toLowerCase();
  if (!key) return "the front desk, my desk";
  return ALIASES[key] ?? `the ${key} agent, ${key} desk`;
}
