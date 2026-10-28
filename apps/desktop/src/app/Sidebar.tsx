import type { ConversationView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ChevronDown, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { type ReactElement, type ReactNode, Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";

import { useDesign } from "../design/context";
import { GhostMark, type GhostPhase, IconButton, LineGlyph, StatusDot, Tooltip, agentName, cx, liveLabel, phaseText, relativeTime, useLive, versionLine } from "../ui";
import { isRetired } from "../screens/conversations/threads";
import { newAgentSeed } from "../screens/conversations/url";
import { hrefFor, usePath } from "./router";
import { navMatch, ROUTES, type RouteDef } from "./routes";
import { DESKTOP_VERSION, otherVersion } from "./version";

/** Design-mode chrome, compiled only in development; null in production builds. */
const MockBadge = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.MockBadge }))) : null;
const Switcher = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.ScenarioSwitcher }))) : null;

/** Below this window height the Prototypes group starts collapsed, so the nav fits without scrolling. */
const SHORT_WINDOW = 840;
const PROTOS_KEY = "ghost.sidebar.prototypes";
/** Collapsed to the icon rail: kept across restarts, unlike the fold above. */
const COLLAPSED_KEY = "ghost.sidebar.collapsed";
const TOGGLE_SHORTCUT = "mod+b";

/** In the rail every row is an icon, so its name moves into a tooltip on the right. */
function RailTip({ rail, content, children }: { rail: boolean; content: ReactNode; children: ReactElement }) {
  return rail ? (
    <Tooltip content={content} side="right" delay={150} asChild>
      {children}
    </Tooltip>
  ) : (
    children
  );
}

/** The mark and the name. Pointing at it plays the mascot; its state stays here, not in Sidebar. */
function Brand({ rail, onToggle, children }: { rail: boolean; onToggle: () => void; children?: ReactNode }) {
  const [hovered, setHovered] = useState(false);
  const toggle = (
    <Tooltip content={rail ? "Show the sidebar" : "Hide the sidebar"} shortcut={TOGGLE_SHORTCUT} side={rail ? "right" : "bottom"}>
      <IconButton
        className="sidebar__toggle"
        label={rail ? "Show the sidebar" : "Hide the sidebar"}
        icon={rail ? <PanelLeftOpen /> : <PanelLeftClose />}
        tooltip={false}
        aria-expanded={!rail}
        onClick={onToggle}
      />
    </Tooltip>
  );
  // The rail keeps only the way back out; the mark and the name return with the sidebar.
  if (rail) return <div className="sidebar__brand">{toggle}</div>;
  return (
    <div className="sidebar__brand" onPointerEnter={(e) => e.pointerType === "mouse" && setHovered(true)} onPointerLeave={() => setHovered(false)}>
      <GhostMark size={28} hovered={hovered} />
      <span className="sidebar__brand-name">Ghost</span>
      {children}
      {toggle}
    </div>
  );
}

export function Sidebar() {
  const path = usePath();
  const current = navMatch(path);
  const design = useDesign();
  const { client, status, connected, pending, conversationsVersion } = useGhost();
  const { connection: state, phase } = useLive();
  const [agents, setAgents] = useState<ConversationView[]>([]);
  const scroller = useRef<HTMLDivElement>(null);
  const [rail, setRail] = useState<boolean>(() => localStorage.getItem(COLLAPSED_KEY) === "1");
  const toggleRail = useCallback(() => {
    setRail((was) => {
      localStorage.setItem(COLLAPSED_KEY, was ? "0" : "1");
      return !was;
    });
  }, []);
  // Ctrl+B anywhere, as in editors. Nothing in Ghost formats text, so the key is free.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.repeat || e.altKey || e.shiftKey || !(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "b") return;
      e.preventDefault();
      toggleRail();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleRail]);
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
  }, [measure, agents.length, design, protosOpen]);

  useEffect(() => {
    if (!connected) return;
    let live = true;
    client
      .conversations()
      .then((l) => live && setAgents(l))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, connected, conversationsVersion]);

  // Agents are the section below, not a nav item: one word, one place (GLOSSARY.md). Apps is
  // a page of Settings with a row of its own here; the rest of Settings is behind the pinned
  // Settings row, which has its own nav (2026-10-28). The session is the status card.
  const main = ROUTES.filter((r) => r.component && r.nav?.group === "main" && r.id !== "conversations");
  const footer = ROUTES.filter((r) => r.component && r.nav?.group === "footer");
  const protos = design ? ROUTES.filter((r) => r.component && r.nav?.group === "prototype") : [];
  // Any page of Settings but Apps, which has its own row: then Settings is the section.
  const inSettings = ROUTES.some((r) => r.id === current && r.area === "settings");
  const settingsCurrent = inSettings && !main.some((r) => r.id === current);

  const inSession = !!status?.session_id;
  const active = status?.conversation ?? null;
  const activeAgent = agents.find((l) => l.slug === active);
  // The card says "Not running" only once the shell says offline, the same moment the Offline
  // page appears. Until then it is a neutral, still "Connecting…": never a false alarm. The
  // same liveLabel() feeds <LiveStatus /> wherever it is used, so the two never disagree.
  const { dot: cardPhase, title } = liveLabel(state, phase);
  // The footer never moves between states: the last known version, or a reserved empty row.
  // When this window isn't the daemon's version (a checkout's window on the deployed Ghost),
  // a second quiet line says which this one is (docs/releasing.md).
  const version = useRef<{ line: string; other: string | null } | null>(null);
  if (status?.version) version.current = { line: versionLine(status.version, status.build), other: otherVersion(status.version) };
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
              : active && active !== "ghost" ? `With ${agentName(active, activeAgent?.name)}` : "With Ghost"
            : "Nothing leaves this computer";
  // Where the eye starts: Ghost, then the agent Ghost is with right now (which is also the one
  // holding any pending okay), then the rest by recency. The live agent is never buried.
  const rank = (l: ConversationView) => (l.is_ghost ? 0 : inSession && l.slug === active ? 1 : 2);
  // A retired agent has left the roster; the Agents screen keeps it in its Retired group.
  const working = agents.filter((l) => !isRetired(l));
  const sortedAgents = [...working].sort((a, b) => rank(a) - rank(b) || (b.last_active_at ?? "").localeCompare(a.last_active_at ?? ""));
  // Every working agent, right here: the sidebar is the list, not a preview of one (the operator's call, 2026-10-20).
  const shownAgents = state !== "offline" ? sortedAgents : [];
  const retiredCount = agents.filter(isRetired).length;
  const agentIsCurrent = shownAgents.some((l) => path === `/agents/${l.slug}`);

  return (
    <aside className={cx("sidebar", rail && "is-rail")}>
      <Brand rail={rail} onToggle={toggleRail}>
        {design && MockBadge && (
          <Suspense fallback={null}>
            <MockBadge />
          </Suspense>
        )}
      </Brand>

      <StatusCard phase={cardPhase} title={title} sub={sub} rail={rail} />

      <nav className="sidebar__nav" aria-label="Main">
      <div className="sidebar__scroll" ref={scroller} onScroll={measure}>
        {main.length > 0 && (
          <ul className="sidebar__group" role="list">
            {main.map((r) => (
              <NavItem
                key={r.id}
                route={r}
                rail={rail}
                current={current === r.id && !(agentIsCurrent && r.id === "conversations")}
                section={agentIsCurrent && r.id === "conversations"}
                badge={r.id === "waiting" && connected && status?.waiting ? status.waiting : undefined}
              />
            ))}
          </ul>
        )}

        {/* Agents stay on screen while reconnecting: stale, never cleared. */}
        {state !== "offline" && sortedAgents.length > 0 && (
          <div className="sidebar__section">
            {/* The heading is the way to the Agents screen (every agent, retired ones, New agent). */}
            <a className="sidebar__label sidebar__label--link" id="sidebar-agents" href={hrefFor("/agents")} aria-current={path === "/agents" ? "page" : undefined}>
              Agents
            </a>
            <ul className="sidebar__group" role="list" aria-labelledby="sidebar-agents">
              {shownAgents.map((l) => {
                const to = `/agents/${l.slug}`;
                const live = inSession && l.slug === active;
                const waiting = live && !!pending;
                const sub = waiting
                  ? "Waiting on your okay"
                  : live
                    ? `Live · ${state === "reconnecting" ? "reconnecting" : phaseText(phase).toLowerCase()}`
                    : l.last_active_at
                      ? relativeTime(l.last_active_at)
                      : "Not used yet";
                const name = l.is_ghost ? "Ghost" : l.name;
                return (
                  <li key={l.slug}>
                    <RailTip rail={rail} content={`${name} · ${sub}`}>
                    <a
                      className={cx("sidebar__item sidebar__item--line", path === to && "is-current")}
                      href={hrefFor(to)}
                      aria-current={path === to ? "page" : undefined}
                      aria-label={rail ? `${name}, ${sub}` : undefined}
                      title={rail ? undefined : l.job || undefined}
                    >
                      <LineGlyph slug={l.slug} name={l.name} size="sm" />
                      <span className="sidebar__line-text">
                        <span className="sidebar__line-name">{name}</span>
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
                    </RailTip>
                  </li>
                );
              })}
              {!rail && (
              <li>
                <a
                  className="sidebar__item sidebar__item--more"
                  href={hrefFor("/agents")}
                  onClick={() => {
                    newAgentSeed.open = true;
                  }}
                >
                  <span className="sidebar__item-label">New agent…</span>
                </a>
              </li>
              )}
              {!rail && retiredCount > 0 && (
                <li>
                  <a
                    className="sidebar__item sidebar__item--more"
                    href={hrefFor("/agents")}
                    onClick={() => {
                      newAgentSeed.retired = true;
                    }}
                  >
                    <span className="sidebar__item-label">Retired · {retiredCount}</span>
                  </a>
                </li>
              )}
            </ul>
          </div>
        )}

        {protos.length > 0 && (
          <div className="sidebar__section">
            <button type="button" className={cx("sidebar__label sidebar__label--toggle", protosOpen && "is-open")} id="sidebar-protos" aria-expanded={protosOpen} aria-controls="sidebar-protos-list" onClick={toggleProtos}>
              <ChevronDown size={12} aria-hidden className="sidebar__label-chevron" />
              Prototypes
              {!protosOpen && protos.some((r) => current === r.id) && <span className="sidebar__label-dot" aria-hidden />}
            </button>
            {protosOpen && (
              <ul className="sidebar__group" role="list" id="sidebar-protos-list" aria-labelledby="sidebar-protos">
                {protos.map((r) => (
                  <NavItem key={r.id} route={r} rail={rail} current={current === r.id} />
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
              <NavItem key={r.id} route={r} rail={rail} current={settingsCurrent} section={inSettings && !settingsCurrent} />
            ))}
          </ul>
        </nav>
        {design && Switcher && !rail && (
          <Suspense fallback={null}>
            <Switcher />
          </Suspense>
        )}
        {/* At least one row tall, so the footer holds still. Empty until Ghost first answers. */}
        {!rail && (
          <div className="sidebar__version t-caption t-3 t-num" aria-hidden={version.current ? undefined : true}>
            {version.current?.line ?? " "}
            {version.current?.other && <span className="sidebar__version-other">This window is {DESKTOP_VERSION}</span>}
          </div>
        )}
      </div>
    </aside>
  );
}

/** The sidebar's status card: the one place the voice loop's phase dot lives. Exported for the gallery. */
export function StatusCard({ phase, title, sub, rail = false }: { phase: GhostPhase; title: string; sub: string; rail?: boolean }) {
  return (
    <RailTip rail={rail} content={`${title} · ${sub}`}>
    <a className={cx("sidebar__status", `is-${phase}`)} href={hrefFor("/")} aria-label={rail ? `${title}, ${sub}` : undefined}>
      <span className="sidebar__status-dot">
        <StatusDot phase={phase} />
      </span>
      <span className="sidebar__status-text">
        <span className="sidebar__status-title">{title}</span>
        <span className="sidebar__status-sub">{sub}</span>
      </span>
    </a>
    </RailTip>
  );
}

function NavItem({ route, current, section, badge, rail = false }: { route: RouteDef; current: boolean; section?: boolean; badge?: number; rail?: boolean }) {
  const Icon = route.nav!.icon;
  const label = route.nav!.label;
  return (
    <li>
      <RailTip rail={rail} content={badge != null ? `${label} · ${badge} waiting` : label}>
      <a
        className={cx("sidebar__item", current && "is-current", section && "is-section")}
        href={hrefFor(route.path)}
        aria-current={current ? "page" : undefined}
        aria-label={rail ? (badge != null ? `${label}, ${badge} waiting` : label) : undefined}
      >
        <Icon size={16} aria-hidden />
        <span className="sidebar__item-label">{label}</span>
        {badge != null && (
          <span className="sidebar__count" aria-label={`${badge} waiting`}>
            {badge}
          </span>
        )}
      </a>
      </RailTip>
    </li>
  );
}
