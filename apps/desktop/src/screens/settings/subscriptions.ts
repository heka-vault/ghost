/**
 * Owner: settings. The subscription picker's words and its one action per card (ADR 0078),
 * pure so they are tested without a window: Claude Code, OpenAI Codex and Google Antigravity,
 * each as its own program says it is (`GET /subscriptions`).
 *
 * The words are plain and modest: the vendor's program by its name, its own install command and
 * its own sign-in. Never "Sign in with Claude" or "Sign in with Google" as if Ghost were the one
 * signing in, nothing that says a vendor made or backs Ghost, nothing that says it is free.
 */
import type { SubscriptionJobView, SubscriptionView } from "@ghost/client-ts";

/** What the card's one button does. */
export type CardAction =
  /** Not on this computer: the vendor's own installer, after its command is shown. */
  | "install"
  /** Installed, not signed in: the program's own sign-in, run by Ghost. */
  | "sign_in"
  /** Installed, not signed in, and its sign-in has choices that are the operator's: Ghost opens a terminal running it. */
  | "terminal"
  /** A terminal was opened for its sign-in: look whether it is signed in now. */
  | "look"
  /** Signed in and Ghost can run it: make it Ghost's model. */
  | "use"
  /** It is Ghost's model already. */
  | "in_use"
  /** Signed in, and Ghost can't run it as its model yet. */
  | "coming_next"
  /** Installed and signed in, and Ghost won't run this version: what to do is in `line`. */
  | "fix"
  /** Its install or sign-in is going on. */
  | "busy";

export interface CardFace {
  /** The short state, as a badge. */
  badge: string;
  tone: "ok" | "warn" | "info" | "neutral";
  /** One sentence under the title. */
  line: string;
  action: CardAction;
  /** The button's words; null when there is no button. */
  label: string | null;
}

/** A job that has not ended. */
export function jobRunning(j: SubscriptionJobView | null | undefined): boolean {
  return !!j && (j.state === "running" || j.state === "needs_code");
}

/**
 * The one subscription Continue may make Ghost's model by itself: signed in, one Ghost can
 * run, and the only such one, while Ghost has none. With two or more, the person picks.
 */
export function onlyReady(view: { in_use?: string | null; subscriptions: SubscriptionView[] } | null | undefined): SubscriptionView | null {
  if (!view || view.in_use) return null;
  const ready = view.subscriptions.filter((s) => s.signed_in && s.ghost_can_use && s.usable);
  return ready.length === 1 ? ready[0] : null;
}

export function cardFace(s: SubscriptionView): CardFace {
  if (jobRunning(s.job)) {
    const installing = s.job?.kind === "install";
    return {
      badge: installing ? "Installing" : "Signing in",
      tone: "info",
      line: installing ? `Installing ${s.title} with its own installer…` : s.job?.state === "needs_code" ? `${s.title} is waiting for the code from your browser.` : `Finish signing in to ${s.title} in your browser.`,
      action: "busy",
      label: null,
    };
  }
  if (!s.installed) {
    return { badge: "Not installed", tone: "neutral", line: `${s.title} isn’t on this computer yet. Ghost can install it with its own installer.`, action: "install", label: "Install" };
  }
  if (!s.signed_in) {
    return s.sign_in_here
      ? { badge: "Not signed in", tone: "warn", line: `Sign in with your own ${s.title} account. Its sign-in opens in your browser.`, action: "sign_in", label: "Sign in" }
      : s.job?.kind === "sign_in" && s.job.state === "done"
        ? { badge: "Not signed in", tone: "warn", line: s.job.message ?? `Sign in in the terminal running ${s.sign_in_command}, then look again.`, action: "look", label: "Look again" }
        : { badge: "Not signed in", tone: "warn", line: `${s.title} signs in on a screen of its own, with choices that are yours. Ghost opens a terminal running ${s.sign_in_command} for you.`, action: "terminal", label: "Sign in" };
  }
  if (s.in_use) {
    return { badge: "Ghost uses this", tone: "ok", line: s.line ?? `Signed in. Ghost runs ${s.title} to answer you.`, action: "in_use", label: null };
  }
  if (!s.ghost_can_use) {
    return { badge: "Signed in", tone: "ok", line: `Signed in. Ghost can’t run ${s.title} as its model yet.`, action: "coming_next", label: "Coming next" };
  }
  if (!s.usable) {
    return { badge: "Can’t use this version", tone: "warn", line: s.line ?? `Ghost can’t run this ${s.title}.`, action: "fix", label: "Look again" };
  }
  return { badge: "Signed in", tone: "ok", line: s.line ?? `Signed in. Ghost can run ${s.title} to answer you.`, action: "use", label: "Use it" };
}

/** The line at the top: which one Ghost uses now, plainly. */
export function inUseLine(subs: readonly SubscriptionView[], otherProvider: string | null): string {
  const using = subs.find((s) => s.in_use);
  if (using) return `Ghost uses your ${using.title} now.`;
  if (otherProvider) return `Ghost uses ${otherProvider} now, set up another way. Choose a subscription to switch.`;
  return "Ghost has no model yet. Choose one of your subscriptions.";
}

/** What the confirm dialog says before an install: the vendor's command, as it will run. */
export function installWords(s: Pick<SubscriptionView, "title" | "install_command" | "install_source">): { title: string; body: string } {
  return {
    title: `Install ${s.title}?`,
    body: `Ghost runs ${s.title}’s own installer, as you and never as root, exactly as its makers document it:`,
  };
}

/** A pasted code Ghost will send: one line, not empty, not too long. Null when it may go. */
export function codeProblem(code: string): string | null {
  const c = code.trim();
  if (!c) return "Paste the code from your browser.";
  if (c.length > 512) return "That is longer than a sign-in code.";
  if (/[\u0000-\u001f\u007f]/.test(c)) return "A code is one line.";
  return null;
}

/** How a job ended, for a toast; null while it runs. */
export function endedWords(title: string, j: SubscriptionJobView): { title: string; tone: "ok" | "danger" | undefined } | null {
  if (j.state === "done") return { title: j.kind === "install" ? `${title} is installed` : `${title} is signed in`, tone: "ok" };
  if (j.state === "failed") return { title: j.kind === "install" ? `${title} didn’t install` : `${title} isn’t signed in`, tone: "danger" };
  if (j.state === "cancelled") return { title: "Cancelled", tone: undefined };
  return null;
}
