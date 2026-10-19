import type { ConversationView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ChevronRight } from "lucide-react";
import { Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";

import { useDesign } from "../design/context";
import { GhostMark, type GhostPhase, LineGlyph, StatusDot, cx, lineName, liveLabel, phaseText, relativeTime, useLive } from "../ui";
import { hrefFor, usePath } from "./router";
import { navMatch, ROUTES, type RouteDef } from "./routes";

/** Ghost, the live line, then the most recent: six rows, then a link to all of them. */
const MAX_LINES = 6;

/** Design-mode chrome, compiled only in development; null in production builds. */
const MockBadge = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.MockBadge }))) : null;
const Switcher = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.ScenarioSwitcher }))) : null;

/** Below this window height the Prototypes group starts collapsed, so the nav fits without scrolling. */
const SHORT_WINDOW = 840;
const PROTOS_KEY = "ghost.sidebar.prototypes";

export function Sidebar() {
  const path = usePath();
  const current = navMatch(path);
  const design = useDesign();
  const { client, status, connected, pending, conversationsVersion } = useGhost();
  const { connection: state, phase } = useLive();
  const [lines, setLines] = useState<ConversationView[]>([]);
  const scroller = useRef<HTMLDivElement>(null);
  // The Prototypes group: open by default on a tall window, collapsed on a short one (under
  // 840px) so Settings never pins over it; the operator's last choice wins for the session.
  const [protosOpen, setProtosOpen] = useState<boolean>(() => {
    const saved = sessionStorage.getItem(PROTOS_KEY);
    return saved ? saved === "open" : window.innerHeight >= SHORT_WINDOW;
  });
  const toggleProtos = () => {
    const next = !protosOpen;
    setProtosOpen(next);
    sessionStorage.setItem(PROTOS_KEY, next ? "open" : "closed");
  };

  // Fade the list's edges only on the side that has more to scroll to.
  const measure = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    el.classList.toggle("is-more-above", el.scrollTop > 1);
    el.classList.toggle("is-more-below", el.scrollTop + el.clientHeight < el.scrollHeight - 1);
  }, []);
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    for (const c of Array.from(el.children)) ro.observe(c);
    return () => ro.disconnect();
  }, [measure, lines.length, design, protosOpen]);

  useEffect(() => {
    if (!connected) return;
    let live = true;
    client
      .conversations()
      .then((l) => live && setLines(l))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, connected, conversationsVersion]);

  const main = ROUTES.filter((r) => r.component && r.nav?.group === "main");
  const footer = ROUTES.filter((r) => r.component && r.nav?.group === "footer");
  const protos = design ? ROUTES.filter((r) => r.component && r.nav?.group === "prototype") : [];

  const inSession = !!status?.session_id;
  const active = status?.conversation ?? null;
  const activeLine = lines.find((l) => l.slug === active);
  // The card says "Not running" only once the shell says offline, the same moment the Offline
  // page appears. Until then it is a neutral, still "Connecting…": never a false alarm. The
  // same liveLabel() feeds every page's <LiveStatus />, so the two never disagree.
  const { dot: cardPhase, title } = liveLabel(state, phase);
  // The footer never moves between states: the last known version, or a reserved empty row.
  const version = useRef<string | null>(null);
  if (status?.version) version.current = status.version;
  const sub =
    state === "connecting"
      ? "Looking for Ghost"
      : state === "offline"
      ? "The Ghost service is stopped"
      : state === "reconnecting"
        ? "Trying again now"
        : phase === "starting"
          ? "Loading voice models"
          : inSession
            ? status?.session_origin === "phone"
              ? "On your phone"
              : active && active !== "ghost" ? `On the ${lineName(active, activeLine?.name)} line` : "On the main line"
            : "Nothing leaves this computer";
  // Where the eye starts: Ghost, then the line Ghost is on right now (which is also the one
  // holding any pending okay), then the rest by recency. The live line is never buried.
  const rank = (l: ConversationView) => (l.is_ghost ? 0 : inSession && l.slug === active ? 1 : 2);
  const sortedLines = [...lines].sort((a, b) => rank(a) - rank(b) || (b.last_active_at ?? "").localeCompare(a.last_active_at ?? ""));
  // Exactly one current row. When the open line has its own row, that row is current and
  // Conversations drops to a quiet "section active" state (no tile, no aria-current).
  const shownLines = state !== "offline" ? sortedLines.slice(0, MAX_LINES) : [];
  const lineIsCurrent = shownLines.some((l) => path === `/conversations/${l.slug}`);

  return (
    <aside className="sidebar">
      <div className="sidebar__brand">
        <GhostMark size={22} />
        <span className="sidebar__brand-name">Ghost</span>
        {design && MockBadge && (
          <Suspense fallback={null}>
            <MockBadge />
          </Suspense>
        )}
      </div>

      <StatusCard phase={cardPhase} title={title} sub={sub} />

      <nav className="sidebar__nav" aria-label="Main">
      <div className="sidebar__scroll" ref={scroller} onScroll={measure}>
        <ul className="sidebar__group" role="list">
          {main.map((r) => (
            <NavItem
              key={r.id}
              route={r}
              current={current === r.id && !(lineIsCurrent && r.id === "conversations")}
              section={lineIsCurrent && r.id === "conversations"}
              badge={r.id === "session" && pending && connected ? 1 : undefined}
            />
          ))}
        </ul>

        {/* Lines stay on screen while reconnecting: stale, never cleared. */}
        {state !== "offline" && sortedLines.length > 0 && (
          <div className="sidebar__section">
            <div className="sidebar__label" id="sidebar-lines">
              Lines
            </div>
            <ul className="sidebar__group" role="list" aria-labelledby="sidebar-lines">
              {shownLines.map((l) => {
                const to = `/conversations/${l.slug}`;
                const live = inSession && l.slug === active;
                const waiting = live && !!pending;
                const sub = waiting
                  ? "Waiting on your okay"
                  : live
                    ? `Live · ${state === "reconnecting" ? "reconnecting" : phaseText(phase).toLowerCase()}`
                    : l.last_active_at
                      ? relativeTime(l.last_active_at)
                      : "Not used yet";
                return (
                  <li key={l.slug}>
                    <a className={cx("sidebar__item sidebar__item--line", path === to && "is-current")} href={hrefFor(to)} aria-current={path === to ? "page" : undefined}>
                      <LineGlyph slug={l.slug} name={l.name} size="sm" />
                      <span className="sidebar__line-text">
                        <span className="sidebar__line-name">{l.is_ghost ? "Ghost" : l.name}</span>
                        <span className={cx("sidebar__line-sub", live && "is-live", waiting && "is-waiting")}>{sub}</span>
                      </span>
                      {/* The status card owns the phase dot; "Live · listening" already says it here.
                          Only a pending okay adds a dot, because there the color adds information. */}
                      {waiting && (
                        <span className="sidebar__live">
                          <span className="sidebar__waiting-dot" aria-hidden />
                        </span>
                      )}
                    </a>
                  </li>
                );
              })}
              {sortedLines.length > MAX_LINES && (
                <li>
                  <a className="sidebar__item sidebar__item--more" href={hrefFor("/conversations")}>
                    <span className="sidebar__item-label">All {sortedLines.length} lines</span>
                  </a>
                </li>
              )}
            </ul>
          </div>
        )}

        {protos.length > 0 && (
          <div className="sidebar__section">
            <button type="button" className={cx("sidebar__label sidebar__label--toggle", protosOpen && "is-open")} id="sidebar-protos" aria-expanded={protosOpen} aria-controls="sidebar-protos-list" onClick={toggleProtos}>
              <ChevronRight size={12} aria-hidden />
              Prototypes
              {!protosOpen && protos.some((r) => current === r.id) && <span className="sidebar__label-dot" aria-hidden />}
            </button>
            {protosOpen && (
              <ul className="sidebar__group" role="list" id="sidebar-protos-list" aria-labelledby="sidebar-protos">
                {protos.map((r) => (
                  <NavItem key={r.id} route={r} current={current === r.id} />
                ))}
              </ul>
            )}
          </div>
        )}
      </div>
      </nav>

      <div className="sidebar__footer">
        <nav aria-label="Settings">
          <ul className="sidebar__group" role="list">
            {footer.map((r) => (
              <NavItem key={r.id} route={r} current={current === r.id} />
            ))}
          </ul>
        </nav>
        {design && Switcher && (
          <Suspense fallback={null}>
            <Switcher />
          </Suspense>
        )}
        {/* Always one row tall, so the footer holds still. Empty until Ghost first answers. */}
        <div className="sidebar__version t-caption t-3 t-num" aria-hidden={version.current ? undefined : true}>{version.current ? `Ghost ${version.current}` : "\u00a0"}</div>
      </div>
    </aside>
  );
}

/** The sidebar's status card: the one place the voice loop's phase dot lives. Exported for the gallery. */
export function StatusCard({ phase, title, sub }: { phase: GhostPhase; title: string; sub: string }) {
  return (
    <a className={cx("sidebar__status", `is-${phase}`)} href={hrefFor("/")}>
      <span className="sidebar__status-dot">
        <StatusDot phase={phase} />
      </span>
      <span className="sidebar__status-text">
        <span className="sidebar__status-title">{title}</span>
        <span className="sidebar__status-sub">{sub}</span>
      </span>
    </a>
  );
}

function NavItem({ route, current, section, badge }: { route: RouteDef; current: boolean; section?: boolean; badge?: number }) {
  const Icon = route.nav!.icon;
  return (
    <li>
      <a className={cx("sidebar__item", current && "is-current", section && "is-section")} href={hrefFor(route.path)} aria-current={current ? "page" : undefined}>
        <Icon size={16} aria-hidden />
        <span className="sidebar__item-label">{route.nav!.label}</span>
        {badge != null && (
          <span className="sidebar__count" aria-label={`${badge} waiting`}>
            {badge}
          </span>
        )}
      </a>
    </li>
  );
}
