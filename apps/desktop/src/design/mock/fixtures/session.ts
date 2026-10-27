/**
 * Owner: session. The live session: transcript, the pending change, and how the mock
 * answers typed turns. Extend freely; keep the exported shape.
 */
import type { PendingProposal, Turn } from "@ghost/client-ts";

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
  /** Canned replies for typed turns: first rule whose pattern matches wins. */
  replies: { match: RegExp; reply: string; propose?: Omit<PendingProposal, "id" | "expires_at" | "expires_in_secs"> }[];
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

  return {
    sessionId,
    turns,
    pending,
    replies: [
      {
        match: /\bclose\b/i,
        reply: "Close that issue? Say yes, or press Confirm.",
        propose: {
          source: "github",
          operation: "close",
          target: "ghost-mock/app#163",
          summary: "On ghost-mock/app#163: close the issue.",
          rationale: "You asked to close it. Closing is set to ask first.",
        },
      },
      { match: /waiting|sign.?off|what('| i)s (new|up)/i, reply: "Three issues need your sign-off: app 212, app 208 and infra 77. Want the oldest first?" },
      { match: /label|tag/i, reply: "Done. Labeled it." },
      { match: /comment/i, reply: "Commented." },
      { match: /remember|pin/i, reply: "Pinned." },
      { match: /thank/i, reply: "Anytime." },
    ],
    fallbackReply: "On it. Nothing in the app or infra repos matches that yet.",
  };
}
