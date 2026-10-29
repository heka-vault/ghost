/** Owner: audit. Every write Ghost made or proposed, with its event trail. */
import type { AuditEntry, Schemas } from "@ghost/client-ts";

import { ago, hex, type Scenario, seeded } from "../scenarios";

type AuditEventView = Schemas["AuditEventView"];

const ISSUES: [string, string][] = [
  ["ghost-mock/app#212", "invoice PDFs round euro totals down"],
  ["ghost-mock/app#208", "annual plan receipt shows the monthly price"],
  ["ghost-mock/infra#77", "nightly ledger export times out"],
  ["ghost-mock/app#151", "login loop on Safari after password reset"],
  ["ghost-mock/app#148", "Safari drops the session cookie"],
  ["ghost-mock/app#163", "dark mode resets on reload"],
  ["ghost-mock/app#170", "search ignores accented characters"],
  ["ghost-mock/app#174", "CSV import drops the last row"],
  ["ghost-mock/infra#91", "AGPL packages in the build image"],
  ["ghost-mock/infra#83", "Terraform drift on staging DNS"],
  ["ghost-mock/infra#88", "rotate the staging database password"],
  ["ghost-mock/app#181", "typo on the pricing page"],
];

/** What Ghost said on each issue: comments, approval notes, change requests, close reasons. */
interface IssueLines { comment: string[]; approve: string[]; changes: string[]; close: string[] }
const LINES: Record<string, IssueLines> = {
  "ghost-mock/app#212": {
    comment: ["Marta, is this only for EUR, or do GBP invoices round too?", "Attaching the invoice from Halden GmbH: 1,249.995 shows as 1,249.99."],
    approve: ["Banker’s rounding in the PDF matches the ledger now."],
    changes: ["Needs a test with a three-decimal tax line."],
    close: ["Fixed in #219, shipped in 4.12."],
  },
  "ghost-mock/app#208": {
    comment: ["The receipt template still reads plan.monthly_price.", "Dana, can you check the Stripe webhook payload for annual plans?"],
    approve: ["Dana’s fix looks right to me."],
    changes: ["The proration line still shows the monthly figure."],
    close: ["Duplicate of #201."],
  },
  "ghost-mock/infra#77": {
    comment: ["Priya, is the archive bucket quota the cause?", "Two timeouts this week, both after 02:00 UTC when the backup runs."],
    approve: ["Splitting the export by month gets it under the 15 minute limit."],
    changes: ["Keep the old export behind a flag until month-end close."],
    close: ["Bucket quota raised; three clean runs since."],
  },
  "ghost-mock/app#151": {
    comment: ["Can you attach the console log from Safari 18?", "Does it still loop with ITP turned off?"],
    approve: ["The SameSite change fixes it on my Safari too."],
    changes: ["Needs the same fix on the magic-link path."],
    close: ["Duplicate of #148."],
  },
  "ghost-mock/app#148": {
    comment: ["Safari drops the cookie after the 302 from /reset. Repro steps in the thread.", "Leo, is this the same ITP change that hit the docs site?"],
    approve: ["Setting SameSite=Lax on the session cookie fixes it."],
    changes: ["Please add a Playwright test on WebKit."],
    close: ["Fixed by #155."],
  },
  "ghost-mock/app#163": {
    comment: ["The theme is read before localStorage loads; that’s the reset.", "Does it happen on a hard reload only, or every navigation?"],
    approve: ["Reading the theme in the inline script fixes the flash too."],
    changes: ["Respect prefers-color-scheme when nothing is stored."],
    close: ["Can’t reproduce on 4.12. Reopen if it comes back."],
  },
  "ghost-mock/app#170": {
    comment: ["Search should fold accents: café should match cafe.", "Is this Postgres unaccent, or the client filter?"],
    approve: ["unaccent on both sides of the query works."],
    changes: ["Index the folded column, or this gets slow."],
    close: ["Fixed in #176."],
  },
  "ghost-mock/app#174": {
    comment: ["Only when the file has no trailing newline.", "Sam, can you send the CSV from the Nordwind import?"],
    approve: ["The parser now flushes the last row."],
    changes: ["Needs a test for the empty-file case."],
    close: ["Fixed in 4.11.2."],
  },
  "ghost-mock/infra#91": {
    comment: ["Two AGPL packages come in through the PDF renderer.", "Legal wants a list before Friday."],
    approve: ["Swapping to the MIT renderer clears both."],
    changes: ["Keep the licence scan in CI so this doesn’t come back."],
    close: ["Both packages removed in the 2026-10 base image."],
  },
  "ghost-mock/infra#83": {
    comment: ["Drift is the TTL on the staging CNAME; someone changed it by hand.", "Is anyone still editing staging DNS in the console?"],
    approve: ["Importing the record and pinning the TTL fixes the drift."],
    changes: ["Lock the zone so console edits fail."],
    close: ["Clean plan for a week."],
  },
  "ghost-mock/infra#88": {
    comment: ["Rotation is due; the password is 400 days old.", "Do the preview apps read it from Vault or from the env file?"],
    approve: ["Vault lease plus the restart hook looks right."],
    changes: ["Rotate the replica user too."],
    close: ["Rotated October 9."],
  },
  "ghost-mock/app#181": {
    comment: ["“Anually” on the pricing page, second card.", "Copy lives in the CMS, not the repo."],
    approve: ["Typo fixed in the CMS and the fallback string."],
    changes: ["The German page has the same typo."],
    close: ["Fixed in the CMS."],
  },
};

/**
 * Each step is written exactly as the gate's describe() writes it, because that is
 * what the gate stores in step_applied and step_failed: "labels: add bug, priority-high",
 * "comment: <body>", "closed the issue".
 */
type Made = { summary: string; steps: string[] };
/** `line(t, kind)` hands out that issue's lines in turn, so one day never shows the same comment twice. */
type Op = { op: string; autonomy: string; make: (t: string, line: (t: string, kind: keyof IssueLines) => string) => Made };
const OPS: Op[] = [
  { op: "set_labels", autonomy: "quiet", make: (t) => ({ summary: `On ${t}: labels: add bug, priority-high.`, steps: ["labels: add bug, priority-high"] }) },
  { op: "set_labels", autonomy: "quiet", make: (t) => ({ summary: `On ${t}: labels: add needs-info.`, steps: ["labels: add needs-info"] }) },
  {
    op: "comment", autonomy: "announce",
    make: (t, line) => {
      const body = line(t, "comment");
      return { summary: `On ${t}: comment: “${body}”`, steps: [`comment: ${body}`] };
    },
  },
  {
    op: "approve", autonomy: "announce",
    make: (t, line) => {
      const body = line(t, "approve");
      return { summary: `On ${t}: approved, with a comment: “${body}”`, steps: ["labels: add approved; remove needs-signoff", `comment: ${body}`] };
    },
  },
  {
    op: "request_changes", autonomy: "announce",
    make: (t, line) => {
      const body = line(t, "changes");
      return { summary: `On ${t}: requested changes: “${body}”`, steps: ["labels: add changes-requested; remove needs-signoff", `comment: ${body}`] };
    },
  },
  {
    op: "close", autonomy: "confirm",
    make: (t, line) => {
      const body = line(t, "close");
      return { summary: `On ${t}: closed the issue, with a comment: “${body}”`, steps: [`comment: ${body}`, "closed the issue"] };
    },
  },
  { op: "reopen", autonomy: "confirm", make: (t) => ({ summary: `On ${t}: reopened the issue.`, steps: ["open the issue"] }) },
];
/** Conversation writes, with steps as ghost_core::conversations describes them (the operator's text quoted as Python's repr did, kept word for word by the port). */
const CONV_OPS: { op: string; autonomy: string; target: string; summary: string; steps: string[] }[] = [
  ...["Stripe renewal is due November 3.", "Marta signs off on anything over 5,000.", "Close the books by the 5th; Priya needs the export the day before."].map((note) => ({
    op: "remember", autonomy: "announce", target: "conversation:accounting", summary: `On the Accounting line: pinned “${note}”`, steps: [`remember: ${note}`],
  })),
  { op: "set_voice", autonomy: "announce", target: "conversation:legal", summary: "On the Legal line: voice set to British female: Emma, speed 0.95.", steps: ["voice bf_emma at 0.95x"] },
  { op: "set_persona", autonomy: "confirm", target: "conversation:standup", summary: "On the Standup line: persona set, in your words.", steps: ["persona: 'Keep it to three bullets. Blockers first, then what shipped.'"] },
  { op: "create", autonomy: "announce", target: "conversation:standup", summary: "Started a new line: Standup.", steps: ["start conversation 'Standup'"] },
];

const CLASSIFIERS = ["jev", "jev", "regex", "ui"];
const YES = ["yes", "yeah go ahead", "do it", "yep, close it"];
const NO = ["no wait", "not yet", "hold on, no"];
/** Turns that weren't a clear yes: the operator moved on, and the gate canceled. */
const UNCLEAR = ["what’s left on the board for tomorrow", "hmm, let me think about that", "actually, read me the next one"];
/**
 * How a waiting write ends up canceled, in the gate's and the pipeline's own reasons. Cycled,
 * so every scenario with a handful of cancels shows each one: a spoken no, an answer that
 * wasn't a clear yes, “never mind” (operator cancelled), sleep, switching lines, and the
 * app's Cancel button.
 */
const CANCELS = ["negative", "unclear", "operator cancelled", "went to sleep", "switched conversation", "cancelled from the desktop app"];

function ev(at: number, event: string, detail?: Record<string, unknown>): AuditEventView {
  return { at: ago(at), event, detail: detail ?? null };
}

/**
 * The agent a GitHub write was made with: billing issues from Accounting (now and then from
 * the Q3 audit agent), the rest of the app repo from App, infra from Infra. The remainder was
 * done from Ghost.
 */
const BILLING = new Set(["ghost-mock/app#212", "ghost-mock/app#208"]);
function whereMade(target: string, rand: () => number): Pick<AuditEntry, "conversation"> {
  if (BILLING.has(target)) return { conversation: rand() < 0.35 ? "q3-audit" : "accounting" };
  if (rand() < 0.4) return { conversation: null };
  if (target.startsWith("ghost-mock/infra")) return { conversation: "infra" };
  return { conversation: "app" };
}

export default function audit(s: Scenario): AuditEntry[] {
  if (s.data === "empty") return [];
  const count = s.data === "busy" ? 200 : 26;
  const rand = seeded(s.data === "busy" ? 4242 : 1717);
  const pick = <T,>(xs: T[]) => xs[Math.floor(rand() * xs.length)];
  // Each issue's lines go out in turn (a random start), so the same comment never repeats on
  // an issue until every other one has been used.
  const turn = new Map<string, number>();
  const line = (t: string, kind: keyof IssueLines) => {
    const xs = LINES[t][kind];
    const k = `${t}|${kind}`;
    const i = turn.get(k) ?? Math.floor(rand() * xs.length);
    turn.set(k, i + 1);
    return xs[i % xs.length];
  };
  const entries: AuditEntry[] = [];
  let minutes = 8;
  const sessions = Array.from({ length: Math.ceil(count / 5) }, () => hex(32, rand));
  let cancels = 0;

  for (let i = 0; i < count; i++) {
    minutes += s.data === "busy" ? 20 + rand() * 240 : 12 + rand() * 300;
    const session_id = sessions[Math.floor(i / 5)];
    const id = hex(32, rand);
    const conv = rand() < 0.14;
    const o = pick(OPS);
    const [target] = pick(ISSUES);
    const made = o.make(target, line);
    const c = pick(CONV_OPS);
    const source = conv ? "conversations" : "github";
    const operation = conv ? c.op : o.op;
    const autonomy = conv ? c.autonomy : o.autonomy;
    const steps = conv ? c.steps : made.steps;
    const tgt = conv ? c.target : target;
    const summary = conv ? c.summary : made.summary;
    const r = rand();
    let status = "executed";
    if (autonomy === "confirm") status = r < 0.62 ? "executed" : r < 0.8 ? "cancelled" : r < 0.9 ? "expired" : "stale";
    else status = r < 0.9 ? "executed" : r < 0.95 || steps.length < 2 ? "failed" : "partial";

    const t = minutes;
    const key = conv ? tgt : `github:${tgt}`;
    const events: AuditEventView[] = [ev(t, "prepared", { source, operation, target: key, autonomy })];
    if (autonomy === "confirm") {
      const classifier = pick(CLASSIFIERS);
      if (status === "cancelled") {
        const reason = CANCELS[cancels++ % CANCELS.length];
        const spoken = classifier === "ui" ? "jev" : classifier;
        if (reason === "negative") events.push(ev(t - 0.2, "user_turn", { verdict: "negative", text: pick(NO), classifier: spoken }));
        if (reason === "unclear") events.push(ev(t - 0.2, "user_turn", { verdict: "unclear", text: pick(UNCLEAR), classifier: spoken }));
        events.push(ev(t - 0.2, "cancelled", { reason }));
      } else if (status === "expired") {
        events.push(ev(t - 1, "expired"));
      } else {
        events.push(ev(t - 0.2, "user_turn", classifier === "ui" ? { verdict: "affirmative", classifier } : { verdict: "affirmative", text: pick(YES), classifier }));
      }
    }
    if (status === "stale") events.push(ev(t - 0.25, "stale", { was: "etag-41", now: "etag-42" }));
    if (["executed", "failed", "partial"].includes(status)) {
      events.push(ev(t - 0.25, "claimed"));
      steps.forEach((step, k) => {
        const fail = (status === "failed" && k === 0) || (status === "partial" && k === steps.length - 1 && k > 0);
        events.push(ev(t - 0.26, fail ? "step_failed" : "step_applied", fail ? { index: k, step, error: "GitHub answered 502 Bad Gateway" } : { index: k, step }));
      });
      events.push(ev(t - 0.27, "finished", { status: status === "failed" ? "failed" : status === "partial" ? "partial" : "success" }));
    }
    // The real gate stores a failed or partial write as "executed"; its finished event says how
    // it went. Keep both shapes in the fixture so the screen proves it reads either.
    const stored = (status === "failed" || status === "partial") && rand() < 0.6 ? "executed" : status;
    const where = conv ? { conversation: tgt.replace(/^conversation:/, "") } : whereMade(tgt, rand);
    entries.push({ id, session_id, source, operation, target: tgt, summary, autonomy, status: stored, created_at: ago(t), events, ...where });
  }
  // One of each way a waiting write gets canceled, early in the log so the first page shows
  // them: an answer that wasn't a clear yes, “never mind”, switching lines, the app's Cancel,
  // and sleep. The cycle above spreads more of them through the older rows.
  const cancelled = (at: number, operation: string, target: string, summary: string, reason: string, turn?: Record<string, unknown>, source = "github"): AuditEntry => ({
    id: hex(32, rand), session_id: sessions[0], source, operation, target, summary, autonomy: "confirm", status: "cancelled", created_at: ago(at),
    events: [
      ev(at, "prepared", { source, operation, target: source === "github" ? `github:${target}` : target, autonomy: "confirm" }),
      ...(turn ? [ev(at - 0.2, "user_turn", turn)] : []),
      ev(at - 0.2, "cancelled", { reason }),
    ],
  });
  if (count > 0) {
    entries.push(
      cancelled(38, "close", "ghost-mock/app#151", "On ghost-mock/app#151: closed the issue, with a comment: “Duplicate of #148.”", "unclear", { verdict: "unclear", text: UNCLEAR[0], classifier: "jev" }),
      cancelled(71, "reopen", "ghost-mock/app#163", "On ghost-mock/app#163: reopened the issue.", "operator cancelled"),
      cancelled(96, "close", "ghost-mock/infra#91", "On ghost-mock/infra#91: closed the issue, with a comment: “Both packages removed in the 2026-10 base image.”", "switched conversation"),
      cancelled(133, "close", "ghost-mock/app#174", "On ghost-mock/app#174: closed the issue, with a comment: “Fixed in 4.11.2.”", "cancelled from the desktop app"),
      cancelled(170, "set_persona", "conversation:standup", "On the Standup line: persona set, in your words.", "went to sleep", undefined, "conversations"),
    );
  }
  // A wait the gate never closed: stored "pending" with only its prepared event, from a day
  // ago. The real daemon leaves these behind when it stops while a change waits; the screen
  // must read them as lapsed, never as waiting. Two in the busy log, one otherwise.
  const lapsed = (at: number, target: string, summary: string): AuditEntry => ({
    id: hex(32, rand), session_id: hex(32, rand), source: "github", operation: "close", target, summary, autonomy: "confirm", status: "pending", created_at: ago(at),
    events: [ev(at, "prepared", { source: "github", operation: "close", target: `github:${target}`, autonomy: "confirm" })],
  });
  if (count > 0) {
    entries.push(lapsed(60 * 26 + 17, "ghost-mock/app#170", "On ghost-mock/app#170: closed the issue, with a comment: “Fixed in #176.”"));
    if (s.data === "busy") entries.push(lapsed(60 * 26 + 21, "ghost-mock/infra#83", "On ghost-mock/infra#83: closed the issue, with a comment: “Clean plan for a week.”"));
  }
  // The two writes from the live transcript (fixtures/session.ts), newest first.
  const recentSession = "5f2c9e1a8b7d4c3e9a1f0b6d2e4c8a7f";
  const recent: AuditEntry[] = [
    {
      id: hex(32, rand), session_id: recentSession, source: "github", operation: "approve", target: "ghost-mock/app#208",
      summary: "On ghost-mock/app#208: approved, with a comment: “Dana’s fix looks right to me.”", autonomy: "announce", status: "executed", created_at: ago(4.0),
      conversation: "accounting",
      events: [ev(4.0, "prepared", { source: "github", operation: "approve", target: "github:ghost-mock/app#208", autonomy: "announce" }), ev(3.99, "claimed"), ev(3.98, "step_applied", { index: 0, step: "labels: add approved; remove needs-signoff" }), ev(3.97, "step_applied", { index: 1, step: "comment: Dana’s fix looks right to me." }), ev(3.96, "finished", { status: "success" })],
    },
    {
      id: hex(32, rand), session_id: recentSession, source: "github", operation: "set_labels", target: "ghost-mock/app#212",
      summary: "On ghost-mock/app#212: labels: add bug, priority-high.", autonomy: "quiet", status: "executed", created_at: ago(4.8),
      conversation: "accounting",
      events: [ev(4.8, "prepared", { source: "github", operation: "set_labels", target: "github:ghost-mock/app#212", autonomy: "quiet" }), ev(4.79, "claimed"), ev(4.78, "step_applied", { index: 0, step: "labels: add bug, priority-high" }), ev(4.77, "finished", { status: "success" })],
    },
  ];
  return [...recent, ...entries.sort((a, b) => b.created_at.localeCompare(a.created_at))].slice(0, count);
}
