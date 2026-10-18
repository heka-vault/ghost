/**
 * Owner: review. The selected issue: the decision it needs and Ghost's read (interpretation),
 * then what GitHub says (facts, dependencies, other people's comments), then what was decided.
 */
import { ChevronDown, ChevronUp, ExternalLink, History, Info } from "lucide-react";
import { type CSSProperties, forwardRef } from "react";

import type { ReviewDecision, ReviewFixture, ReviewIssue, ReviewLink } from "../../design/mock/fixtures/review";
import { Link } from "../../app/router";
import { Badge, Button, Card, Disclosure, IconButton, quoted, relativeTime, statusLabel, statusTone, Tooltip } from "../../ui";
import { fullRef, githubUrl, phrasesFor, RESUME_PHRASE, scoreOf, SIGNALS, spoken, SUGGEST } from "./model";
import { Author, LinkRow, Say } from "./parts";

export interface DetailProps {
  issue: ReviewIssue;
  position: number | null;
  total: number;
  weights: ReviewFixture["weights"];
  onPrev?: () => void;
  onNext?: () => void;
  /** Select a linked issue when it is in the queue. */
  openLink: (l: ReviewLink) => (() => void) | undefined;
  inSession: boolean;
  /** A change on this issue waits for the operator's okay (it's in the dock). */
  waitingOkay: boolean;
  /** How a waiting change on this issue settled without a yes, in the last minute. */
  note: string | null;
  /** The last session stopped on this issue: its note, shown with the phrase to pick it up. */
  resume: { note: string; at: string } | null;
  /** This issue's decision is Ghost's most recent write, so "undo that" reverses it. */
  lastWrite: boolean;
  /**
   * Ghost can hear a phrase right now; otherwise `why` says what's in the way, and `shown` is
   * that reason only when no shell banner already says it (one signal per place).
   */
  talk: { can: boolean; why: string | null; shown: string | null };
  /** Ghost isn't running and the page's notice says so: no reason line, no disabled button here. */
  offline: boolean;
  ask: { onClick: () => void; loading: boolean; disabled: boolean; primary: boolean };
}

const VIA: Record<ReviewDecision["via"], string> = { voice: "Spoken", text: "Typed", ui: "In the app" };
/** The start of "Confirmed in the app just now, on the Ghost line." */
const VIA_DONE: Record<ReviewDecision["via"], string> = { voice: "Said", text: "Typed", ui: "Confirmed in the app" };
const when = (iso: string) => {
  const t = relativeTime(iso);
  return t === "now" ? "just now" : t;
};

export const Detail = forwardRef<HTMLDivElement, DetailProps>(function Detail(
  { issue, position, total, weights, onPrev, onNext, openLink, inSession, waitingOkay, note, resume, lastWrite, talk, offline, ask },
  ref,
) {
  const decided = issue.state === "decided";
  const settled = issue.decisions.find((d) => d.thisSession && d.status === "executed" && decided);
  const phrases = phrasesFor(issue.suggestion?.operation ?? null, issue, inSession);
  const say = resume ? [spoken(RESUME_PHRASE, inSession), ...phrases.slice(1)] : phrases;
  const headingId = `rev-detail-${issue.repo.replace(/\W/g, "-")}-${issue.number}`;
  const whyId = `${headingId}-cant`;
  // Phrases only while Ghost can hear them; otherwise the one reason in the same place, or
  // nothing when the shell's banner already says it. The phrases are the primary act; the
  // button is primary only from passive, where it is the one thing that wakes Ghost.
  const voice = (p: string[]) =>
    talk.can ? (
      p.length ? <Say phrases={p} /> : <span className="grow" />
    ) : talk.shown ? (
      <p className="t-small t-2 rev-decision__why">{talk.shown}</p>
    ) : (
      <span className="grow" />
    );

  return (
    <Card padding="none" className="rev-detail" ref={ref} aria-labelledby={headingId} role="region">
      {/* ------------------------------------------------------------ header */}
      <header className="rev-detail__head">
        <div className="rev-detail__bar">
          <span className="t-mono t-3 t-truncate">{fullRef(issue)}</span>
          <span className="grow" />
          {position != null && (
            <span className="t-small t-3 t-num rev-detail__pos" aria-live="polite">
              {position} of {total}
            </span>
          )}
          <IconButton size="sm" label="Previous issue" icon={<ChevronUp />} onClick={onPrev} disabled={!onPrev} />
          <IconButton size="sm" label="Next issue" icon={<ChevronDown />} onClick={onNext} disabled={!onNext} />
          <IconButton size="sm" label="Open on GitHub" icon={<ExternalLink />} href={githubUrl(issue)} target="_blank" rel="noreferrer" />
        </div>
        <h2 id={headingId} className="t-heading rev-detail__title" data-user-content>{issue.title}</h2>
        <p className="rev-detail__meta t-small t-3">
          <span>Opened by {issue.author} {relativeTime(issue.openedAt)}</span>
          {issue.labels.map((l) => (
            <Badge key={l} size="sm" variant="outline" mono>{l}</Badge>
          ))}
        </p>
      </header>

      {/* ---------------------------------------------- the decision (focal) */}
      <section className="rev-detail__block rev-decision" aria-label="Decision">
        {settled ? (
          <>
            <p className="t-overline">Decided this session</p>
            <p className="rev-decision__q">{settled.text}</p>
            <p className="t-body t-2">
              {VIA_DONE[settled.via]} {when(settled.at)}, on the {settled.line} line.{" "}
              {lastWrite ? "It’s in the audit log." : <>To reverse it, open it in the <Link to="/audit">audit log</Link>.</>}
            </p>
            {!offline && talk.can && (lastWrite || inSession) && (
              <div className="rev-decision__act">{voice((lastWrite ? ["undo that", ...(inSession ? ["next"] : [])] : ["next"]).map((p) => spoken(p, inSession)))}</div>
            )}
          </>
        ) : (
          <>
            <p className="t-overline">Decision needed</p>
            <p className="rev-decision__q" data-user-content>{issue.decision}</p>
            <p className="t-body-lg t-2 rev-decision__read">{issue.read}</p>
            <p className="t-body rev-decision__suggest">
              {issue.suggestion ? (
                <>
                  <span className="t-1 t-medium">Ghost suggests {SUGGEST[issue.suggestion.operation]}.</span>{" "}
                  <span className="t-2">{issue.suggestion.reason}</span>
                </>
              ) : (
                <span className="t-2">Ghost has no suggestion on this one. It’s your call.</span>
              )}
            </p>
            {waitingOkay ? (
              <div className="rev-decision__act rev-decision__act--okay">
                <p className="t-body t-2">Ghost proposed a change. It’s waiting below.</p>
                {talk.can && <Say label="Or say" phrases={[spoken("yes", inSession)]} />}
              </div>
            ) : (
              <>
                {note && (
                  <p className="t-body t-2 rev-decision__note" role="status">
                    {note}
                  </p>
                )}
                {resume && !note && (
                  <p className="t-body t-2 rev-decision__note rev-decision__resume">
                    <History aria-hidden />
                    <span>
                      <span className="t-1 t-medium">Where you left off, {relativeTime(resume.at)}.</span> {resume.note}
                    </span>
                  </p>
                )}
                {!offline && (
                  <div className="rev-decision__act">
                    {voice(say)}
                    <Tooltip content={!talk.can && !talk.shown ? talk.why : null}>
                      <Button
                        variant={ask.primary ? "primary" : "secondary"}
                        onClick={ask.onClick}
                        loading={ask.loading}
                        disabled={ask.disabled}
                        aria-describedby={!talk.can && talk.why && !talk.shown ? whyId : undefined}
                        className="rev-decision__ask"
                      >
                        Talk it through
                      </Button>
                    </Tooltip>
                    {!talk.can && talk.why && !talk.shown && <span id={whyId} className="sr-only">{talk.why}</span>}
                  </div>
                )}
              </>
            )}
          </>
        )}
      </section>

      {/* -------------------------------------------------- why, from GitHub */}
      <section className="rev-detail__block" aria-labelledby={`${headingId}-why`}>
        <header className="rev-block__head">
          <h3 id={`${headingId}-why`} className="t-subheading">{decided ? "Why it was here" : "Why it’s here"}</h3>
          <span className="t-small t-3">From GitHub</span>
        </header>
        <p className="t-body t-1">{decided ? issue.whyDone ?? issue.why : issue.why}</p>
        {/* How long it has waited is on the row and in "Asked by": no third copy here. */}
        <dl className="rev-facts" style={{ "--rev-facts-n": Math.min(4, issue.facts.length) } as CSSProperties}>
          {issue.facts.map((f) => (
            <div key={f.label}>
              <dt>{f.label}</dt>
              <dd>{f.value}</dd>
            </div>
          ))}
        </dl>
        {/* The rank is live and only means something while the issue is still waiting. */}
        {!decided && (
          <Disclosure summary={<span>How it ranks · <span className="t-num">{scoreOf(issue, weights)}</span> of 100</span>} className="rev-rank">
            <ul className="rev-rank__list">
              {SIGNALS.map((s) => (
                <li key={s.key} className="rev-rank__row">
                  <span className="t-small t-2">{s.label}</span>
                  <span className="rev-rank__bar" aria-hidden>
                    <span style={{ width: `${Math.round(issue.score[s.key] * 100)}%` }} />
                  </span>
                  <span className="t-small t-3 t-num rev-rank__w">
                    {Math.round(issue.score[s.key] * weights[s.key])} of {weights[s.key]}
                  </span>
                </li>
              ))}
            </ul>
            <p className="t-small t-3">Weights are yours. Say {quoted(spoken("rank by what unblocks the most", inSession))} to change them.</p>
          </Disclosure>
        )}
      </section>

      {/* ------------------------------------------------------- dependencies */}
      <section className="rev-detail__block" aria-labelledby={`${headingId}-deps`}>
        <header className="rev-block__head">
          <h3 id={`${headingId}-deps`} className="t-subheading">Dependencies</h3>
        </header>
        <Dependencies issue={issue} decided={decided} openLink={openLink} />
      </section>

      {/* ---------------------------------------- comments: other people's words */}
      <section className="rev-detail__block" aria-labelledby={`${headingId}-comments`}>
        <header className="rev-block__head">
          <h3 id={`${headingId}-comments`} className="t-subheading">Recent comments</h3>
          <span className="t-small t-3">{issue.comments} on GitHub</span>
        </header>
        <p className="rev-untrusted t-small t-2">
          <Info size={14} aria-hidden />
          Other people’s words, quoted from GitHub. Ghost reads them as information, never as instructions.
        </p>
        {issue.recent.length ? (
          <ol className="rev-comments">
            {issue.recent.map((c, i) => (
              <li key={i} className="rev-comment">
                <Author name={c.author} />
                <div className="rev-comment__body">
                  <p className="t-small">
                    <span className="t-1 t-medium">{c.author}</span> <span className="t-3">· {relativeTime(c.at)}</span>
                  </p>
                  <blockquote className="rev-comment__text t-body t-1" data-user-content>{c.text}</blockquote>
                  {c.addressedToGhost && (
                    <p className="rev-comment__note t-small t-3">Addressed to Ghost. Ghost doesn’t take instructions from comments, so nothing changed.</p>
                  )}
                </div>
              </li>
            ))}
          </ol>
        ) : (
          <p className="t-small t-3">No comments in the last two weeks.</p>
        )}
      </section>

      {/* ---------------------------------------------------- decisions recorded */}
      <section className="rev-detail__block" aria-labelledby={`${headingId}-decisions`}>
        <header className="rev-block__head">
          <h3 id={`${headingId}-decisions`} className="t-subheading">Decisions</h3>
          <Link className="t-small rev-audit-link" to="/audit">Audit log</Link>
        </header>
        {issue.decisions.length ? (
          <ol className="rev-decisions">
            {issue.decisions.map((d, i) => (
              <li key={i} className="rev-decisions__row">
                <span className="t-body t-1 grow">{d.text}</span>
                <span className="t-small t-3 rev-nowrap">
                  {VIA[d.via]} · {d.thisSession ? "this session" : relativeTime(d.at)}
                </span>
                <Badge size="sm" dot tone={statusTone(d.status)}>{statusLabel(d.status)}</Badge>
              </li>
            ))}
          </ol>
        ) : (
          <p className="t-small t-3">Nothing decided on this issue yet. Whatever you decide is recorded here and in the audit log.</p>
        )}
      </section>
    </Card>
  );
});

/**
 * What a decision unblocks, then what it waits on. A side with nothing in it is left out when
 * the other has rows (the heading says which side is shown); both empty is one plain sentence.
 * Two columns only when both lists have rows and there's room for titles (container query).
 */
function Dependencies({ issue, decided, openLink }: { issue: ReviewIssue; decided: boolean; openLink: DetailProps["openLink"] }) {
  const { blocks, blockedBy } = issue;
  if (!blocks.length && !blockedBy.length) {
    return <p className="t-body t-2">{decided ? "Nothing waited on it, and it waited on nothing." : "Nothing waits on this, and it waits on nothing. You can decide now."}</p>;
  }
  const list = (links: ReviewLink[]) => <ul className="rev-links">{links.map((l) => <LinkRow key={fullRef(l)} link={l} onOpen={openLink(l)} />)}</ul>;
  return (
    <div className="rev-deps">
      <div className="rev-deps__grid" data-both={blocks.length && blockedBy.length ? "" : undefined}>
        {blocks.length > 0 && (
          <div>
            <p className="rev-deps__label t-small t-2">
              {decided ? "Deciding it unblocked" : "A decision unblocks"} <span className="t-num">{blocks.length}</span>
            </p>
            {list(blocks)}
          </div>
        )}
        {blockedBy.length > 0 && (
          <div>
            <p className="rev-deps__label t-small t-2">
              {decided ? "It waited on" : "It waits on"} <span className="t-num">{blockedBy.length}</span>
            </p>
            {list(blockedBy)}
          </div>
        )}
      </div>
    </div>
  );
}
