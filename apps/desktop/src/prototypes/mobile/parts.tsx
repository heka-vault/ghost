/**
 * Phone-sized building blocks for the mobile prototype. The shared primitives are sized for
 * a desktop pointer (controls top out at 40px); a phone needs 48dp targets and a few much
 * larger ones (Start, End). These stay local until the foundation adds a touch size
 * (see shared_requests in the mobile handoff).
 */
import { ChevronDown, ChevronLeft, House, ScrollText, Settings2, Signal, Wifi, BatteryFull } from "lucide-react";
import { type InputHTMLAttributes, type ReactNode, type Ref, type RefObject, useEffect, useRef, useState } from "react";

import { hrefFor, navigate } from "../../app/router";
import { cx, Spinner, useFieldControl, useLive } from "../../ui";

/* ------------------------------------------------------------------ touch button */

export type TouchVariant = "primary" | "secondary" | "inverse" | "ghost" | "danger" | "danger-ghost";
/** md 48 (the minimum target), lg 56, xl 64 (Start), hero 80 (End). */
export type TouchSize = "md" | "lg" | "xl" | "hero";

export interface TouchButtonProps {
  variant?: TouchVariant;
  size?: TouchSize;
  icon?: ReactNode;
  loading?: boolean;
  disabled?: boolean;
  /** Renders a link with the same look. */
  href?: string;
  onClick?: () => void;
  fullWidth?: boolean;
  className?: string;
  children: ReactNode;
  /** "submit" inside a form, so Enter in a field submits it. */
  type?: "button" | "submit";
  "aria-describedby"?: string;
}

/**
 * A phone button. Like `Button`, loading keeps the label and the width, sets aria-busy, and
 * is aria-disabled rather than disabled so focus stays put.
 */
export function TouchButton({ variant = "secondary", size = "md", icon, loading, disabled, href, onClick, fullWidth, className, children, type = "button", ...rest }: TouchButtonProps) {
  const cls = cx("mob-btn", `mob-btn--${variant}`, `mob-btn--${size}`, fullWidth && "mob-btn--full", loading && "is-loading", className);
  const body = (
    <>
      {(icon || loading) && <span className="mob-btn__icon" aria-hidden>{loading ? <Spinner size={size === "md" ? 16 : 20} /> : icon}</span>}
      <span className="mob-btn__label">{children}</span>
    </>
  );
  if (href && !disabled) {
    return <a className={cls} href={href} aria-describedby={rest["aria-describedby"]}>{body}</a>;
  }
  return (
    <button
      type={type}
      className={cls}
      disabled={disabled}
      aria-disabled={loading || undefined}
      aria-busy={loading || undefined}
      aria-describedby={rest["aria-describedby"]}
      onClick={(e) => {
        if (loading) e.preventDefault();
        else onClick?.();
      }}
    >
      {body}
    </button>
  );
}

/** A 48dp icon-only button with a required accessible name. */
export function TouchIcon({ label, icon, onClick, pressed, className }: { label: string; icon: ReactNode; onClick: () => void; pressed?: boolean; className?: string }) {
  return (
    <button type="button" className={cx("mob-icon-btn", pressed && "is-pressed", className)} aria-label={label} aria-pressed={pressed} onClick={onClick}>
      {icon}
    </button>
  );
}

/**
 * A 48dp text field with 16px text, so Android and iOS don't zoom on focus. Goes inside a
 * shared `Field`, which still wires the label, help and aria-invalid. Local until the
 * foundation adds a touch size to `Input` (shared_requests).
 */
export function TouchInput({ mono, invalid, className, ...rest }: InputHTMLAttributes<HTMLInputElement> & { mono?: boolean; invalid?: boolean }) {
  const wired = useFieldControl({ id: rest.id, "aria-describedby": rest["aria-describedby"], "aria-invalid": invalid || undefined });
  return <input {...rest} {...wired} className={cx("mob-input", mono && "is-mono", className)} />;
}

/**
 * A 48dp disclosure for phone screens, set like a section heading: the label on the content
 * edge, the chevron at the end. Local until `Disclosure` gets a touch size.
 */
export function TouchDisclosure({ summary, children, className }: { summary: ReactNode; children: ReactNode; className?: string }) {
  return (
    <details className={cx("mob-disclosure", className)}>
      <summary>
        <span className="mob-disclosure__label">{summary}</span>
        <ChevronDown size={20} className="mob-disclosure__chevron" aria-hidden />
      </summary>
      <div className="mob-disclosure__body">{children}</div>
    </details>
  );
}

/* ------------------------------------------------------------------ device chrome */

/** Android's status bar: the time left, radios and battery right. Decorative. */
export function StatusBar() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const h = window.setInterval(() => setNow(new Date()), 30_000);
    return () => window.clearInterval(h);
  }, []);
  return (
    <div className="mob-statusbar" aria-hidden>
      <span className="t-num">{now.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }).replace(/\s?[AP]M$/i, "")}</span>
      <span className="mob-statusbar__icons">
        <Signal size={14} />
        <Wifi size={14} />
        <BatteryFull size={16} />
      </span>
    </div>
  );
}

/** The gesture handle at the bottom edge. Decorative. */
export function GestureBar() {
  return <div className="mob-gesture" aria-hidden><span /></div>;
}

/* ------------------------------------------------------------------ navigation */

const TABS = [
  { id: "home", label: "Home", icon: House },
  { id: "activity", label: "Activity", icon: ScrollText },
  { id: "settings", label: "Settings", icon: Settings2 },
] as const;

/** Material-style bottom navigation: three destinations, 64dp, the current one on a pill. */
export function TabBar({ current }: { current: string }) {
  return (
    <nav className="mob-tabbar" aria-label="Phone">
      {TABS.map(({ id, label, icon: Icon }) => (
        <a key={id} href={hrefFor(`/mobile/${id}`)} className="mob-tab" aria-current={current === id ? "page" : undefined}>
          <span className="mob-tab__pill"><Icon size={20} aria-hidden /></span>
          <span className="mob-tab__label">{label}</span>
        </a>
      ))}
    </nav>
  );
}

/**
 * A screen's top bar with a 48dp back button and the screen title as the page's h1. Without a
 * title it is only the back button, and the screen supplies its own h1.
 */
export function TopBar({ title, back, trailing }: { title?: ReactNode; back?: string; trailing?: ReactNode }) {
  return (
    <header className="mob-topbar">
      {back ? (
        <button type="button" className="mob-icon-btn" aria-label="Back" onClick={() => navigate(back)}>
          <ChevronLeft size={24} aria-hidden />
        </button>
      ) : (
        <span className="mob-topbar__spacer" />
      )}
      {title ? <h1 className="mob-topbar__title">{title}</h1> : <span />}
      <span className="mob-topbar__trailing">{trailing}</span>
    </header>
  );
}

/* ------------------------------------------------------------------ connection */

export type Link = "connecting" | "online" | "reconnecting" | "offline";

/** How the phone reaches the desk, in words, with a dot that means only that. */
export function linkLabel(c: Link): { tone: "ok" | "warn" | "danger" | "neutral"; title: string } {
  if (c === "online") return { tone: "ok", title: "Connected" };
  if (c === "reconnecting") return { tone: "warn", title: "Reconnecting" };
  if (c === "offline") return { tone: "danger", title: "Not connected" };
  return { tone: "neutral", title: "Connecting…" };
}

export function useLink(force?: Link): Link {
  const { connection } = useLive();
  return force ?? connection;
}

export function LinkDot({ tone }: { tone: ReturnType<typeof linkLabel>["tone"] }) {
  return <span className={cx("mob-dot", `mob-dot--${tone}`)} aria-hidden />;
}

/* ------------------------------------------------------------------ rows */

/** A 56dp+ row for phone lists: leading, title and supporting text, trailing. */
export function Row({ leading, title, support, trailing, onClick, href, className, userContent }: { leading?: ReactNode; title: ReactNode; support?: ReactNode; trailing?: ReactNode; onClick?: () => void; href?: string; className?: string; userContent?: boolean }) {
  const inner = (
    <>
      {leading && <span className="mob-row__leading">{leading}</span>}
      <span className="mob-row__text">
        <span className="mob-row__title" data-user-content={userContent || undefined}>{title}</span>
        {support && <span className="mob-row__support">{support}</span>}
      </span>
    </>
  );
  if (href) {
    return (
      <div className={cx("mob-row", className)}>
        <a className="mob-row__hit" href={href}>{inner}</a>
        {trailing && <span className="mob-row__trailing">{trailing}</span>}
      </div>
    );
  }
  if (onClick) {
    return (
      <div className={cx("mob-row", className)}>
        <button type="button" className="mob-row__hit" onClick={onClick}>{inner}</button>
        {trailing && <span className="mob-row__trailing">{trailing}</span>}
      </div>
    );
  }
  return (
    <div className={cx("mob-row", className)}>
      <span className="mob-row__hit is-static">{inner}</span>
      {trailing && <span className="mob-row__trailing">{trailing}</span>}
    </div>
  );
}

/** Minutes and seconds since `iso`, ticking once a second. */
export function useElapsed(iso: string | null | undefined): string {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const h = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(h);
  }, []);
  if (!iso) return "0:00";
  const s = Math.max(0, Math.floor((now - Date.parse(iso)) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** “6 min”, “1 h 12 min”, “under a minute”. */
export function durationWords(fromIso: string, toIso: string): string {
  const m = Math.round((Date.parse(toIso) - Date.parse(fromIso)) / 60_000);
  if (m < 1) return "under a minute";
  if (m < 60) return `${m} min`;
  return `${Math.floor(m / 60)} h ${m % 60} min`;
}

/* ------------------------------------------------------------------ copy */

/**
 * Body copy one sentence per line, so a short line never breaks mid-sentence (“The / mic
 * stays off”). A long sentence still wraps on its own.
 */
export function Sentences({ text }: { text: string }) {
  const parts = text.match(/[^.?!]+[.?!…]+(?:[”’])?\s*|[^.?!]+$/g) ?? [text];
  return (
    <>
      {parts.map((t, i) => (
        <span key={i} className="mob-sentence">{t.trim()}</span>
      ))}
    </>
  );
}

/* ------------------------------------------------------------------ scrolling */

/**
 * The phone's scroller. Marks itself `data-scrolled` once it moves, which adds the design
 * system's 24px fade at the top edge, so content never cuts hard under the status bar.
 */
export function Scroller({ children, className, ref }: { children: ReactNode; className?: string; ref?: Ref<HTMLDivElement> }) {
  const own = useRef<HTMLDivElement>(null);
  const [scrolled, setScrolled] = useState(false);
  return (
    <div
      ref={(el) => {
        own.current = el;
        if (typeof ref === "function") ref(el);
        else if (ref) (ref as { current: HTMLDivElement | null }).current = el;
      }}
      className={cx("mob-scroll", className)}
      data-scrolled={scrolled || undefined}
      onScroll={(e) => setScrolled(e.currentTarget.scrollTop > 0)}
    >
      {children}
    </div>
  );
}

/**
 * Material’s collapsing top app bar for a large-title screen: an opaque 56dp bar over the
 * scroller that shows the title at 16px semibold, with a hairline, once the large title has
 * gone under it. Decorative (aria-hidden): the large title stays the page’s h1.
 */
export function CompactBar({ title, watch }: { title: ReactNode; watch: RefObject<HTMLElement | null> }) {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const el = watch.current;
    const root = el?.closest<HTMLElement>(".mob-scroll") ?? null;
    if (!el || !root) return;
    const io = new IntersectionObserver(
      ([e]) => setShown(!e.isIntersecting && e.boundingClientRect.top < (e.rootBounds?.top ?? 0)),
      { root, rootMargin: "-56px 0px 0px 0px", threshold: 0 },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [watch]);
  return (
    <div className={cx("mob-compactbar", shown && "is-shown")} aria-hidden>
      <span className="mob-compactbar__title">{title}</span>
    </div>
  );
}
