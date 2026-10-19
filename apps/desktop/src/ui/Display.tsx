import { ChevronRight, CircleCheck, CircleX, Info, TriangleAlert, X } from "lucide-react";
import { type ComponentPropsWithRef, type CSSProperties, type ElementType, type KeyboardEvent, type ReactNode, type Ref, createContext, useContext } from "react";

import type { GhostPhase, Tone } from "@ghost/ui";

import { cx } from "./cx";
import { formatShortcut, shortcutKeys } from "./format";

/* ------------------------------------------------------------------------- Badge */
export type { Tone } from "@ghost/ui";

export interface BadgeProps {
  tone?: Tone;
  /** soft: tinted fill (default). outline: hairline only, for low-emphasis tags. */
  variant?: "soft" | "outline";
  size?: "sm" | "md";
  /** A leading status dot in the tone color. */
  dot?: boolean;
  icon?: ReactNode;
  mono?: boolean;
  title?: string;
  className?: string;
  children: ReactNode;
}

export function Badge({ tone = "neutral", variant = "soft", size = "md", dot, icon, mono, title, className, children }: BadgeProps) {
  return (
    <span title={title} className={cx("g-badge", `g-badge--${tone}`, `g-badge--${variant}`, `g-badge--${size}`, mono && "g-badge--mono", className)}>
      {dot && <span className="g-badge__dot" aria-hidden />}
      {icon}
      {children}
    </span>
  );
}

/* --------------------------------------------------------------------- StatusDot */
export type { GhostPhase } from "@ghost/ui";

export interface StatusDotProps {
  phase: GhostPhase;
  size?: "sm" | "md" | "lg";
  /** Accessible label; when omitted the dot is decorative. */
  label?: string;
  /** Hold the dot still even in a moving phase: something nearby already shows the motion. */
  still?: boolean;
  className?: string;
}

/**
 * The voice loop's state as one dot. It moves only when something is happening:
 * starting breathes, listening pulses a ring (the microphone is open to the cloud),
 * thinking shimmers, speaking ripples. Passive and offline are still.
 */
export function StatusDot({ phase, size = "md", label, still, className }: StatusDotProps) {
  return (
    <span
      className={cx("g-status-dot", `g-status-dot--${size}`, `is-${phase}`, still && "is-still", className)}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    />
  );
}

/* --------------------------------------------------------------------- Card/Section */
export interface CardProps extends ComponentPropsWithRef<"div"> {
  padding?: "none" | "sm" | "md" | "lg";
  /** Rows inside get hairline dividers and no outer padding (use with ListRow). */
  list?: boolean;
  tone?: "default" | "sunken" | "accent" | "warn" | "danger";
}

export function Card({ padding = "md", list, tone = "default", className, ...rest }: CardProps) {
  return <div className={cx("g-card", `g-card--pad-${list ? "none" : padding}`, list && "g-card--list", tone !== "default" && `g-card--${tone}`, className)} {...rest} />;
}

export interface SectionProps {
  title?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  /** Heading level for the title, default h2. */
  level?: 2 | 3;
  className?: string;
  id?: string;
  children?: ReactNode;
}

/** A titled block within a page. Sections are separated by space, not lines. */
export function Section({ title, description, actions, level = 2, className, id, children }: SectionProps) {
  const H = `h${level}` as ElementType;
  return (
    <section className={cx("g-section", className)} id={id} aria-labelledby={title && id ? `${id}-title` : undefined}>
      {(title || actions) && (
        <header className="g-section__header">
          <div className="g-section__titles">
            {title && <H id={id ? `${id}-title` : undefined} className="g-section__title">{title}</H>}
            {description && <p className="g-section__description">{description}</p>}
          </div>
          {actions && <div className="g-section__actions">{actions}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

/* ------------------------------------------------------------------------ ListRow */
export interface ListRowProps {
  title: ReactNode;
  description?: ReactNode;
  leading?: ReactNode;
  /** Right-aligned control (switch, select, button). Clicks on it do not trigger onClick. */
  trailing?: ReactNode;
  /** Small right-aligned text before trailing (time, count). */
  meta?: ReactNode;
  onClick?: () => void;
  href?: string;
  selected?: boolean;
  disabled?: boolean;
  /** Shows a chevron: the row navigates somewhere. */
  chevron?: boolean;
  density?: "compact" | "default" | "comfortable";
  /** Top-align leading and trailing with the first line (multi-line titles, a glyph beside a paragraph). */
  align?: "center" | "start";
  className?: string;
  "aria-current"?: "page" | "true" | boolean;
  /**
   * Roving-tabindex lists (a listbox of rows, one Tab stop): the hit area's tabIndex, role,
   * aria-selected and key handler, and a ref to focus it.
   */
  tabIndex?: number;
  role?: string;
  "aria-selected"?: boolean;
  onKeyDown?: (e: KeyboardEvent<HTMLElement>) => void;
  hitRef?: Ref<HTMLButtonElement & HTMLAnchorElement>;
  id?: string;
}

export function ListRow({ title, description, leading, trailing, meta, onClick, href, selected, disabled, chevron, density = "default", align = "center", className, tabIndex, role, onKeyDown, hitRef, id, ...rest }: ListRowProps) {
  const interactive = !!(onClick || href);
  const body = (
    <>
      {leading && <span className="g-row__leading">{leading}</span>}
      <span className="g-row__text">
        <span className="g-row__title">{title}</span>
        {description && <span className="g-row__description">{description}</span>}
      </span>
      {meta && <span className="g-row__meta">{meta}</span>}
      {chevron && <ChevronRight className="g-row__chevron" size={16} aria-hidden />}
    </>
  );
  const cls = cx("g-row", `g-row--${density}`, align === "start" && "g-row--start", interactive && "is-interactive", selected && "is-selected", disabled && "is-disabled", className);
  const roving = { tabIndex, role, "aria-selected": rest["aria-selected"], onKeyDown, id };
  if (!interactive) {
    return (
      <div className={cls} {...roving}>
        {body}
        {trailing && <span className="g-row__trailing">{trailing}</span>}
      </div>
    );
  }
  // The clickable area and the trailing control are siblings so a control inside the row
  // never nests inside a button (invalid HTML, broken keyboard behavior).
  return (
    <div className={cls}>
      {href ? (
        <a ref={hitRef} className="g-row__hit" href={href} aria-current={rest["aria-current"]} aria-disabled={disabled || undefined} {...roving}>
          {body}
        </a>
      ) : (
        <button ref={hitRef} type="button" className="g-row__hit" onClick={onClick} disabled={disabled} aria-current={rest["aria-current"]} aria-pressed={role ? undefined : (selected ?? undefined)} {...roving}>
          {body}
        </button>
      )}
      {trailing && <span className="g-row__trailing">{trailing}</span>}
    </div>
  );
}

/* -------------------------------------------------------------------------- Table */
export interface TableProps extends ComponentPropsWithRef<"table"> {
  density?: "compact" | "default";
  /** Header sticks to the top of the nearest scroller. */
  stickyHeader?: boolean;
  /** Rows highlight on hover (use when rows are clickable). */
  interactive?: boolean;
}

/** A styled <table>. Write thead/tbody/tr/th/td as usual; use `.is-num` on numeric cells. */
export function Table({ density = "default", stickyHeader, interactive, className, ...rest }: TableProps) {
  return (
    <div className="g-table-wrap">
      <table className={cx("g-table", `g-table--${density}`, stickyHeader && "g-table--sticky", interactive && "g-table--interactive", className)} {...rest} />
    </div>
  );
}

/* ---------------------------------------------------------------------------- Kbd */
export interface KbdProps {
  /** One key, written as it reads on the keyboard: "Esc", "/", "Enter". */
  children?: ReactNode;
  /** A whole shortcut in formatShortcut() notation ("mod+z", "super+g"): one key cap per key, Linux names. */
  shortcut?: string;
  className?: string;
}

/** Keyboard keys. Ghost runs on Linux: shortcuts read Ctrl, Alt, Shift and Super, never ⌘. */
export function Kbd({ children, shortcut, className }: KbdProps) {
  if (shortcut) {
    return (
      <kbd className={cx("g-kbds", className)} aria-label={formatShortcut(shortcut)}>
        {shortcutKeys(shortcut).map((k, i) => (
          <kbd key={i} className="g-kbd" aria-hidden>{k}</kbd>
        ))}
      </kbd>
    );
  }
  return <kbd className={cx("g-kbd", className)}>{children}</kbd>;
}

/* ----------------------------------------------------------------------- Skeleton */
export function Skeleton({ width, height = 12, radius, className, style }: { width?: number | string; height?: number | string; radius?: number | string; className?: string; style?: CSSProperties }) {
  return <span className={cx("g-skeleton", className)} aria-hidden style={{ width, height, borderRadius: radius, ...style }} />;
}

export function SkeletonText({ lines = 3, className }: { lines?: number; className?: string }) {
  return (
    <span className={cx("g-skeleton-text", className)} aria-hidden>
      {Array.from({ length: lines }, (_, i) => (
        <Skeleton key={i} width={i === lines - 1 ? "62%" : "100%"} />
      ))}
    </span>
  );
}

/* --------------------------------------------------------------------- EmptyState */
/** True inside a Page, which owns the screen's one h1. Provided by Page. */
export const InPageContext = createContext(false);

export interface EmptyStateProps {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  size?: "compact" | "default" | "page";
  /**
   * The title's element. Default: h1 for size="page" outside a Page (the state is the whole
   * screen: Offline, Not found, a crashed route), h2 for size="page" inside one, p otherwise.
   */
  headingLevel?: "h1" | "h2" | "h3" | "p";
  className?: string;
}

export function EmptyState({ icon, title, description, actions, size = "default", headingLevel, className }: EmptyStateProps) {
  const inPage = useContext(InPageContext);
  const Title = headingLevel ?? (size === "page" ? (inPage ? "h2" : "h1") : "p");
  return (
    <div className={cx("g-empty", `g-empty--${size}`, className)}>
      {icon && <div className="g-empty__icon" aria-hidden>{icon}</div>}
      <Title className="g-empty__title">{title}</Title>
      {description && <p className="g-empty__description">{description}</p>}
      {actions && <div className="g-empty__actions">{actions}</div>}
    </div>
  );
}

/* ------------------------------------------------------------------------- Notice */
export interface NoticeProps {
  tone?: "neutral" | "info" | "ok" | "warn" | "danger";
  title?: ReactNode;
  children?: ReactNode;
  icon?: ReactNode | false;
  action?: ReactNode;
  onDismiss?: () => void;
  className?: string;
}

const NOTICE_ICON = { neutral: Info, info: Info, ok: CircleCheck, warn: TriangleAlert, danger: CircleX };

/** An inline, persistent message inside the page: a result, a warning, an error. */
export function Notice({ tone = "neutral", title, children, icon, action, onDismiss, className }: NoticeProps) {
  const Icon = NOTICE_ICON[tone];
  return (
    <div className={cx("g-notice", `g-notice--${tone}`, className)} role={tone === "danger" ? "alert" : "status"}>
      {icon !== false && <span className="g-notice__icon">{icon ?? <Icon size={16} aria-hidden />}</span>}
      <div className="g-notice__body">
        {title && <p className="g-notice__title">{title}</p>}
        {children && <div className="g-notice__text">{children}</div>}
      </div>
      {action && <div className="g-notice__action">{action}</div>}
      {onDismiss && (
        <button type="button" className="g-notice__dismiss" aria-label="Dismiss" onClick={onDismiss}>
          <X size={14} aria-hidden />
        </button>
      )}
    </div>
  );
}

/* ---------------------------------------------------------------------- Progress */
/** A thin bar. Omit value for indeterminate (model loading). `tone` colors the bar: accent (default), or a phase or semantic color. */
export function Progress({ value, label, tone = "accent", className }: { value?: number; label: string; tone?: "accent" | "starting" | "ok" | "warn" | "danger" | "info" | "neutral"; className?: string }) {
  const indeterminate = value == null;
  return (
    <div
      className={cx("g-progress", `g-progress--${tone}`, indeterminate && "is-indeterminate", className)}
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={indeterminate ? undefined : Math.round(value)}
    >
      <span className="g-progress__bar" style={indeterminate ? undefined : { width: `${Math.max(0, Math.min(100, value))}%` }} />
    </div>
  );
}

/* --------------------------------------------------------------------- Disclosure */
export function Disclosure({ summary, children, defaultOpen, className }: { summary: ReactNode; children: ReactNode; defaultOpen?: boolean; className?: string }) {
  return (
    <details className={cx("g-disclosure", className)} open={defaultOpen}>
      <summary>
        <ChevronRight size={14} className="g-disclosure__chevron" aria-hidden />
        {summary}
      </summary>
      <div className="g-disclosure__body">{children}</div>
    </details>
  );
}
