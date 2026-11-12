/**
 * Owner: session. The live session: transcript, the pending change, and how the mock
 * answers typed turns. Extend freely; keep the exported shape.
 */
import type { ActivityView, AuditEntry, CallPreview, PendingProposal, QuestionView, Turn } from "@ghost/client-ts";

import { ago, type Scenario } from "../scenarios";

/** The Accounting session; the audit fixture's two recent writes (208, 212) belong to it. */
export const SESSION_ID = "5f2c9e1a8b7d4c3e9a1f0b6d2e4c8a7f";
/** Every transcript has its own session, so its changes are the ones you can see on screen. */
const SESSION_IDS: Record<string, string> = {
  accounting: SESSION_ID,
  ghost: "0b9d4e7a2c6f41e8b3a5d9c1e7f2a4b6",
  legal: "c3e8a1f5b9d24c7e8a6f0b2d4e9c1a57",
};

type Line = [role: Turn["role"], text: string, minutesAgo: number, via?: Turn["via"]];

const TRANSCRIPTS: Record<string, Line[]> = {
  accounting: [
    ["assistant", "Ghost here.", 6.4],
    ["user", "Transfer me to accounting.", 6.3],
    ["assistant", "Transferring you to Accounting.", 6.25],
    ["assistant", "Accounting here. Three billing issues are waiting on your sign-off, and the Stripe renewal note is still pinned.", 6.2],
    ["user", "What are the three?", 5.6],
    ["assistant", "App 212, invoice PDFs round euro totals down a cent. App 208, the annual plan shows the monthly price on the receipt. And infra 77, the nightly ledger export failed twice this week.", 5.5],
    ["user", "Label 212 as a bug and high priority.", 4.9],
    ["assistant", "Done. 212 now has bug and priority high.", 4.8],
    ["user", "Approve 208, Dana\u2019s fix looks right to me.", 4.1, "text"],
    ["assistant", "Approved app 208 and left your note that Dana\u2019s fix looks right.", 4.0],
    ["user", "What\u2019s going on with the ledger export?", 3.2],
    ["assistant", "The last two runs timed out writing to the archive bucket. Priya commented an hour ago that the bucket hit its object limit.", 3.0],
    ["assistant", "Nobody\u2019s assigned yet. Want me to put Priya on it?", 2.95],
  ],
  ghost: [
    ["assistant", "Ghost here.", 4.5],
    ["user", "What\u2019s waiting on me in the app repo?", 4.3],
    ["assistant", "Four issues need your sign-off in ghost-mock/app. The oldest is 151, a login loop on Safari after a password reset, opened nine days ago.", 4.1],
    ["user", "Isn\u2019t that the same as 148?", 3.4],
    ["assistant", "It looks like it. 148 is the Safari cookie bug, and its fix is in review. 151 has the same steps and the same console error.", 3.2],
    ["user", "Close 151 as a duplicate of 148.", 2.6],
  ],
  legal: [
    ["assistant", "Ghost here.", 9.4],
    ["user", "Get me legal, please.", 9.3],
    ["assistant", "Transferring you to Legal.", 9.25],
    ["assistant", "Legal here. Nothing new since yesterday except Marco\u2019s question on the license audit.", 9.2],
    ["user", "Anything new on the license audit?", 8.7],
    ["assistant", "Infra 91 flags two packages under AGPL in the build image: a PDF renderer and a font tool. Marco asked whether they ship to customers.", 8.5],
    ["user", "They don\u2019t, they\u2019re build-time only. Comment that and add needs-legal-review.", 7.9],
    ["assistant", "Commented on infra 91 that both are build-time only and never shipped, and added needs-legal-review.", 7.7],
    ["user", "Remember that the DPA with Northwind renews in January.", 6.8, "text"],
    ["assistant", "Pinned: the Northwind DPA renews in January.", 6.7],
  ],
};

export interface SessionFixture {
  sessionId: string | null;
  turns: Turn[];
  pending: PendingProposal | null;
  /** The session's activity so far (ADR 0055), sent to each new subscriber. */
  activity: ActivityView[];
  /** Questions agents asked (ADR 0051). */
  questions: QuestionView[];
  /** Audit rows of this session's changes that ran without asking. */
  audit: AuditEntry[];
  /** What the story's calls returned, by call id (`GET /calls/{id}`, ADR 0058). */
  results: Record<string, Omit<CallPreview, "id" | "session_id">>;
  /** Canned replies for typed turns: first rule whose pattern matches wins. */
  replies: { match: RegExp; reply: string; calls?: string[]; propose?: Omit<PendingProposal, "id" | "expires_at" | "expires_in_secs"> }[];
  fallbackReply: string;
}

export default function session(s: Scenario): SessionFixture {
  const key = s.session && TRANSCRIPTS[s.session] ? s.session : "ghost";
  const sessionId = s.session ? SESSION_IDS[key] : null;
  const lines = s.session ? TRANSCRIPTS[key] : [];
  let turns: Turn[] = lines.map(([role, text, m, via]) => ({
    role,
    text,
    at: ago(m),
    session_id: sessionId ?? "",
    via: via ?? "voice",
  }));
  // Thinking: the operator just asked something. Speaking: Ghost is saying its answer, which
  // is already in the transcript (the turn arrives as the reply starts playing).
  if (s.phase === "thinking") turns = turns.slice(0, 4);
  if (s.phase === "speaking") turns = turns.slice(0, 5);
  if (s.pending) {
    turns.push({ role: "assistant", text: "Close ghost-mock/app 151 with a comment that it duplicates 148? Say yes to go ahead.", at: ago(2.5), session_id: sessionId ?? "", via: "voice" });
  }
  const pending: PendingProposal | null = s.pending
    ? {
        id: "7c1e4b2a9d8f4e6a8b3c5d7e9f1a2b4c",
        source: "github",
        operation: "close",
        target: "ghost-mock/app#151",
        summary: "On ghost-mock/app#151: close the issue, with a comment: “Duplicate of #148, which has the Safari cookie fix in review.”",
        rationale: "You said 151 is the same bug as 148. Closing is set to ask first in your policy.",
        expires_at: new Date(Date.now() + 52_000).toISOString(),
        expires_in_secs: 52,
      }
    : null;

  const story = s.story && s.session ? STORIES[s.story](sessionId ?? "") : null;
  if (story) turns = story.turns;
  return {
    sessionId,
    turns,
    pending,
    activity: story?.activity ?? [],
    questions: story?.questions ?? [],
    audit: story?.audit ?? [],
    results: story?.results ?? {},
    replies: [
      {
        match: /\bclose\b/i,
        calls: ["github_issue_read"],
        reply: "Close that issue? Say yes, or press Confirm.",
        propose: {
          source: "github",
          operation: "close",
          target: "ghost-mock/app#163",
          summary: "On ghost-mock/app#163: close the issue.",
          rationale: "You asked to close it. Closing is set to ask first.",
        },
      },
      { match: /waiting|sign.?off|what('| i)s (new|up)/i, calls: ["github_issue_list"], reply: "Three issues need your sign-off: app 212, app 208 and infra 77. Want the oldest first?" },
      { match: /label|tag/i, reply: "Done. Labeled it." },
      { match: /comment/i, reply: "Commented." },
      { match: /remember|pin/i, reply: "Pinned." },
      { match: /thank/i, reply: "Anytime." },
    ],
    fallbackReply: "On it. Nothing in the app or infra repos matches that yet.",
  };
}

// ---------------------------------------------------------------- conversation-screen stories

type Story = { turns: Turn[]; activity: ActivityView[]; questions: QuestionView[]; audit: AuditEntry[]; results?: SessionFixture["results"] };

const json = (v: unknown) => JSON.stringify(v, null, 2);
const LONG_COMMENTS = json({
  comments: Array.from({ length: 14 }, (_, i) => ({
    author: ["dana", "priya", "marco"][i % 3],
    at: `2026-11-0${(i % 9) + 1}T1${i % 10}:0${i % 6}:00Z`,
    body: ["Reproduced on Safari 18.2 after a password reset; the session cookie is set on the wrong domain.", "Same console error as 148: `refresh token missing`.", "Fix for 148 is in review, it moves the cookie to the parent domain."][i % 3],
  })),
}).slice(0, 2000);

const t = (session: string, role: Turn["role"], text: string, m: number, via: Turn["via"] = "voice"): Turn => ({ role, text, at: ago(m), session_id: session, via });
const a = (session: string, kind: ActivityView["kind"], m: number, extra: Partial<ActivityView> = {}): ActivityView => ({ kind, session_id: session, at: ago(m), names: [], ...extra });
const call = (session: string, name: string, id: string, from: number, to: number) => [a(session, "call", from, { name, id }), a(session, "result", to, { name, id })];

/** The ghost session's opening, as the other stories start from it. */
const opening = (sid: string): Turn[] => [
  t(sid, "assistant", "Ghost here.", 4.5),
  t(sid, "user", "What\u2019s waiting on me in the app repo?", 4.3),
  t(sid, "assistant", "Four issues need your sign-off in ghost-mock/app. The oldest is 151, a login loop on Safari after a password reset, opened nine days ago.", 4.1),
];

const STORIES: Record<NonNullable<Scenario["story"]>, (sid: string) => Story> = {
  thread: (sid) => ({
    turns: [
      t(sid, "assistant", "Ghost here.", 14.0),
      t(sid, "user", "What\u2019s waiting on me in the app repo?", 13.9),
      t(sid, "assistant", "Four issues need your sign-off in ghost-mock/app. The oldest is 151, a login loop on Safari after a password reset, opened nine days ago.", 13.6),
      t(sid, "user", "Isn\u2019t that the same as 148?", 13.0),
      t(sid, "assistant", "It looks like it. 148 is the Safari cookie bug, and its fix is in review. 151 has the same steps and the same console error.", 12.7),
      t(sid, "user", "Label 151 as a duplicate.", 12.2),
      t(sid, "assistant", "Labeled 151 as a duplicate of 148.", 12.0),
      t(sid, "user", "Ask Accounting whether the Q3 numbers are final.", 3.2, "text"),
      t(sid, "assistant", "Accounting says they\u2019re final except the Stripe fees line, and it has a question for you.", 2.6),
    ],
    activity: [
      a(sid, "you_started", 13.95),
      a(sid, "you_stopped", 13.9),
      ...call(sid, "github_issue_list", "c1", 13.85, 13.8),
      ...call(sid, "github_issue_read", "c2", 13.78, 13.7),
      a(sid, "you_started", 13.05),
      a(sid, "you_stopped", 13.0),
      a(sid, "handover", 12.98, { names: ["github_issue_read"], secs: 0.4 }),
      ...call(sid, "github_issue_read", "c3", 12.95, 12.9),
      ...call(sid, "github_issue_comments", "c4", 12.95, 12.85),
      a(sid, "you_started", 12.25),
      a(sid, "you_stopped", 12.2),
      ...call(sid, "github_issue_write", "c5", 12.15, 12.1),
      ...call(sid, "ghost_ask_agent", "c6", 3.1, 2.7),
    ],
    questions: [
      {
        key: "question:q3-send", kind: "question", source: "agent:accounting", asked_by: "agent:accounting", asked_at: ago(2.65),
        question: "Should I send the Q3 summary to Dana now, or wait for the Stripe fees line?", detail: null,
        choices: ["Send it now", "Wait for Stripe"], choices_only: true, state: "open", urgency: "notify", in_run: true,
        expires_at: ago(-60 * 24), answer: null, answered_at: null, answered_via: null, choice: null, change_id: null,
      },
    ],
    audit: [
      {
        id: "a51dup0000000000000000000000000a", session_id: sid, source: "github", operation: "issue_write", target: "ghost-mock/app#151",
        summary: "Labeled ghost-mock/app#151 duplicate, pointing at #148", autonomy: "quiet", status: "executed", created_at: ago(12.15), conversation: "ghost",
        events: [{ at: ago(12.15), event: "prepared" }, { at: ago(12.1), event: "finished", detail: { status: "success" } }],
      },
    ],
    results: {
      c1: { name: "github_issue_list", read: true, at: ago(13.8), dropped: 0, preview: json({ issues: [{ number: 151, title: "Login loops on Safari after a password reset", age_days: 9 }, { number: 156, title: "Dark mode toggle forgets its setting" }, { number: 160, title: "Export button does nothing on Firefox" }, { number: 163, title: "Typo in the footer" }] }) },
      c2: { name: "github_issue_read", read: true, at: ago(13.7), dropped: 0, preview: json({ number: 151, title: "Login loops on Safari after a password reset", state: "open", author: "dana", labels: ["bug", "auth"], body: "After resetting a password on Safari 18, signing in goes back to the login page. Console: refresh token missing." }) },
      c3: { name: "github_issue_read", read: true, at: ago(12.9), dropped: 0, preview: json({ number: 148, title: "Safari drops the session cookie", state: "open", linked_pr: 4412, review: "in review" }) },
      c4: { name: "github_issue_comments", read: true, at: ago(12.85), dropped: 3140, preview: LONG_COMMENTS },
      c5: { name: "github_issue_write", read: false, at: ago(12.1), dropped: 0, preview: json({ ok: true, status: "success", applied: ["labels: duplicate", "comment: Duplicate of #148"] }) },
      c6: { name: "ghost_ask_agent", read: true, at: ago(2.7), dropped: 0, preview: json({ agent: "accounting", answer: "Final except the Stripe fees line. I asked the operator whether to send the summary now." }) },
    },
  }),
  you: (sid) => ({ turns: opening(sid), activity: [a(sid, "you_started", 0.02)], questions: [], audit: [] }),
  cut: (sid) => ({ turns: opening(sid), activity: [], questions: [], audit: [] }),
  hold: (sid) => ({ turns: opening(sid), activity: [], questions: [], audit: [] }),
  ending: (sid) => ({ turns: [...opening(sid), t(sid, "user", "That\u2019s all, thanks.", 0.3)], activity: [], questions: [], audit: [] }),
  error: (sid) => ({
    turns: [...opening(sid), t(sid, "user", "Read me the newest comment on 151.", 0.4)],
    activity: [a(sid, "you_started", 0.45), a(sid, "you_stopped", 0.4), ...call(sid, "github_issue_comments", "e1", 0.38, 0.3), a(sid, "error", 0.25)],
    questions: [],
    audit: [],
  }),
};

/** What Ghost says in the `cut` story, cut off two seconds in. */
export const LONG_REPLY = "There are three things on it. First, Dana's fix for the cookie domain is in review and waiting on one approval. Second, Priya asked whether the Safari steps also fail on iOS, and nobody has answered. Third, the console error points at the session refresh, not the login form.";
