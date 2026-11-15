/**
 * Today counts once (2026-11-14): the headline, the rail's badge and the cards Today shows (the
 * live strip's and Now's Decide and Answer) agree in every scenario with a change waiting, a
 * question, both, and a live session, and the session's own change is counted even before the
 * waiting list has caught up with it.
 */
import type { PendingProposal, QuestionView, Status, WaitingItemView, WaitingView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { MockDaemon } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";
import { cardsShown, liveNeeds, liveQuestion, needCount } from "../src/screens/today/live";
import { NO_TROUBLE, countsOf, headline, needsOf } from "../src/screens/today/model";

const q = new URLSearchParams();

/** What Today and the rail each read for a scenario, as the screens read it. */
async function read(name: string) {
  const s = scenarioByName(name);
  const d = new MockDaemon(s, composeFixtures(s), {});
  const status = (await d.handle("GET", "/status", q, null)) as Status;
  const waiting = (await d.handle("GET", "/waiting", q, null)) as WaitingView;
  const { questions } = (await d.handle("GET", "/questions", q, null)) as { questions: QuestionView[] };
  const inSession = !!status.session_id;
  const pending = inSession ? (status.pending ?? null) : null;
  const question = inSession ? liveQuestion(questions) : null;
  const needs = needsOf(waiting.items);
  return { needs, pending, question };
}

/** The headline's number, read back from its words. */
function headlineCount(needs: ReturnType<typeof needsOf>, pending: PendingProposal | null): number {
  const all = liveNeeds(needs, pending);
  const lead = headline({ ...countsOf(all, []), decide: all.decide.length, answer: all.answer.length }, true, NO_TROUBLE).lead;
  if (lead.startsWith("One thing")) return 1;
  const n = /^(\d+) things need you/.exec(lead);
  return n ? Number(n[1]) : 0;
}

describe("the headline, the rail's badge and the cards on Today agree", () => {
  for (const name of ["passive", "pending", "always", "always-live", "always-held", "asked", "expiring", "busy", "session", "thread"]) {
    it(`in the ${name} scenario`, async () => {
      const { needs, pending, question } = await read(name);
      // The rail counts `needCount` of the same inputs (app/Rail.tsx).
      const badge = needCount(needs, pending);
      const shown = cardsShown(needs, pending, question);
      expect(headlineCount(needs, pending)).toBe(badge);
      expect(shown.strip + shown.decide + shown.answer).toBe(badge);
      // A live change is on the strip, and only there.
      if (pending) expect(shown.strip).toBe(1);
    });
  }

  it("counts a change waiting in the session and one on the list as two, each shown once", async () => {
    const { needs, pending, question } = await read("pending");
    expect(pending).not.toBeNull();
    const shown = cardsShown(needs, pending, question);
    expect(shown).toMatchObject({ strip: 1, decide: 1 });
    expect(needCount(needs, pending)).toBe(2);
    expect(headlineCount(needs, pending)).toBe(2);
  });
});

describe("the session's change before the waiting list has it", () => {
  const pending = { id: "p1", source: "github", operation: "close", target: "app#1", summary: "On app#1: close the issue.", rationale: "", expires_at: "2026-11-14T10:01:00Z", expires_in_secs: 60 } as PendingProposal;
  const listed: WaitingItemView = { key: "change:p1", source: "gate", title: "Waiting for your yes: On app#1: close the issue.", state: "new", first_seen: "2026-11-14T10:00:00Z", changed_at: "2026-11-14T10:00:00Z", age_secs: 0, change: pending };
  const empty = { decide: [], answer: [], know: [] };
  it("is counted once whether or not the list has caught up", () => {
    expect(needCount(empty, pending)).toBe(1);
    expect(needCount({ ...empty, decide: [listed] }, pending)).toBe(1);
    expect(cardsShown(empty, pending, null)).toEqual({ strip: 1, decide: 0, answer: 0 });
    expect(cardsShown({ ...empty, decide: [listed] }, pending, null)).toEqual({ strip: 1, decide: 0, answer: 0 });
  });
  it("is nothing outside a session", () => {
    expect(needCount({ ...empty, decide: [listed] }, null)).toBe(1);
    expect(cardsShown({ ...empty, decide: [listed] }, null, null)).toEqual({ strip: 0, decide: 1, answer: 0 });
  });
  it("leaves a question the strip can't show in Answer when a change and a question both wait", () => {
    const question = { key: "question:q1" } as QuestionView;
    const asked = { key: "question:q1", source: "agent:accounting", title: "Send it?", state: "new", first_seen: "2026-11-14T10:00:00Z", changed_at: "2026-11-14T10:00:00Z", age_secs: 0, question } as WaitingItemView;
    const needs = { decide: [listed], answer: [asked], know: [] };
    expect(cardsShown(needs, pending, question)).toEqual({ strip: 1, decide: 0, answer: 1 });
    expect(needCount(needs, pending)).toBe(2);
  });
});
