import { type RefObject, useEffect, useId, useRef, useState } from "react";

import { Button } from "./Button";
import { cx } from "./cx";

export interface PendingChangeProps {
  /** What Ghost will do, in the gate's words (`pending.summary`). */
  summary: string;
  /** Why Ghost proposed it (`pending.rationale`). Optional. */
  rationale?: string | null;
  /** `ability.operation`, shown as mono detail (`github_issues.close`). */
  operation?: string;
  /** ISO time the proposal lapses (`pending.expires_at`). Never `expires_in_secs`: that is a snapshot. */
  expiresAt?: string | null;
  /** Must call `client.confirm()` and nothing else. The click is confirmation evidence (ADR 0013). */
  onConfirm: () => void;
  /** Must call `client.cancel()`. */
  onCancel: () => void;
  /** `true` disables both buttons; `"confirm"` or `"cancel"` also shows that button's spinner. */
  busy?: boolean | "confirm" | "cancel";
  /** Both buttons off, no spinner: Ghost isn’t answering (`useLive().online` is false). */
  disabled?: boolean;
  /** Tighter padding, 13px summary, two-line rationale, small buttons: the tray popover. */
  compact?: boolean;
  /**
   * Phone sizes: the summary at 16/24, the operation on its own mono line, the rationale
   * clamped to two lines, then Confirm and Cancel side by side as 56px full-width buttons.
   */
  touch?: boolean;
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

const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

/**
 * A change waiting for the operator's okay: Ghost's most consequential control. One
 * component for the session, the tray, mobile and the review queue, so there is one
 * countdown and one Confirm.
 *
 * Place it where it can't scroll away from its own buttons: in the session it lives in the
 * Page dock directly above the composer. In development it checks on load that whenever
 * its Confirm is on screen, its summary is fully on screen too (not under the compact page
 * bar, the dock or the panel edge), and logs a console error if not.
 */
export function PendingChange({ summary, rationale, operation, expiresAt, onConfirm, onCancel, busy = false, disabled = false, compact, touch, now, className }: PendingChangeProps) {
  const left = useSecondsLeft(expiresAt, now);
  const expired = left === 0;
  const urgent = left != null && left > 0 && left <= 10;
  const summaryId = useId();
  const root = useRef<HTMLElement>(null);
  const summaryRef = useRef<HTMLParagraphElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  // One spoken announcement at 10 seconds and one at expiry; the ticking number stays silent.
  const [said, setSaid] = useState("");
  useEffect(() => {
    if (expired) setSaid("Expired. Ghost left it alone.");
    else if (urgent && left === 10) setSaid("10 seconds left to confirm.");
  }, [expired, urgent, left]);

  useVisibilityCheck(root, summaryRef, confirmRef, summary);

  return (
    <section
      ref={root}
      className={cx("g-pending", compact && "g-pending--compact", touch && "g-pending--touch", expired && "is-expired", urgent && "is-urgent", className)}
      aria-labelledby={summaryId}
      aria-roledescription="change waiting for your okay"
    >
      <div className="g-pending__overline">
        <span className="g-pending__state">
          {expired ? (
            <>Expired · Ghost left it alone</>
          ) : (
            <>
              Waiting for your okay
              {left != null && (
                <>
                  {" · "}
                  <span className="g-pending__clock t-num" aria-live="off">{clock(left)}</span>
                </>
              )}
            </>
          )}
        </span>
        {operation && !compact && !touch && <span className="g-pending__operation">{operation}</span>}
      </div>
      <p ref={summaryRef} id={summaryId} className="g-pending__summary" data-user-content>{summary}</p>
      {/* Compact and touch cards are too narrow to share the overline: the id gets its own line, whole. */}
      {operation && (compact || touch) && <p className="g-pending__operation g-pending__operation--line">{operation}</p>}
      {rationale && <p className="g-pending__rationale" data-user-content>{rationale}</p>}
      <div className="g-pending__actions">
        <Button ref={confirmRef} variant="primary" size={compact ? "sm" : touch ? "lg" : "md"} fullWidth={touch} disabled={expired || disabled || (!!busy && busy !== "confirm")} loading={busy === "confirm"} onClick={onConfirm}>
          Confirm
        </Button>
        <Button variant="ghost" size={compact ? "sm" : touch ? "lg" : "md"} fullWidth={touch} disabled={expired || disabled || (!!busy && busy !== "cancel")} loading={busy === "cancel"} onClick={onCancel}>
          Cancel
        </Button>
      </div>
      <span className="sr-only" role="status">{said}</span>
    </section>
  );
}

/**
 * Development only: if Confirm is on screen at load, the summary must be too. Measured
 * against the page panel, the compact bar (opaque once the panel scrolls) and the Page dock
 * (when the card isn't in it). Logs a console error, which shot.mjs reports as a page error.
 */
function useVisibilityCheck(root: RefObject<HTMLElement | null>, summary: RefObject<HTMLElement | null>, confirm: RefObject<HTMLElement | null>, key: string) {
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    let cancelled = false;
    const t = window.setTimeout(() => {
      void document.fonts.ready.then(() => {
        const el = root.current, s = summary.current, c = confirm.current;
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
        if (cr.top < top || cr.bottom > bottom) return; // Confirm isn't on screen: nothing to assert.
        const sr = s.getBoundingClientRect();
        const ok = sr.top >= top - 0.5 && sr.bottom <= bottom + 0.5;
        el.dataset.visibility = ok ? "ok" : "hidden";
        if (!ok) console.error(`PendingChange: Confirm is on screen but the summary is not (summary ${Math.round(sr.top)}–${Math.round(sr.bottom)}, visible ${Math.round(top)}–${Math.round(bottom)}).`);
      });
    }, 700);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [root, summary, confirm, key]);
}
