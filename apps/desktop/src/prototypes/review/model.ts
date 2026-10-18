/**
 * Owner: review. Pure helpers for the review queue: refs, ranking, filters and the URL
 * state. No React here.
 */
import type { ReviewFilter, ReviewFixture, ReviewIssue, ReviewLink, ReviewOperation, ReviewScore } from "../../design/mock/fixtures/review";

type Ref = Pick<ReviewLink, "repo" | "number">;

/** "app-212": the issue's key in the URL. */
export const keyOf = (i: Ref) => `${i.repo.split("/").pop()}-${i.number}`;
/** "app 212": how Ghost and the operator say it. */
export const spokenRef = (i: Ref) => `${i.repo.split("/").pop()} ${i.number}`;
/** "ghost-mock/app#212": the identifier, for mono detail. */
export const fullRef = (i: Ref) => `${i.repo}#${i.number}`;
export const githubUrl = (i: Ref) => `https://github.com/${i.repo}/issues/${i.number}`;
export const sameRef = (a: Ref, b: Ref) => a.repo === b.repo && a.number === b.number;

export const SIGNALS: { key: keyof ReviewScore; label: string }[] = [
  { key: "impact", label: "Unblocks other work" },
  { key: "readiness", label: "Ready to decide" },
  { key: "urgency", label: "Urgency" },
  { key: "markers", label: "Asked for your review" },
  { key: "age", label: "Time waiting on you" },
];

/** The weighted score, 0 to 100. Configurable weights, never an opaque model score (PRD 6). */
export function scoreOf(i: ReviewIssue, w: ReviewFixture["weights"]): number {
  return Math.round(SIGNALS.reduce((sum, s) => sum + i.score[s.key] * w[s.key], 0));
}

export function rank(issues: ReviewIssue[], w: ReviewFixture["weights"]): ReviewIssue[] {
  return [...issues].sort((a, b) => scoreOf(b, w) - scoreOf(a, w) || Date.parse(a.waitingSince) - Date.parse(b.waitingSince));
}

export function matches(i: ReviewIssue, f: ReviewFilter | undefined): boolean {
  if (!f) return true;
  if (f.labels && !f.labels.some((l) => i.labels.includes(l))) return false;
  if (f.repo && i.repo !== f.repo) return false;
  if (f.unblocks && i.blocks.length === 0) return false;
  if (f.minWaitDays && Date.now() - Date.parse(i.waitingSince) < f.minWaitDays * 86_400_000) return false;
  return true;
}

/** How long it has waited, short: "26 h", "3 days", "2 wk". */
export function waited(iso: string): string {
  const h = Math.max(0, (Date.now() - Date.parse(iso)) / 3_600_000);
  if (h < 1) return "now";
  if (h < 36) return `${Math.round(h)} h`;
  const d = Math.round(h / 24);
  if (d < 14) return `${d} days`;
  return `${Math.round(d / 7)} wk`;
}

/** Ghost's suggestion as a verb phrase for "Ghost suggests …". */
export const SUGGEST: Record<ReviewOperation, string> = {
  approve: "approving",
  request_changes: "requesting changes",
  close: "closing it",
  comment: "a comment first",
  set_labels: "relabeling it",
};

/** The wake phrase, as the operator says it. */
export const WAKE = "Hey Ghost";

/**
 * A phrase as the operator would say it right now: as is inside a session, with the wake
 * phrase in front when Ghost is waiting to be woken. Every phrase on the page goes through
 * this, so none of them leaves the wake phrase out when it's needed.
 */
export const spoken = (p: string, inSession: boolean) => (inSession ? p : `${WAKE}, ${p}`);

/** The one wording for "show me the queue", everywhere on the page. */
export const QUEUE_PHRASE = "what’s waiting on me";
export const RESUME_PHRASE = "continue where I left off";
export const WEEK_PHRASE = "read me the week";

/**
 * The one phrase that brings an issue up with Ghost: "open app 212" from passive (the wake
 * phrase starts a new conversation, so the phrase names the issue), "tell me about app 212"
 * inside a session. The Talk it through button sends exactly this, and the screen shows it.
 */
export const talkPhrase = (issue: Ref, inSession: boolean) => (inSession ? `tell me about ${spokenRef(issue)}` : `open ${spokenRef(issue)}`);

/**
 * What the operator can say about the issue in front of them: the talk phrase first, then
 * the decision. In a session the issue is already the subject ("approve that"); from passive
 * each phrase names it.
 */
export function phrasesFor(op: ReviewOperation | null, issue: Ref, inSession: boolean): string[] {
  const ref = spokenRef(issue);
  const talk = talkPhrase(issue, inSession);
  if (!inSession) {
    const act: Record<ReviewOperation, string> = {
      approve: `approve ${ref}`,
      request_changes: `request changes on ${ref}`,
      close: `close ${ref}`,
      comment: `ask Lea about downtime on ${ref}`,
      set_labels: `relabel ${ref}`,
    };
    // The wake phrase once, on the first phrase: it wakes Ghost, and the next is said to it.
    return [spoken(talk, false), ...(op ? [act[op]] : [])];
  }
  switch (op) {
    case "approve":
      return [talk, "approve that", "next"];
    case "request_changes":
      return [talk, "request changes", "next"];
    case "close":
      return [talk, "close it", "next"];
    case "comment":
      return [talk, "ask Lea about downtime", "next"];
    default:
      return [talk, "pause this until later", "next"];
  }
}

/* ------------------------------------------------------------------- URL state */
/**
 * The page's state lives in the query, before the hash, so every state has a URL without a
 * route change: ?mock=session&issue=infra-77&view=week&filter=billing&state=loading#/review.
 * Written with replaceState, which leaves the hash (the route) alone.
 */
export interface ReviewQuery {
  view: "queue" | "week";
  issue: string | null;
  filter: string;
  /** Held states for design work: loading (first load), error (GitHub didn't answer). */
  state: "loading" | "error" | null;
}

export function readQuery(): ReviewQuery {
  const p = new URLSearchParams(window.location.search);
  const state = p.get("state");
  return {
    view: p.get("view") === "week" ? "week" : "queue",
    issue: p.get("issue"),
    filter: p.get("filter") ?? "all",
    state: state === "loading" || state === "error" ? state : null,
  };
}

export function writeQuery(q: ReviewQuery) {
  const p = new URLSearchParams(window.location.search);
  const set = (k: string, v: string | null, dflt?: string) => (v == null || v === dflt ? p.delete(k) : p.set(k, v));
  set("view", q.view, "queue");
  set("issue", q.issue);
  set("filter", q.filter, "all");
  set("state", q.state);
  const s = p.toString();
  window.history.replaceState(window.history.state, "", `${window.location.pathname}${s ? `?${s}` : ""}${window.location.hash}`);
}
