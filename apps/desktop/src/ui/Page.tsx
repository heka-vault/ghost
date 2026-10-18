import { createContext, type ReactNode, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { GlyphSizeContext } from "./Brand";
import { ButtonSizeContext } from "./Button";
import { cx } from "./cx";
import { InPageContext } from "./Display";

export interface PageHeaderProps {
  title: ReactNode;
  /** One or two plain sentences: what this page is for. */
  description?: ReactNode;
  /** Right-aligned buttons. At most one primary. */
  actions?: ReactNode;
  /** Small line above the title (e.g. a Prototype badge). Comes after `status`. */
  eyebrow?: ReactNode;
  /** A way back, first in the eyebrow: “‹ Conversations”. Comes before `status`. */
  crumb?: ReactNode;
  /**
   * The page's live state, short: a StatusDot and a few words ("Listening"). Shown first in
   * the eyebrow, and repeated after the title in the compact bar, so a screen never loses its
   * own state when its header scrolls away.
   */
  status?: ReactNode;
  /** Leading visual (e.g. a LineGlyph). */
  leading?: ReactNode;
  /** The title's element: h1 (default). Specimens in the gallery use "p" so the page keeps one h1. */
  headingLevel?: "h1" | "h2" | "p";
  className?: string;
}

export function PageHeader({ title, description, actions, eyebrow, crumb, status, leading, headingLevel: Title = "h1", className }: PageHeaderProps) {
  return (
    <header className={cx("g-page-header", className)}>
      {leading && <div className="g-page-header__leading">{leading}</div>}
      <div className="g-page-header__titles">
        {(status || eyebrow || crumb) && (
          <div className="g-page-header__eyebrow">
            {crumb && <span className="g-page-crumb">{crumb}</span>}
            {status && <span className="g-page-status">{status}</span>}
            {eyebrow}
          </div>
        )}
        <Title className="g-page-header__title">{title}</Title>
        {description && <p className="g-page-header__description">{description}</p>}
      </div>
      {actions && <div className="g-page-header__actions">{actions}</div>}
    </header>
  );
}

/**
 * Provided by the app shell: where the compact page bar renders (a slot under any
 * connection banner, sticky at the top of the page panel) and the element that scrolls.
 * Screens never use this; Page does.
 */
export interface PageChrome {
  slot: HTMLElement | null;
  scroller: HTMLElement | null;
}
export const PageChromeContext = createContext<PageChrome | null>(null);

/** Height of the compact page bar. Matches --page-bar-height. */
const BAR_HEIGHT = 48;

export type PageWidth = "narrow" | "default" | "wide" | "full";

export interface PageProps extends Omit<PageHeaderProps, "className" | "headingLevel"> {
  /**
   * Every width shares one column (centered, up to 1160) and one left edge, so the title never
   * moves between routes. narrow caps header, content and dock at 680 (forms, settings),
   * default at 880 (single lists, policy), wide uses the whole column (tables, two panes, the
   * session transcript). full has no column and no padding (bare canvases: tray, mobile).
   */
  width?: PageWidth;
  /** Hide the header. The title still renders as a visually hidden h1, so the page keeps its one heading. */
  bare?: boolean;
  /**
   * Once the header scrolls under the top of the panel, show a 48px bar pinned there with
   * the leading glyph (at sm), the title, the `status`, and the header's actions at size sm
   * (the same instance, moved, not a copy). Default on for pages with a header that aren't
   * `full`; `full` and `bare` pages get a 24px fade at the top edge instead.
   */
  stickyHeader?: boolean;
  /**
   * Pinned to the bottom of the page panel while the content scrolls under it: a composer,
   * a save bar. The content fades out above it. One dock per page.
   */
  dock?: ReactNode;
  /**
   * Put the dock before the content in Tab order (it still sits at the bottom): a save bar on
   * a long form is then one Tab from the header, not one past every field.
   */
  dockFirst?: boolean;
  className?: string;
  children: ReactNode;
}

/**
 * The page layout every screen uses: a header (title, description, actions) and a
 * centerd content column of a fixed width. The shell's page panel is the scroller.
 *
 * The header's actions exist exactly once. React renders them into one host element that
 * Page owns, and Page moves that element between the header and the compact bar as the bar
 * takes the title. Nothing remounts, state and open dialogs survive the move, and keyboard
 * and screen-reader users meet each action once.
 */
export function Page({ width = "default", bare, stickyHeader, dock, dockFirst, className, children, ...header }: PageProps) {
  const chrome = useContext(PageChromeContext);
  const sticky = (stickyHeader ?? width !== "full") && !bare && !!chrome?.slot;
  const root = useRef<HTMLDivElement>(null);
  const [titled, setTitled] = useState(false);
  const [host] = useState(() => (typeof document === "undefined" ? null : document.createElement("div")));
  const [headerSlot, setHeaderSlot] = useState<HTMLDivElement | null>(null);
  const [barSlot, setBarSlot] = useState<HTMLDivElement | null>(null);
  const hasActions = header.actions != null && header.actions !== false;
  const inBar = sticky && titled && !!barSlot;

  // The bar takes the title as soon as the big title has gone under it (not the whole header:
  // a page that overflows by less than its description and actions would otherwise end its
  // scroll with an empty bar over them).
  useEffect(() => {
    const el = root.current?.querySelector<HTMLElement>(".g-page-header__title");
    if (!sticky || !el) return;
    const io = new IntersectionObserver(
      ([e]) => setTitled(!e.isIntersecting && e.boundingClientRect.top < (e.rootBounds?.top ?? 0) + BAR_HEIGHT),
      { root: chrome?.scroller ?? null, rootMargin: `-${BAR_HEIGHT}px 0px 0px 0px`, threshold: 0 },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [sticky, chrome?.scroller]);

  // Move the one actions host to wherever it belongs now. The header slot keeps its size
  // while empty, so the scrolled-away header never reflows the page under the reader.
  useLayoutEffect(() => {
    if (!host || !headerSlot) return;
    host.className = "g-page-actions";
    const target = inBar ? barSlot : headerSlot;
    if (!target || host.parentElement === target) return;
    const focused = host.contains(document.activeElement) ? (document.activeElement as HTMLElement) : null;
    if (inBar) {
      const r = headerSlot.getBoundingClientRect();
      headerSlot.style.width = `${r.width}px`;
      headerSlot.style.height = `${r.height}px`;
    } else {
      headerSlot.style.width = "";
      headerSlot.style.height = "";
    }
    target.appendChild(host);
    focused?.focus({ preventScroll: true });
  }, [host, headerSlot, barSlot, inBar]);

  useEffect(() => () => host?.remove(), [host]);

  return (
    <InPageContext.Provider value={true}>
    <div ref={root} className={cx("g-page", `g-page--${width}`, dock != null && "has-dock", dockFirst && "dock-first", className)}>
      {/* bare still has its one h1, for screen readers: the page is never headless. */}
      {bare && <h1 className="sr-only">{header.title}</h1>}
      {!bare && (
        <PageHeader
          {...header}
          actions={hasActions && host ? <div ref={setHeaderSlot} className="g-page-header__slot" /> : header.actions}
        />
      )}
      {!bare &&
        hasActions &&
        host &&
        createPortal(<ButtonSizeContext.Provider value={inBar ? "sm" : null}>{header.actions}</ButtonSizeContext.Provider>, host)}
      {sticky &&
        chrome?.slot &&
        createPortal(
          <div className={cx("g-page-bar", titled && "is-titled")} inert={!titled} aria-hidden={!titled || undefined}>
            <div className="g-page-bar__inner">
              <div className={cx("g-page-bar__row", `g-page-bar__row--${width}`)}>
                {/* The h1 and the status stay in the header; this is a visual repeat, hidden from AT. */}
                <div className="g-page-bar__heading" aria-hidden>
                  {header.leading && (
                    <span className="g-page-bar__leading">
                      <GlyphSizeContext.Provider value="sm">{header.leading}</GlyphSizeContext.Provider>
                    </span>
                  )}
                  <span className="g-page-bar__title">{header.title}</span>
                  {header.status && <span className="g-page-status g-page-bar__status">{header.status}</span>}
                </div>
                {hasActions && <div ref={setBarSlot} className="g-page-bar__actions" />}
              </div>
            </div>
          </div>,
          chrome.slot,
        )}
      {dock != null && dockFirst && <div className="g-page__dock">{dock}</div>}
      <div className="g-page__content">{children}</div>
      {dock != null && !dockFirst && <div className="g-page__dock">{dock}</div>}
    </div>
    </InPageContext.Provider>
  );
}
