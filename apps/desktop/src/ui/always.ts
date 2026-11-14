/**
 * The words of a decision made to stick (ADR 0070), on PendingChange and in its toast. Built
 * from the daemon's offer only: `scope` is the tool's name and the app's key in a few plain
 * words (`“issue write” in github`), never anything the app says about itself, and never the
 * change's summary or rationale.
 */
import type { AlwaysChoice, AlwaysOffer } from "@ghost/client-ts";

/** The disclosure on the card: the sticky choices sit behind it, never the default action. */
export const ALWAYS_TOGGLE = "More choices";

/** Said once, on the confirmation: there is no undo (ADR 0015), a rule is taken back out there. */
export const ALWAYS_WHERE = "You can change this in Settings › Permissions.";

export interface AlwaysWords {
  /** "Yes, and don’t ask again for “issue write” in github"; null when it would not take effect. */
  yes: string | null;
  /** "No, and never allow “issue write” in github". */
  no: string;
  /** Said in place of `yes` when a more careful rule of the operator's would still ask. */
  held: string | null;
}

export function alwaysWords(offer: AlwaysOffer): AlwaysWords {
  return {
    yes: offer.allow ? `Yes, and don’t ask again for ${offer.scope}` : null,
    no: `No, and never allow ${offer.scope}`,
    held: offer.allow ? null : (offer.held ?? "Your rules ask for this every time: change that in Permissions."),
  };
}

/** The toast after the click: what now holds, and where to take it back. */
export function alwaysDone(choice: AlwaysChoice, scope: string): { title: string; description: string } {
  return choice === "allow"
    ? { title: `Done. ${capital(scope)} runs without asking from now on.`, description: ALWAYS_WHERE }
    : { title: `Canceled. ${capital(scope)} won’t run from now on.`, description: ALWAYS_WHERE };
}

/** “issue write” → “Issue write”, past the opening quote. */
function capital(s: string): string {
  const i = s.search(/[a-z]/i);
  return i < 0 ? s : s.slice(0, i) + s[i].toUpperCase() + s.slice(i + 1);
}
