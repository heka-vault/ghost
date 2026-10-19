/** Owner: audit. One write: a summary row that opens onto its event timeline. */
import type { AuditEntry } from "@ghost/client-ts";
import { ArrowUpRight, ChevronRight } from "lucide-react";
import { type FocusEventHandler, Fragment, forwardRef, useId } from "react";

import { hrefFor } from "../../app/router";
import { Badge, Button, Disclosure, Notice, PendingChange, cx, errorText, formatDateTime, formatTime } from "../../ui";
import { ATTENTION, describeEvent, levelParts, opName, outcomeLabel, outcomeTone, splitSummary } from "./model";

export interface AuditRowProps {
  entry: AuditEntry;
  open: boolean;
  onToggle: () => void;
  /** Just arrived with a live refresh: tinted for a moment. */
  fresh?: boolean;
  /**
   * This row is the change waiting for the operator's okay right now: PendingChange renders
   * under its timeline with the same client calls the session uses (a Confirm click here is
   * confirmation evidence, ADR 0013).
   */
  live?: LiveChange | null;
  /** Roving tabindex: 0 for the list's one Tab stop, -1 for the rest. */
  tabIndex?: number;
  onFocus?: FocusEventHandler<HTMLButtonElement>;
  /** The search, to mark its terms in the summary. */
  highlight?: string;
  /** The outcome to show, from outcomeOf() with what the page knows (the live id, the time). */
  outcome: string;
}

export interface LiveChange {
  summary: string;
  rationale?: string | null;
  operation: string;
  expiresAt: string | null;
  /** Calls client.confirm() and nothing else. */
  onConfirm: () => void;
  /** Calls client.cancel(). */
  onCancel: () => void;
  busy: boolean | "confirm" | "cancel";
  /** Ghost isn't answering: both buttons off. */
  disabled: boolean;
  /** The last Confirm or Cancel failed: shown under the card. */
  error?: unknown;
}

/** Proposed, but nothing was written: the words step back (see .is-notdone). */
const NOT_DONE = new Set(["cancelled", "expired", "lapsed", "failed", "stale"]);

export const AuditRow = forwardRef<HTMLButtonElement, AuditRowProps>(function AuditRow({ entry: e, open, onToggle, fresh, live, tabIndex, onFocus, highlight, outcome }, ref) {
  const panel = useId();
  const metaId = useId();
  const { subject, action } = splitSummary(e);
  // The accessible name in words, with the pauses the layout draws with space: "ghost-mock/app#208
  // approved, with a comment: …, Done, 4:52 PM". The meta line describes it.
  const name = [[subject, action].filter(Boolean).join(" "), outcomeLabel(outcome), formatTime(e.created_at)].join(", ");
  return (
    <div role="listitem" className={cx("audit-item", open && "is-open", fresh && "is-fresh", NOT_DONE.has(outcome) && "is-notdone")} data-audit-id={e.id}>
      <button
        ref={ref}
        type="button"
        className="audit-row"
        aria-label={name}
        aria-describedby={metaId}
        aria-expanded={open}
        aria-controls={open ? panel : undefined}
        onClick={onToggle}
        tabIndex={tabIndex}
        onFocus={onFocus}
        data-audit-row
      >
        <span className="audit-row__main">
          <span className="audit-row__summary" data-user-content>
            {subject && (
              <>
                <span className="audit-row__target">
                  <Marked text={subject} terms={highlight} />
                </span>
                <span className="sr-only"> </span>
              </>
            )}
            <span className="audit-row__action">
              <Marked text={action} terms={highlight} />
            </span>
          </span>
          <span className="audit-row__meta" id={metaId}>
            <span className="t-mono audit-row__op">{opName(e)}</span>
            <span aria-hidden className="audit-row__sep">·</span>
            {levelParts(e, outcome).map((part, i) => (
              <Fragment key={part}>
                {i > 0 && <span aria-hidden className="audit-row__sep">·</span>}
                <span>{part}</span>
              </Fragment>
            ))}
          </span>
        </span>
        <span className="audit-row__status">
          {ATTENTION.has(outcome) ? (
            <Badge tone={outcomeTone(outcome)} dot size="sm">{outcomeLabel(outcome)}</Badge>
          ) : (
            // Settled outcomes need nothing: a dot and a word, no fill, so the only pills on
            // the page are the ones that need the operator. Done gets the ok dot; Canceled
            // and Expired a quiet one.
            <span className={cx("audit-quiet", outcomeTone(outcome) === "ok" && "audit-quiet--ok")}>
              <span className="audit-quiet__dot" aria-hidden />
              {outcomeLabel(outcome)}
            </span>
          )}
        </span>
        <time className="audit-row__time t-num" dateTime={e.created_at} title={formatDateTime(e.created_at)}>
          {formatTime(e.created_at)}
        </time>
        <ChevronRight className="audit-row__chev" aria-hidden />
      </button>
      {open && (
        <div id={panel} className="audit-detail" role="region" aria-label={`What happened: ${opName(e)}`}>
          <ol className="audit-timeline">
            {e.events.map((ev, i) => {
              const v = describeEvent(ev, e);
              return (
                <li key={`${ev.at}-${ev.event}-${i}`} className={cx("audit-ev", v.tone && `audit-ev--${v.tone}`)}>
                  <time className="audit-ev__time t-num" dateTime={ev.at}>{timeWithSeconds(ev.at)}</time>
                  <span className="audit-ev__dot" aria-hidden />
                  <span className="audit-ev__body">
                    <span className="audit-ev__title">{v.title}</span>
                    {v.quote && <span className="audit-ev__quote" data-user-content>“{v.quote}”</span>}
                    {v.detail && <span className="audit-ev__detail t-mono">{v.detail}</span>}
                  </span>
                </li>
              );
            })}
            {/* The step still open. With the live card under the timeline, the card is the "now"
                step: the trail ends at "Ghost prepared it" and the card follows, said once. */}
            {!live && (outcome === "pending" || outcome === "claimed") && (
              <li className={cx("audit-ev", "audit-ev--now", outcome === "pending" && "audit-ev--warn")}>
                <span className="audit-ev__time">now</span>
                <span className="audit-ev__dot" aria-hidden />
                <span className="audit-ev__body">
                  <span className="audit-ev__title">{outcome === "pending" ? "Waiting for your okay" : "Writing…"}</span>
                  {outcome === "pending" && <span className="audit-ev__detail">Ghost leaves it alone if you don’t answer in time.</span>}
                </span>
              </li>
            )}
            {/* A stored "pending" with no expiry event: the gate never got to close it (Ghost
                restarted, or stopped, while it waited). Nothing was written; the trail says so. */}
            {outcome === "lapsed" && (
              <li className="audit-ev audit-ev--lapsed">
                <span className="audit-ev__time t-num">{timeWithSeconds(new Date(new Date(e.created_at).getTime() + 60_000).toISOString())}</span>
                <span className="audit-ev__dot" aria-hidden />
                <span className="audit-ev__body">
                  <span className="audit-ev__title">Expired · Ghost left it alone</span>
                  <span className="audit-ev__detail">No answer came before the minute ran out, and Ghost stopped before it could say so. Nothing was written.</span>
                </span>
              </li>
            )}
          </ol>
          {live && (
            <div className="audit-detail__live">
              {/* The session link sits on the card's own button line (see .audit-detail__live-aside), so
                  the card has one row of actions and nothing dangles under it. */}
              <div className="audit-detail__live-card">
                <PendingChange
                  compact
                  summary={live.summary}
                  rationale={live.rationale}
                  operation={live.operation}
                  expiresAt={live.expiresAt}
                  busy={live.busy}
                  disabled={live.disabled}
                  onConfirm={live.onConfirm}
                  onCancel={live.onCancel}
                />
                <div className="audit-detail__live-aside">
                  <Button size="sm" variant="ghost" href={hrefFor("/")} trailingIcon={<ArrowUpRight />}>
                    Answer in the session
                  </Button>
                </div>
              </div>
              {live.error != null && (
                <Notice tone="danger" title="Couldn’t send your answer">
                  {errorText(live.error)} The change is still waiting; try again, or answer in the session.
                </Notice>
              )}
            </div>
          )}
          <Disclosure
            className="audit-detail__raw"
            summary={
              <span className="audit-detail__ids">
                Raw record
                <span aria-hidden className="audit-row__sep">·</span>
                <span className="t-mono" title={e.id}>write {e.id.slice(0, 8)}</span>
                <span aria-hidden className="audit-row__sep">·</span>
                <span className="t-mono" title={e.session_id}>session {e.session_id.slice(0, 8)}</span>
              </span>
            }
          >
            <pre className="audit-raw t-mono">{JSON.stringify(e, null, 2)}</pre>
          </Disclosure>
        </div>
      )}
    </div>
  );
});

function timeWithSeconds(iso: string): string {
  return new Date(iso).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", second: "2-digit" });
}

/** Text with the search's terms marked. */
function Marked({ text, terms }: { text: string; terms?: string }) {
  const words = (terms ?? "").trim().split(/\s+/).filter((w) => w.length > 1);
  if (!text || !words.length) return <>{text}</>;
  const re = new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi");
  const parts = text.split(re);
  return (
    <>
      {parts.map((p, i) => (i % 2 ? <mark key={i} className="audit-mark">{p}</mark> : <Fragment key={i}>{p}</Fragment>))}
    </>
  );
}
