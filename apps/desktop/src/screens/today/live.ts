/**
 * Owner: today. A live session as Today shows it (2026-11-14: the in-window room is gone, so
 * a session where Ghost doesn't float is seen on the desk). Pure, so the words are tested
 * without a browser. The talk bar says what Ghost is doing in the stage's own words (the
 * conversation screen's `stageWords`: “Listening to you”, “Using GitHub and Calendar”, “Ghost
 * is speaking”); a change waiting for the operator's yes and a question asked in this session
 * sit right above it with their buttons, and come out of Now's lists while they do, so nothing
 * shows twice on one page.
 */
import type { PendingProposal, QuestionView, WaitingItemView } from "@ghost/client-ts";

/** The open question the strip offers: the newest one asked in this session that is still open. */
export function liveQuestion(questions: readonly QuestionView[]): QuestionView | null {
  const open = questions.filter((q) => q.kind === "question" && q.state === "open");
  return open.length ? open[open.length - 1] : null;
}

/**
 * Now's needs without what the live strip already shows: the change waiting in this session (by
 * its id) and the question the strip offers (by its key).
 */
export function withoutLive<T extends { decide: WaitingItemView[]; answer: WaitingItemView[]; know: WaitingItemView[] }>(needs: T, pending: Pick<PendingProposal, "id"> | null, question: Pick<QuestionView, "key"> | null): T {
  if (!pending && !question) return needs;
  return {
    ...needs,
    decide: pending ? needs.decide.filter((i) => i.change?.id !== pending.id) : needs.decide,
    answer: question ? needs.answer.filter((i) => i.question?.key !== question.key && i.key !== question.key) : needs.answer,
  };
}
