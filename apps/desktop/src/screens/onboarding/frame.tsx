/** Owner: onboarding. The first run's frame: the step rail, a step's header and footer, the hero presence. */
import { ArrowLeft, Check, Minus } from "lucide-react";
import { createContext, type FormEvent, type KeyboardEvent, type ReactNode, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { Button, cx, type GhostPhase, Skeleton, VoicePresence } from "../../ui";
import { type Progress, type StepDef, STEPS, type StepId, stepIndex, TASKS } from "./state";

/* -------------------------------------------------------------------------------- rail */

export interface RailProps {
  current: StepId;
  progress: Progress;
  /** The first answer from Ghost hasn't come: no count yet, a placeholder instead. */
  checking: boolean;
  onGo: (id: StepId) => void;
}

const NETWORK_NOTE: Record<StepDef["network"], string | null> = { none: null, maybe: null, needs: "Needs the internet" };

/**
 * Every step, in order, each one a button: setup resumes anywhere and nothing is locked.
 * One Tab stop (the current step); arrows, Home and End move between steps.
 */
export function Rail({ current, progress, checking, onGo }: RailProps) {
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const [focusAt, setFocusAt] = useState(stepIndex(current));
  useEffect(() => setFocusAt(stepIndex(current)), [current]);
  const unknown = TASKS.some((t) => progress.state[t.id] === "unknown");

  const move = (e: KeyboardEvent<HTMLOListElement>) => {
    const n = STEPS.length;
    const at = buttons.current.findIndex((b) => b === document.activeElement);
    if (at < 0) return;
    let to: number | null = null;
    if (e.key === "ArrowDown") to = (at + 1) % n;
    else if (e.key === "ArrowUp") to = (at - 1 + n) % n;
    else if (e.key === "Home") to = 0;
    else if (e.key === "End") to = n - 1;
    if (to === null) return;
    e.preventDefault();
    setFocusAt(to);
    buttons.current[to]?.focus();
  };

  return (
    <nav className="onb-rail" aria-label="Setup steps">
      <div className="onb-rail__head">
        <p className="t-heading">Set up Ghost</p>
        {checking ? (
          <Skeleton width={56} height={12} className="onb-rail__count-skel" />
        ) : (
          <p className="t-small t-2 t-num">
            {progress.done === progress.total ? "All done" : `${progress.done} of ${progress.total} done`}
            {unknown && " · start Ghost to check the rest"}
          </p>
        )}
        <div className="onb-rail__bar" aria-hidden>
          <span style={{ transform: `scaleX(${progress.done / progress.total})` }} />
        </div>
      </div>
      <ol className="onb-rail__list" onKeyDown={move}>
        {STEPS.map((s, i) => {
          const isCurrent = s.id === current;
          const st = progress.state[s.id];
          const isDone = s.task && st === "done";
          const isSkipped = s.task && st === "skipped";
          const note = isSkipped ? "Skipped" : !isDone ? NETWORK_NOTE[s.network] : null;
          return (
            <li key={s.id} className={cx("onb-rail__item", isDone && "is-done", isCurrent && "is-current")}>
              <button
                ref={(el) => {
                  buttons.current[i] = el;
                }}
                type="button"
                className="onb-rail__btn"
                tabIndex={i === focusAt ? 0 : -1}
                aria-current={isCurrent ? "step" : undefined}
                onClick={() => onGo(s.id)}
                onFocus={() => setFocusAt(i)}
              >
                <span className={cx("onb-mark", isDone && "is-done", isCurrent && "is-current", isSkipped && "is-skipped")} aria-hidden>
                  {isDone ? <Check size={12} strokeWidth={2.5} /> : isSkipped ? <Minus size={12} strokeWidth={2.5} /> : <span className="t-num">{i + 1}</span>}
                </span>
                <span className="onb-rail__text">
                  <span className="onb-rail__label">{s.label}</span>
                  {note && <span className="onb-rail__note">{note}</span>}
                </span>
                <span className="sr-only">{isDone ? ", done" : isSkipped ? ", skipped" : st === "unknown" ? ", can’t check yet" : ""}</span>
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

/* -------------------------------------------------------------------------- step frame */

/**
 * The Page dock's host element. The wizard hands it down; StepFrame renders its footer there,
 * so Back, Skip and the one primary stay in reach however long a step is. Null (no Page dock,
 * as in a specimen) keeps the footer inline.
 */
export const DockContext = createContext<HTMLElement | null>(null);

export interface StepFrameProps {
  id: StepId;
  title: ReactNode;
  description?: ReactNode;
  /** Above the title, e.g. the GhostMark on the welcome step. */
  lead?: ReactNode;
  children?: ReactNode;
  /** Persistent messages for this step (errors, results), right above the footer. */
  notices?: ReactNode;
  /** Primary action; Enter anywhere in the step does the same. Omit when the step's real action isn't a click (say “Hey Ghost”). */
  primary?: { label: string; onClick: () => void | Promise<void>; loading?: boolean; disabled?: boolean; icon?: ReactNode };
  /** A second action left of the primary (e.g. Cancel a download). */
  secondary?: ReactNode;
  onBack?: () => void;
  onSkip?: () => void;
  skipLabel?: string;
  /** A short line under the footer's actions (“Saves when Ghost is back”). */
  footNote?: ReactNode;
  /** Center the head and body: the hero moments (say “Hey Ghost”, you're set). */
  centered?: boolean;
  /** Focus the title when the step opens (not on first load). */
  autoFocus: boolean;
  /** A div, not a form: for a step that holds forms of its own (a password row), since forms can't nest. Enter then belongs to them. */
  plain?: boolean;
  className?: string;
}

/**
 * Room a docked footer needs beyond its own height: the dock's padding above and below it,
 * plus the step's own bottom padding. Measured against the panel, never the window.
 */
const DOCK_ALLOWANCE = 112;

export function StepFrame({ id, title, description, lead, children, notices, primary, secondary, onBack, onSkip, skipLabel = "Skip for now", footNote, centered, autoFocus, plain, className }: StepFrameProps) {
  const heading = useRef<HTMLHeadingElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const foot = useRef<HTMLDivElement>(null);
  const dockHost = useContext(DockContext);
  const formId = `onb-form-${id}`;

  useEffect(() => {
    if (autoFocus) heading.current?.focus({ preventScroll: true });
  }, [id, autoFocus]);

  // The footer is inline, a line or two under the content, while the step fits the panel;
  // it moves to the Page dock only when the step overflows, so Continue stays in reach on
  // the long steps and a short step never leaves an empty canvas between its last line and
  // its one action. Measured without the footer, so the answer can't flip itself.
  const [overflows, setOverflows] = useState(false);
  useLayoutEffect(() => {
    const el = form.current;
    const scroller = el?.closest(".shell__main") as HTMLElement | null;
    if (!dockHost || !el || !scroller) return;
    const measure = () => {
      const f = foot.current;
      const footH = f?.offsetHeight ?? 0;
      // The content's bottom edge: the last child that isn't the footer. Not the form's height,
      // which includes the inline footer and its margin and would flip the answer back and forth.
      const formTop = el.getBoundingClientRect().top;
      let body = 0;
      for (const child of Array.from(el.children)) {
        if (child === f) continue;
        body = Math.max(body, child.getBoundingClientRect().bottom - formTop);
      }
      const top = formTop - scroller.getBoundingClientRect().top + scroller.scrollTop;
      setOverflows(top + body + footH + DOCK_ALLOWANCE > scroller.clientHeight);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    ro.observe(scroller);
    return () => ro.disconnect();
  }, [dockHost, id]);
  const dock = overflows ? dockHost : null;

  const Root = (plain ? "div" : "form") as "form";
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!primary || primary.loading || primary.disabled) return;
    void primary.onClick();
  };

  // The footer belongs to the form (Enter submits, the primary is its submit button) but
  // lives in the Page dock, so the button is tied back to the form by id.
  const footer = (
    <div ref={foot} className={cx("onb-step__end", centered && "onb-step--centered")}>
      <footer className="onb-step__foot">
        {onBack && (
          <Button variant="ghost" leadingIcon={<ArrowLeft />} onClick={onBack} className="onb-step__back">
            Back
          </Button>
        )}
        <div className="onb-step__actions">
          {onSkip && (
            <Button variant={primary ? "ghost" : "secondary"} onClick={onSkip}>
              {skipLabel}
            </Button>
          )}
          {secondary}
          {primary && (
            <Button
              type={plain ? "button" : "submit"}
              form={plain ? undefined : formId}
              onClick={plain ? () => void primary.onClick() : undefined}
              variant="primary"
              loading={primary.loading}
              disabled={primary.disabled}
              leadingIcon={primary.icon}
            >
              {primary.label}
            </Button>
          )}
        </div>
      </footer>
      {footNote && <p className="onb-step__note">{footNote}</p>}
    </div>
  );

  return (
    <>
      <Root id={formId} ref={form} className={cx("onb-step", centered && "onb-step--centered", className)} onSubmit={plain ? undefined : submit} autoComplete={plain ? undefined : "off"} noValidate={plain ? undefined : true} aria-labelledby={`onb-title-${id}`} role={plain ? "group" : undefined}>
        <header className="onb-step__head">
          {lead && <div className="onb-step__lead">{lead}</div>}
          <h2 id={`onb-title-${id}`} ref={heading} tabIndex={-1} className={cx("onb-step__title", centered ? "t-display" : "t-title-lg")}>
            {title}
          </h2>
          {description && <p className="onb-step__description">{description}</p>}
        </header>
        {children && <div className="onb-step__body">{children}</div>}
        {notices && <div className="onb-step__notices">{notices}</div>}
        {!dock && footer}
      </Root>
      {dock &&
        createPortal(
          <div className="onb onb--dock">
            <div aria-hidden />
            <div className="onb-main">{footer}</div>
          </div>,
          dock,
        )}
    </>
  );
}

/* ------------------------------------------------------------------------- small parts */

/** A padded row inside a Card list, for rows that aren't a ListRow (a field, a meter). */
export function Row({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cx("onb-row", className)}>{children}</div>;
}

/**
 * The hero presence for the try and done steps. VoicePresence stays honest about the phase;
 * a success moment adds one ok check on its shoulder that settles in once (instant under
 * reduced motion), so “heard” never looks like “waiting”.
 */
export function HeroPresence({ phase, ok, size = 160 }: { phase: GhostPhase; ok?: boolean; size?: 96 | 160 }) {
  return (
    <span className={cx("onb-hero", `onb-hero--${size}`)}>
      <VoicePresence phase={phase} size={size} />
      {ok && (
        <span className="onb-hero__ok" aria-hidden>
          <Check strokeWidth={2.5} />
        </span>
      )}
    </span>
  );
}
