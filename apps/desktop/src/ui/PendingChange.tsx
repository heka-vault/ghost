import type { AlwaysChoice, AlwaysOffer } from "@ghost/client-ts";
import { permissionLabel } from "@ghost/ui";
import { ChevronDown } from "lucide-react";
import { type RefObject, useEffect, useId, useRef, useState } from "react";

import { DETAILS, NO, SAY_IT, YES, alwaysWords, clockWords, faceOf, ignoreEnter, plainSummary, shortTarget } from "./always";
import { Button } from "./Button";
import { cx } from "./cx";
import { formatTime } from "./format";

export interface PendingChangeProps {
  /** What Ghost will do, in the gate's words (`pending.summary`): the card's one sentence. */
  summary: string;
  /** Why Ghost proposed it (`pending.rationale`). Behind Details. */
  rationale?: string | null;
  /** `source.operation` (`github.close`), behind Details in mono. Never on the face. */
  operation?: string;
  /** What it acts on (`pending.target`), behind Details in mono. */
  target?: string | null;
  /** ISO time the proposal lapses (`pending.expires_at`). Never `expires_in_secs`: that is a snapshot. */
  expiresAt?: string | null;
  /** Must call `client.confirm()` (or the gate's confirm by id) and nothing else. The click is confirmation evidence (ADR 0013). */
  onConfirm: () => void;
  /** Must call `client.cancel()` (or the gate's cancel by id). */
  onCancel: () => void;
  /** `true` disables the answers; `"confirm"` or `"cancel"` also shows that button's spinner. */
  busy?: boolean | "confirm" | "cancel";
  /** Every answer off, no spinner: Ghost isn’t answering (`useLive().online` is false). */
  disabled?: boolean;
  /**
   * More than one change waits on screen: the face adds the target's short name (“app#17”) after
   * the sentence, so two “Close the issue.” cards can be told apart. Otherwise it is under Details.
   */
  showTarget?: boolean;
  /** A voice session is live and this is its change: the face adds “or say it”. */
  sayable?: boolean;
  /** Tighter padding, a smaller sentence, small buttons: the live strip and the tray. */
  compact?: boolean;
  /** Phone sizes: the sentence at 16/24, then Yes and No side by side as 56px buttons. No sticky choice. */
  touch?: boolean;
  /**
   * What the daemon offers to make the decision stick (`change.always`, ADR 0070): “Always” and
   * “Never” for this action, quiet secondary buttons on the card itself, worded from the tool's
   * name and the app's key (`alwaysWords`). Never the default, and Enter never chooses one.
   * Shown only with `onAlways`, and never on `touch`.
   */
  always?: AlwaysOffer | null;
  /** Must call `client.decideAlways(id, choice)` and nothing else: the click is the operator's own. */
  onAlways?: (choice: AlwaysChoice) => void;
  /** Start with Details open. Gallery specimens and screenshots only. */
  detailsOpen?: boolean;
  /** Pin the clock to this epoch-ms time. Gallery specimens and screenshots only; omit in the app. */
  now?: number;
  className?: string;
}

/** Seconds left, re-read from the wall clock every second. null when there is no deadline. */
function useSecondsLeft(expiresAt: string | null | undefined, pinned?: number): number | null {
  const deadline = expiresAt ? Date.parse(expiresAt) : NaN;
  const read = () => (Number.isFinite(deadline) ? Math.max(0, Math.ceil((deadline - (pinned ?? Date.now())) / 1000)) : null);
  const [left, setLeft] = useState(read);
  useEffect(() => {
    setLeft(read());
    if (pinned != null || !Number.isFinite(deadline)) return;
    const h = window.setInterval(() => setLeft(read()), 1000);
    return () => window.clearInterval(h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deadline, pinned]);
  return left;
}

/**
 * A change waiting for the operator's yes: Ghost's most consequential control, and one shape
 * everywhere the window asks (2026-11-14, ADR 0070 as amended): the sentence of what will
 * happen, Yes and No, “or say it” where a voice session is live, and on the window's own
 * cards “Always” and “Never” for this action as quiet secondary buttons. At most 25 words on
 * its face (`faceOf`). The tool, the level, the time and the target are behind Details; the
 * clock shows on the face only in the wait's last minute.
 *
 * Place it where it can't scroll away from its own buttons: in the session it lives in the
 * Page dock directly above the composer. In development it checks on load that whenever its
 * Yes is on screen, its sentence is fully on screen too, and logs a console error if not.
 */
export function PendingChange({ summary, rationale, operation, target, expiresAt, onConfirm, onCancel, busy = false, disabled = false, sayable = false, compact, touch, always, onAlways, detailsOpen = false, now, className, showTarget = false }: PendingChangeProps) {
  const left = useSecondsLeft(expiresAt, now);
  const expired = left === 0;
  const urgent = left != null && left > 0 && left <= 10;
  const clock = clockWords(left);
  const summaryId = useId();
  const root = useRef<HTMLElement>(null);
  const summaryRef = useRef<HTMLParagraphElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  // The sticky choice is the window's own cards' only: never the phone's touch card.
  const sticky = always && onAlways && !touch ? alwaysWords(always) : null;
  // The plain sentence on the face, the target under Details (or its short name, when asked).
  const plain = plainSummary(summary);
  const on = plain.on ?? target ?? null;
  const tag = showTarget && on ? shortTarget(on) : null;
  const face = faceOf(plain.what, { sayable, clock, sticky, tag });
  const off = expired || disabled || !!busy;
  const size = compact ? "sm" : touch ? "lg" : "md";

  // One spoken announcement at 10 seconds and one at expiry; the ticking number stays silent.
  const [said, setSaid] = useState("");
  useEffect(() => {
    if (expired) setSaid("Too late. Ghost left it alone.");
    else if (urgent && left === 10) setSaid("10 seconds left to answer.");
  }, [expired, urgent, left]);

  useVisibilityCheck(root, summaryRef, confirmRef, summary);

  return (
    <section
      ref={root}
      className={cx("g-pending", compact && "g-pending--compact", touch && "g-pending--touch", expired && "is-expired", urgent && "is-urgent", className)}
      aria-labelledby={summaryId}
      aria-roledescription="change waiting for your yes"
    >
      {expired && <p className="g-pending__state">Too late · Ghost left it alone</p>}
      <p ref={summaryRef} id={summaryId} className="g-pending__summary" aria-label={face.cut || plain.on ? summary : undefined}>
        <span data-user-content>{face.summary}</span>
        {tag && (
          <span className="g-pending__tag">
            {" · "}
            {tag}
          </span>
        )}
      </p>
      <div className="g-pending__actions">
        <Button ref={confirmRef} variant="primary" size={size} fullWidth={touch} disabled={expired || disabled || (!!busy && busy !== "confirm")} loading={busy === "confirm"} onClick={onConfirm}>
          {YES}
        </Button>
        <Button variant="secondary" size={size} fullWidth={touch} disabled={expired || disabled || (!!busy && busy !== "cancel")} loading={busy === "cancel"} onClick={onCancel}>
          {NO}
        </Button>
        {(sayable || clock) && !touch && (
          <span className="g-pending__aside">
            {sayable && !expired && <span>{SAY_IT}</span>}
            {clock && (
              <span className="g-pending__clock t-num" aria-live="off">
                {clock}
              </span>
            )}
          </span>
        )}
      </div>
      {sticky && (
        <div className="g-pending__always" role="group" aria-label="Make this choice stick">
          <span className="g-pending__label" data-user-content>
            {sticky.label}
          </span>
          {sticky.allow ? (
            <Button variant="ghost" size="sm" disabled={off} aria-label={sticky.always} onKeyDown={ignoreEnter} onClick={() => onAlways!("allow")}>
              Always allow
            </Button>
          ) : (
            <span className="g-pending__held" title={sticky.heldWhy ?? undefined}>
              {sticky.held}
            </span>
          )}
          <Button variant="danger-ghost" size="sm" disabled={off} aria-label={sticky.never} onKeyDown={ignoreEnter} onClick={() => onAlways!("block")}>
            Never allow
          </Button>
        </div>
      )}
      {/* A native disclosure: closed, its identifiers are not on the face (the plain-words guard reads it so too). */}
      {!touch && (
        <details className="g-pending__more" open={detailsOpen || undefined}>
          <summary className="g-pending__toggle">
            {DETAILS}
            <ChevronDown size={14} aria-hidden className="g-pending__chevron" />
          </summary>
        <dl className="g-pending__details">
          {(face.cut || plain.on) && (
            <div>
              <dt>In full</dt>
              <dd data-user-content>{summary}</dd>
            </div>
          )}
          {rationale && (
            <div>
              <dt>Why</dt>
              <dd data-user-content>{rationale}</dd>
            </div>
          )}
          {operation && (
            <div>
              <dt>Tool</dt>
              <dd className="t-mono">{operation}</dd>
            </div>
          )}
          {on && (
            <div>
              <dt>On</dt>
              <dd className="t-mono" data-user-content>
                {on}
              </dd>
            </div>
          )}
          <div>
            <dt>Your rule</dt>
            <dd>{permissionLabel("ask")}</dd>
          </div>
          {sticky?.heldWhy && (
            <div>
              <dt>Always</dt>
              <dd>{sticky.heldWhy}</dd>
            </div>
          )}
          {expiresAt && !expired && (
            <div>
              <dt>Waits until</dt>
              <dd>{formatTime(expiresAt)}</dd>
            </div>
          )}
        </dl>
        </details>
      )}
      <span className="sr-only" role="status">
        {said}
      </span>
    </section>
  );
}

/**
 * Development only: if Yes is on screen at load, the sentence must be too. Measured against
 * the page panel, the compact bar (opaque once the panel scrolls) and the Page dock (when the
 * card isn't in it). Logs a console error, which shot.mjs reports as a page error.
 */
function useVisibilityCheck(root: RefObject<HTMLElement | null>, summary: RefObject<HTMLElement | null>, confirm: RefObject<HTMLElement | null>, key: string) {
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    let cancelled = false;
    const t = window.setTimeout(() => {
      void document.fonts.ready.then(() => {
        const el = root.current,
          s = summary.current,
          c = confirm.current;
        if (cancelled || !el || !s || !c) return;
        const panel = el.closest<HTMLElement>(".shell__main");
        const p = panel?.getBoundingClientRect() ?? { top: 0, bottom: window.innerHeight };
        let top = p.top;
        let bottom = p.bottom;
        const bar = panel?.classList.contains("is-scrolled") ? panel.querySelector<HTMLElement>(".g-page-bar") : null;
        if (bar) top = Math.max(top, bar.getBoundingClientRect().bottom);
        const dock = panel?.querySelector<HTMLElement>(".g-page__dock");
        if (dock && !dock.contains(el)) bottom = Math.min(bottom, dock.getBoundingClientRect().top);
        const cr = c.getBoundingClientRect();
        if (cr.top < top || cr.bottom > bottom) return; // Yes isn't on screen: nothing to assert.
        const sr = s.getBoundingClientRect();
        const ok = sr.top >= top - 0.5 && sr.bottom <= bottom + 0.5;
        el.dataset.visibility = ok ? "ok" : "hidden";
        if (!ok) console.error(`PendingChange: Yes is on screen but the sentence is not (sentence ${Math.round(sr.top)}–${Math.round(sr.bottom)}, visible ${Math.round(top)}–${Math.round(bottom)}).`);
      });
    }, 700);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [root, summary, confirm, key]);
}
