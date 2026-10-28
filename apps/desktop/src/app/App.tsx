import { SearchX } from "lucide-react";
import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Button, EmptyState, LiveContext, type LiveState, PageChromeContext, Progress, Skeleton, Spinner, ToastProvider } from "../ui";
import { useDesign } from "../design/context";
import { useConnection } from "./connection";
import { ErrorBoundary } from "./ErrorBoundary";
import { Offline } from "./Offline";
import { hrefFor, navigate, usePath } from "./router";
import { SETTINGS_PAGES, redirectFor, resolve, settingsPageOf } from "./routes";
import { SettingsFrame } from "./SettingsFrame";
import { useSetupNeeded } from "./setup";
import { Sidebar } from "./Sidebar";
import "./shell.css";

/** Design-mode chrome (the prototype strip), compiled only in development. */
const PrototypeStrip = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.PrototypeStrip }))) : null;

/** The app shell: sidebar, global connection banners, and the routed screen. */
export function App() {
  const raw = usePath();
  // An old address (#/apps, #/audit, #/settings) shows its new page at once; the hash follows.
  const path = redirectFor(raw) ?? raw;
  useEffect(() => {
    if (path !== raw) navigate(path, { replace: true });
  }, [path, raw]);
  const match = resolve(path);
  const settingsPage = match?.route.area === "settings" ? settingsPageOf(path) : null;
  const { state, phase, loadingModels } = useConnection();
  const design = useDesign();
  const setupNeeded = useSetupNeeded(path, state === "online");
  // A screen whose focal point is the setup prompt itself (the session's start state) claims
  // it with useSetupPromptHere(); then the banner steps aside: one signal per place.
  const [setupClaims, setSetupClaims] = useState(0);
  const claimSetupPrompt = useCallback((d: number) => setSetupClaims((n) => Math.max(0, n + d)), []);
  const setupHref = hrefFor(design ? "/welcome" : SETTINGS_PAGES[0].path);
  const live = useMemo<LiveState>(
    () => ({ connection: state, phase, online: state === "online", loadingModels, setupNeeded, setupHref, claimSetupPrompt }),
    [state, phase, loadingModels, setupNeeded, setupHref, claimSetupPrompt],
  );
  // The settings screen (General, Model, Passwords…) and the onboarding prototype are where
  // setup happens: no banner pointing at itself.
  const setupHere = match?.route.id === "settings" || path === "/welcome" || setupClaims > 0;
  const main = useRef<HTMLElement | null>(null);
  const [scroller, setScroller] = useState<HTMLElement | null>(null);
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const chrome = useMemo(() => ({ scroller, slot }), [scroller, slot]);
  const mainRef = useCallback((el: HTMLElement | null) => {
    main.current = el;
    setScroller(el);
  }, []);
  const [top, setTop] = useState<HTMLElement | null>(null);
  // --page-sticky-top on #main: the bottom of the sticky stack (banners plus the compact page
  // bar) from the top of the panel, so a screen's own sticky parts and scroll-margins sit
  // under it without measuring the bar themselves.
  useEffect(() => {
    const el = main.current;
    if (!el || !top) return;
    const bar = parseFloat(getComputedStyle(el).getPropertyValue("--page-bar-height")) || 48;
    const measure = () => {
      const banners = Array.from(top.children).filter((c) => !c.classList.contains("shell__bar-slot")).reduce((h, c) => h + c.getBoundingClientRect().height, 0);
      el.style.setProperty("--page-sticky-top", `${Math.round(banners + bar)}px`);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(top);
    return () => ro.disconnect();
  }, [top]);

  // Marks the panel as scrolled, which shows the compact page bar (or the top fade on
  // full and bare pages). A class, not state: no re-render per scroll event.
  const onScroll = useCallback(() => {
    const el = main.current;
    if (el) el.classList.toggle("is-scrolled", el.scrollTop > 0);
  }, []);

  // A new route starts at the top. The page panel is the scroller, not the window.
  useEffect(() => {
    main.current?.scrollTo({ top: 0 });
    onScroll();
  }, [path, onScroll]);

  const title = settingsPage?.label ?? match?.route.title;
  useEffect(() => {
    document.title = match ? (match.route.id === "session" ? "Ghost" : `${title} · Ghost`) : "Not found · Ghost";
  }, [match?.route.id, title]);

  const Screen = match?.route.component;
  const bare = match?.route.chrome === "bare";
  const screen = match && Screen ? (
    <ErrorBoundary key={match.route.id} name={title ?? match.route.title}>
      {/* A bare route (tray, phone) loads behind its own quiet placeholder, never the desktop page's shape. */}
      <Suspense fallback={bare ? <BareSkeleton /> : <ScreenSkeleton />}>
        <Screen params={match.params} />
      </Suspense>
    </ErrorBoundary>
  ) : (
    <EmptyState
      size="page"
      icon={<SearchX />}
      title="Nothing here"
      description={`There is no page at ${path}.`}
      actions={<Button href={hrefFor("/")}>Go to the session</Button>}
    />
  );

  useHeadingCheck(path, state);

  if (bare) {
    return (
      <LiveContext.Provider value={live}>
        <ToastProvider>{screen}</ToastProvider>
      </LiveContext.Provider>
    );
  }

  const gated = match?.route.needsDaemon && (state === "offline" || state === "connecting");

  return (
    <LiveContext.Provider value={live}>
    <ToastProvider>
      {/* A button, not href="#main": under hash routing that hash is a route. */}
      <button type="button" className="shell-skip" onClick={skipToContent}>
        Skip to content
      </button>
      <div className="shell">
        <Sidebar />
        <main ref={mainRef} className="shell__main" id="main" tabIndex={-1} onScroll={onScroll}>
          {/* Sticky at the top of the panel: connection banners, then the compact page bar's slot. */}
          <div className="shell__top" ref={setTop}>
          {state === "reconnecting" && (
            <div className="shell-banner" role="status">
              <Spinner size={14} />
              <span>
                <strong>Lost the connection to Ghost.</strong> Reconnecting. What you see may be out of date.
              </span>
            </div>
          )}
          {state === "online" && loadingModels && (
            <div className="shell-banner shell-banner--loading" role="status">
              {/* One signal: the progress line. No spinning icon, no second amber mark. */}
              <span>
                <strong>Ghost is loading its voice models.</strong> It can’t hear you until they’re ready, usually under a minute.
              </span>
              <Progress label="Loading voice models" className="shell-banner__progress" />
            </div>
          )}
          {state === "online" && !loadingModels && setupNeeded && !setupHere && (
            <div className="shell-banner shell-banner--setup" role="status">
              <span>
                <strong>Ghost needs a model password before it can answer.</strong> Add the Anthropic password or a model endpoint.
              </span>
              <Button size="sm" className="shell-banner__action" href={setupHref}>
                Finish setup
              </Button>
            </div>
          )}
            <div className="shell__bar-slot" ref={setSlot} />
          </div>
          {match?.route.kind === "prototype" && PrototypeStrip && (
            <Suspense fallback={null}>
              <PrototypeStrip />
            </Suspense>
          )}
          <PageChromeContext.Provider value={chrome}>
            <div className="shell__scroll">
              {gated ? (
                state === "offline" ? <Offline /> : <ScreenSkeleton />
              ) : settingsPage ? (
                // Settings has its own nav beside the page; it stays put while a page loads.
                <SettingsFrame current={settingsPage.id}>{screen}</SettingsFrame>
              ) : (
                screen
              )}
            </div>
          </PageChromeContext.Provider>
        </main>
      </div>
    </ToastProvider>
    </LiveContext.Provider>
  );
}

/**
 * Development only: every settled screen in the page panel has exactly one h1 (Page's title,
 * or a page-level EmptyState such as Offline and Not found). Checked once the panel stops
 * changing and isn't busy; a miss logs a console error, which the screenshot tool reports.
 */
function useHeadingCheck(path: string, state: string) {
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const main = document.getElementById("main");
    if (!main) return;
    let t = 0;
    let reported = "";
    const check = () => {
      if (main.querySelector('[aria-busy="true"]')) return;
      const n = main.querySelectorAll("h1").length;
      const key = `${path}:${n}`;
      if (n !== 1 && reported !== key) {
        reported = key;
        console.error(`[ghost] ${path} has ${n} h1 elements; every screen has exactly one (Page's title, or a page-level EmptyState).`);
      }
    };
    const schedule = () => {
      window.clearTimeout(t);
      t = window.setTimeout(check, 600);
    };
    const mo = new MutationObserver(schedule);
    mo.observe(main, { childList: true, subtree: true });
    schedule();
    return () => {
      mo.disconnect();
      window.clearTimeout(t);
    };
  }, [path, state]);
}

/** Moves focus to the page panel without touching the URL. */
function skipToContent() {
  const main = document.getElementById("main");
  main?.focus({ preventScroll: true });
  main?.scrollTo({ top: 0 });
}

/** A bare route while its code loads: one centered placeholder, no desktop page frame. */
function BareSkeleton() {
  return (
    <div className="shell-skeleton shell-skeleton--bare" aria-busy="true" aria-label="Loading">
      <Skeleton width={96} height={96} radius={999} />
      <Skeleton width={180} height={20} radius={6} />
    </div>
  );
}

/** Suspense and first-connect fallback: the page frame with placeholder lines. */
function ScreenSkeleton() {
  return (
    <div className="g-page g-page--default" aria-busy="true" aria-label="Loading">
      <div className="shell-skeleton">
        <Skeleton width={220} height={28} radius={6} />
        <Skeleton width={420} height={14} />
        <div className="shell-skeleton__block">
          <Skeleton height={56} radius={12} />
          <Skeleton height={56} radius={12} />
          <Skeleton height={56} radius={12} />
        </div>
      </div>
    </div>
  );
}
