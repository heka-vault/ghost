/**
 * Owner: today. A live session as Today shows it (2026-11-14: the in-window room is gone, so
 * a session where Ghost doesn't float is seen on the desk). Pure, so the words are tested
 * without a browser. The talk bar says what Ghost is doing in the stage's own words (the
 * conversation screen's `stageWords`: “Listening to you”, “Using GitHub and Calendar”, “Ghost
 * is speaking”); a change waiting for the operator's yes, or else a question asked in this
 * session, sits right above it with its buttons, and comes out of Now's lists while it does, so
 * nothing shows twice on one page.
 *
 * Counted once (2026-11-14): the headline, the rail's badge and what Today shows all count
 * `liveNeeds`, the waiting list with the session's own change in it even before the list has
 * caught up with it, so a yes the strip shows is never missing from the count, nor counted twice.
 */
import type { PendingProposal, QuestionView, WaitingItemView } from "@ghost/client-ts";

type NeedsLike = { decide: WaitingItemView[]; answer: WaitingItemView[]; know: WaitingItemView[] };

/** The open question the strip offers: the newest one asked in this session that is still open. */
export function liveQuestion(questions: readonly QuestionView[]): QuestionView | null {
  const open = questions.filter((q) => q.kind === "question" && q.state === "open");
  return open.length ? open[open.length - 1] : null;
}

/**
 * The needs with the live session's change in Decide: the daemon lists every change waiting for
 * a yes (`change:<id>`, ADR 0030), but the list is read again only after an event, so for a
 * moment the strip can show a change the list doesn't have. It is the same item either way.
 */
export function liveNeeds<T extends NeedsLike>(needs: T, pending: PendingProposal | null, at: string = new Date().toISOString()): T {
  if (!pending || needs.decide.some((i) => i.change?.id === pending.id)) return needs;
  const item: WaitingItemView = { key: `change:${pending.id}`, source: "gate", title: `Waiting for your yes: ${pending.summary}`, state: "new", first_seen: at, changed_at: at, age_secs: 0, change: pending };
  return { ...needs, decide: [item, ...needs.decide] };
}

/** What the strip shows: the change waiting in the session, else the question. Never both. */
export function stripShows(pending: Pick<PendingProposal, "id"> | null, question: Pick<QuestionView, "key"> | null): { pending: Pick<PendingProposal, "id"> | null; question: Pick<QuestionView, "key"> | null } {
  return pending ? { pending, question: null } : { pending: null, question };
}

/**
 * Now's needs without what the live strip already shows: the change waiting in the session (by
 * its id), or the question the strip offers (by its key) when no change is waiting. A question
 * the strip doesn't show stays in Answer.
 */
export function withoutLive<T extends NeedsLike>(needs: T, pending: Pick<PendingProposal, "id"> | null, question: Pick<QuestionView, "key"> | null): T {
  const shown = stripShows(pending, question);
  if (!shown.pending && !shown.question) return needs;
  return {
    ...needs,
    decide: shown.pending ? needs.decide.filter((i) => i.change?.id !== shown.pending!.id) : needs.decide,
    answer: shown.question ? needs.answer.filter((i) => i.question?.key !== shown.question!.key && i.key !== shown.question!.key) : needs.answer,
  };
}

/** How many things need the operator: one number for the headline and the rail's badge. */
export function needCount(needs: NeedsLike, pending: PendingProposal | null): number {
  const all = liveNeeds(needs, pending);
  return all.decide.length + all.answer.length;
}

/**
 * The cards Today shows for what needs the operator: the strip's one, and Now's Decide and
 * Answer. Equal to `needCount` whenever the strip's question is on the waiting list, as the
 * daemon always puts it there (ADR 0051).
 */
export function cardsShown(needs: NeedsLike, pending: PendingProposal | null, question: QuestionView | null): { strip: number; decide: number; answer: number } {
  const all = liveNeeds(needs, pending);
  const rest = withoutLive(all, pending, question);
  return { strip: pending || question ? 1 : 0, decide: rest.decide.length, answer: rest.answer.length };
}
