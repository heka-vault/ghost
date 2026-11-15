/**
 * The words of the card that asks for a yes, and of a decision made to stick (ADR 0070, its
 * card reshaped 2026-11-14): one sentence of what will happen, Yes and No, and for the window's
 * own cards "Always" and "Never" for this action, worded from the rule the daemon offers
 * (`github.close`): the tool's own name and the app's key, as ADR 0017 builds the words that ask
 * for a call. Never anything the app says about itself (its description, its catalog title),
 * and never the change's summary or rationale. Deterministic and bounded, so a hostile or
 * unreadable tool name reads as a short quoted name or "this action", never as a sentence.
 */
import type { AlwaysChoice, AlwaysOffer } from "@ghost/client-ts";

/** The answers on every card that asks for a yes, in every place. */
export const YES = "Yes";
export const NO = "No";
/** Where a voice session is live: the answer can be said. */
export const SAY_IT = "or say it";
/** The disclosure holding the tool, the level, the time left and the target. */
export const DETAILS = "Details";
/** The card's face holds at most this many words, whatever the summary and the tool's name. */
export const FACE_WORDS = 25;
/** The time left shows only in the wait's last minute (seconds). */
export const CLOCK_FROM = 60;

/** Said once, on the confirmation: there is no undo (ADR 0015), a rule is taken back out there. */
export const ALWAYS_WHERE = "You can change this in Settings › Permissions.";

/** Apps whose name is not the key with a capital. Plain words a person says; not the catalog's text. */
const APP_NAMES: Record<string, string> = {
  github: "GitHub", gitlab: "GitLab", gmail: "Gmail", ghost: "Ghost", posthog: "PostHog", ci: "CI", t3: "T3", youtube: "YouTube", linkedin: "LinkedIn", hubspot: "HubSpot", openai: "OpenAI", sendgrid: "SendGrid", clickup: "ClickUp",
};

/**
 * Verbs a tool's name starts or ends with, as the card says them: `issue_write` changes issues,
 * `add_comment` adds comments. A name with none of these is said by its own words, quoted.
 */
const VERBS: Record<string, string> = {
  create: "create", write: "change", update: "change", edit: "change", modify: "change", patch: "change", put: "change",
  delete: "delete", remove: "remove", close: "close", reopen: "reopen", open: "open", merge: "merge", add: "add", set: "set",
  send: "send", post: "post", reply: "reply to", comment: "comment on", move: "move", rename: "rename", archive: "archive",
  assign: "assign", label: "label", run: "run", start: "start", stop: "stop", cancel: "cancel", publish: "publish",
  upload: "upload", push: "push", invite: "invite", approve: "approve", reject: "reject", lock: "lock", unlock: "unlock",
  star: "star", fork: "fork", deploy: "deploy", schedule: "schedule", book: "book", forward: "forward", mark: "mark",
  pin: "pin", unpin: "unpin", share: "share", transfer: "transfer", trigger: "trigger", restart: "restart", notify: "notify",
  resolve: "resolve", snooze: "snooze", manage: "manage", connect: "connect", dispatch: "dispatch", submit: "submit", save: "save",
};
/** Verbs that are also things an action acts on: `add_comment` adds comments, `set_label` sets labels. */
const NOUNS_TOO = new Set(["comment", "label", "post", "schedule", "run", "mark", "star", "pin", "book", "share", "merge", "lock", "reply", "forward", "transfer", "trigger", "deploy", "upload", "push", "fork", "archive", "invite", "move"]);
/** Nouns that read the same in the plural. */
const UNCOUNTED = new Set(["data", "info", "information", "news", "status", "mail", "email", "feedback", "content", "metadata", "access", "settings", "media", "inbox"]);
/** A word of a name the card will say: letters only, short. */
const WORD = /^[a-z]{1,20}$/;
const MAX_TOOL_WORDS = 3;
const MAX_APP_WORDS = 3;
const MAX_APP_CHARS = 24;

export interface ActionWords {
  /** The app as a person says it: `github` → “GitHub”, `google_calendar` → “Google calendar”. */
  app: string;
  /** “close issues”: a verb and what it acts on, when the tool's name reads as one; else null. */
  phrase: string | null;
  /** The tool's own name in a few words, quoted (“issue write”), or “this action”. For the fallback. */
  named: string;
}

/** `github` → “GitHub”, `google_calendar` → “Google calendar”, nothing readable → “this app”. */
export function appWords(key: string): string {
  const k = key.trim().toLowerCase();
  if (APP_NAMES[k]) return APP_NAMES[k];
  const words = k
    .split(/[^a-z0-9]+/)
    .filter((w) => /^[a-z0-9]{1,20}$/.test(w))
    .slice(0, MAX_APP_WORDS);
  if (!words.length) return "this app";
  // A brand keeps its spelling wherever it falls: `github-acme` → “GitHub acme”.
  const said = words.map((w) => APP_NAMES[w] ?? w);
  let out = said.join(" ");
  if (out.length > MAX_APP_CHARS) out = said[0].slice(0, MAX_APP_CHARS);
  return out.charAt(0).toUpperCase() + out.slice(1);
}

/** “issue” → “issues”, “repository” → “repositories”, “inbox” stays. */
function plural(w: string): string {
  if (UNCOUNTED.has(w) || w.endsWith("s")) return w;
  if (/(x|ch|sh|z)$/.test(w)) return `${w}es`;
  if (/[^aeiou]y$/.test(w)) return `${w.slice(0, -1)}ies`;
  return `${w}s`;
}

/** The words of a rule the daemon offers (`app.tool`), from the tool's name and the app's key only. */
export function actionWords(rule: string): ActionWords {
  const dot = rule.indexOf(".");
  const appKey = dot > 0 ? rule.slice(0, dot) : rule;
  const tool = dot > 0 ? rule.slice(dot + 1) : "";
  const app = appWords(appKey);
  const raw = tool
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
  const words = raw.filter((w) => WORD.test(w));
  // Something the card can't say plainly (digits, very long words, no words at all) is "this action".
  const readable = words.length > 0 && words.length === raw.length;
  const named = readable && words.length <= MAX_TOOL_WORDS ? `“${words.join(" ")}”` : words.length ? `“${words.slice(0, MAX_TOOL_WORDS).join(" ")}”` : "this action";
  let phrase: string | null = null;
  if (readable && words.length >= 2 && words.length <= MAX_TOOL_WORDS) {
    const [first, last] = [words[0], words[words.length - 1]];
    const thing = (w: string) => !VERBS[w] || NOUNS_TOO.has(w);
    const verbFirst = !!VERBS[first] && thing(last) && (!VERBS[last] || !NOUNS_TOO.has(first));
    const verbLast = !verbFirst && !!VERBS[last] && !VERBS[first];
    if (verbFirst || verbLast) {
      const verb = verbFirst ? VERBS[first] : VERBS[last];
      const object = verbFirst ? words.slice(1) : words.slice(0, -1);
      if (object.every(thing)) {
        object[object.length - 1] = plural(object[object.length - 1]);
        phrase = `${verb} ${object.join(" ")}`;
      }
    }
  }
  return { app, phrase, named };
}

export interface AlwaysWords {
  /** “Close issues in GitHub”, or ““close” in GitHub”: what the two quiet buttons act on. */
  label: string;
  /** “Always close issues in GitHub”: the allow, whole, for its accessible name and the toast. */
  always: string;
  /** “Never close issues in GitHub”. */
  never: string;
  /** False when a more careful rule of the operator's would still ask: the allow isn't offered. */
  allow: boolean;
  /** Said short in place of “Always” when it would not take effect. */
  held: string | null;
  /** The daemon's whole sentence for that, for Details. */
  heldWhy: string | null;
}

const cap = (s: string) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);

export const HELD_SHORT = "Your rules ask every time";

export function alwaysWords(offer: Pick<AlwaysOffer, "rule" | "allow" | "held">): AlwaysWords {
  const w = actionWords(offer.rule);
  const what = w.phrase ? `${w.phrase} in ${w.app}` : `${w.named} in ${w.app}`;
  return {
    label: cap(what),
    always: w.phrase ? `Always ${what}` : `Always allow ${what}`,
    never: w.phrase ? `Never ${what}` : `Never allow ${what}`,
    allow: offer.allow,
    held: offer.allow ? null : HELD_SHORT,
    heldWhy: offer.allow ? null : (offer.held ?? "Your rules ask for this every time: change that in Permissions."),
  };
}

/** A standing rule set from a decision, as one line: “Always close issues in GitHub”. Permissions' What you've decided. */
export function decidedWords(choice: AlwaysChoice, rule: string): string {
  const w = alwaysWords({ rule, allow: true });
  return choice === "allow" ? w.always : w.never;
}

/** The toast after the click: what now holds, and where to take it back. */
export function alwaysDone(choice: AlwaysChoice, offer: Pick<AlwaysOffer, "rule"> | null | undefined): { title: string; description: string } {
  const w = actionWords(offer?.rule ?? "");
  const what = w.phrase ? `${w.phrase} in ${w.app}` : `${w.named} in ${w.app}`;
  if (choice === "allow") return { title: w.phrase ? `Done. Ghost will ${what} without asking from now on.` : `Done. ${cap(what)} runs without asking from now on.`, description: ALWAYS_WHERE };
  return { title: w.phrase ? `Canceled. Ghost won’t ${what} from now on.` : `Canceled. ${cap(what)} won’t run from now on.`, description: ALWAYS_WHERE };
}

/**
 * The sticky buttons' key handler: Enter never chooses one (ADR 0070). Space and a click do;
 * a decision that outlives the moment is never one keystroke from wherever focus landed.
 */
export function ignoreEnter(e: { key: string; preventDefault: () => void }): void {
  if (e.key === "Enter") e.preventDefault();
}

/** “m:ss left” in the wait's last minute, else null: a clock is shown only when it matters. */
export function clockWords(secondsLeft: number | null): string | null {
  if (secondsLeft == null || secondsLeft <= 0 || secondsLeft > CLOCK_FROM) return null;
  return `${Math.floor(secondsLeft / 60)}:${String(secondsLeft % 60).padStart(2, "0")} left`;
}

const countWords = (s: string) => s.split(/\s+/).filter(Boolean).length;

export interface Face {
  /** The summary as the face shows it: whole, or cut at a word with “…” to keep the bound. */
  summary: string;
  /** The summary was cut: Details has it whole. */
  cut: boolean;
  /** Every word on the face, for the bound's test. */
  text: string;
}

/**
 * The words on the card's face, at most `FACE_WORDS`: the sentence, Yes, No, “or say it” where a
 * voice session is live, the clock in its last minute, the sticky label and its two buttons, and
 * Details. The summary takes what is left and is cut at a word when it would go over.
 */
export function faceOf(summary: string, o: { sayable?: boolean; clock?: string | null; sticky?: AlwaysWords | null; details?: boolean } = {}): Face {
  const rest = [YES, NO, o.sayable ? SAY_IT : "", o.clock ?? "", o.sticky ? `${o.sticky.label} ${o.sticky.allow ? "Always allow" : o.sticky.held} Never allow` : "", o.details === false ? "" : DETAILS].filter(Boolean).join(" ");
  const room = Math.max(4, FACE_WORDS - countWords(rest));
  const words = summary.trim().split(/\s+/).filter(Boolean);
  const cut = words.length > room;
  const shown = cut ? `${words.slice(0, room).join(" ").replace(/[,;:.]+$/, "")}…` : words.join(" ");
  return { summary: shown, cut, text: `${shown} ${rest}`.trim() };
}

export { countWords };
