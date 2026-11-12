import type { QuestionView, WaitingItemView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { canSend, expiresIn, sourceLabel } from "../src/screens/waiting";

const question = (over: Partial<QuestionView> = {}): QuestionView => ({
  key: "question:abc",
  kind: "question",
  asked_by: "Ledger",
  source: "agent:ledger",
  question: "Should the October invoice go out today?",
  choices: ["Yes", "No"],
  choices_only: false,
  urgency: "notify",
  asked_at: "2026-11-11T10:00:00Z",
  expires_at: "2026-11-11T13:00:00Z",
  state: "open",
  ...over,
});

const item = (q: QuestionView): WaitingItemView => ({
  key: q.key,
  source: q.source,
  title: `${q.asked_by} asks: ${q.question}`,
  state: "new",
  first_seen: q.asked_at,
  changed_at: q.asked_at,
  age_secs: 0,
  question: q,
});

describe("a question on the waiting list (ADR 0051)", () => {
  it("says who asked and when it stops waiting", () => {
    const q = question();
    expect(sourceLabel(item(q))).toBe("Asked by Ledger");
    expect(sourceLabel(item(question({ asked_by: "Claude Code", source: "client:claude-code" })))).toBe("Asked by Claude Code");
    const now = new Date("2026-11-11T10:00:00Z").getTime();
    expect(expiresIn(q, now)).toBe("expires in 3 h");
    expect(expiresIn(question({ expires_at: "2026-11-11T10:20:00Z" }), now)).toBe("expires in 20 min");
    expect(expiresIn(question({ expires_at: "2026-11-11T09:00:00Z" }), now)).toBe("expiring now");
  });

  it("sends the operator's own words, or only a choice when only the choices will do", () => {
    expect(canSend(question(), "  ")).toBe(false);
    expect(canSend(question(), "yes, but tomorrow")).toBe(true);
    const strict = question({ choices_only: true });
    expect(canSend(strict, "yes, but tomorrow")).toBe(false);
    expect(canSend(strict, "no")).toBe(true);
  });
});
