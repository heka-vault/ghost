import type { ConversationView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ChevronDown } from "lucide-react";
import { type ReactNode, Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";

import { useDesign } from "../design/context";
import { GhostMark, type GhostPhase, LineGlyph, StatusDot, agentName, cx, liveLabel, phaseText, relativeTime, useLive } from "../ui";
import { groupByDepartment, isRetired } from "../screens/conversations/threads";
import { newAgentSeed } from "../screens/conversations/url";
import { hrefFor, usePath } from "./router";
import { navMatch, ROUTES, type RouteDef } from "./routes";

/** Design-mode chrome, compiled only in development; null in production builds. */
const MockBadge = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.MockBadge }))) : null;
const Switcher = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.ScenarioSwitcher }))) : null;

/** Below this window height the Prototypes group starts collapsed, so the nav fits without scrolling. */
const SHORT_WINDOW = 840;
const PROTOS_KEY = "ghost.sidebar.prototypes";
const SETTINGS_KEY = "ghost.sidebar.settings";

/** The mark and the name. Pointing at it plays the mascot; its state stays here, not in Sidebar. */
function Brand({ children }: { children?: ReactNode }) {
  const [hovered, setHovered] = useState(false);
  return (
    <div className="sidebar__brand" onPointerEnter={(e) => e.pointerType === "mouse" && setHovered(true)} onPointerLeave={() => setHovered(false)}>
      <GhostMark size={28} hovered={hovered} />
      <span className="sidebar__brand-name">Ghost</span>
      {children}
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

  // Agents are the section below, not a nav item: one word, one place (GLOSSARY.md). The
  // session and the configuration screens fold under the pinned Settings row (2026-10-24).
  const main = ROUTES.filter((r) => r.component && r.nav?.group === "main" && r.id !== "conversations");
  const footer = ROUTES.filter((r) => r.component && r.nav?.group === "footer");
  const under = ROUTES.filter((r) => r.component && r.nav?.group === "settings");
  const protos = design ? ROUTES.filter((r) => r.component && r.nav?.group === "prototype") : [];
  const inSettings = under.some((r) => r.id === current) || footer.some((r) => r.id === current);
  // The Settings group: open while one of its screens is the page, else as the operator last
  // left it (closed to start). Landing on one of them opens it; Settings itself doesn't.
  const [settingsOpen, setSettingsOpen] = useState<boolean>(() => sessionStorage.getItem(SETTINGS_KEY) === "open");
  const toggleSettings = () => {
    const next = !settingsOpen;
    setSettingsOpen(next);
    sessionStorage.setItem(SETTINGS_KEY, next ? "open" : "closed");
  };
  useEffect(() => {
    if (under.some((r) => r.id === current)) setSettingsOpen(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current]);

  const inSession = !!status?.session_id;
  const active = status?.conversation ?? null;
  const activeAgent = agents.find((l) => l.slug === active);
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
              : active && active !== "ghost" ? `With ${agentName(active, activeAgent?.name)}` : "With Ghost"
            : "Nothing leaves this computer";
  // Where the eye starts: Ghost, then the agent Ghost is with right now (which is also the one
  // holding any pending okay), then the rest by recency. The live agent is never buried.
  const rank = (l: ConversationView) => (l.is_ghost ? 0 : inSession && l.slug === active ? 1 : 2);
  // A retired agent (ADR 0024) has left the roster; the Agents screen keeps it in its Retired group.
  const working = agents.filter((l) => !isRetired(l));
  const sortedAgents = [...working].sort((a, b) => rank(a) - rank(b) || (b.last_active_at ?? "").localeCompare(a.last_active_at ?? ""));
  // Every working agent, right here: the sidebar is the list, not a preview of one (the operator's call, 2026-10-20).
  const shownAgents = state !== "offline" ? sortedAgents : [];
  // Agents sit under their department's own agent (ADR 0024), one glyph column in; a group takes the place of its first member, so the live agent still pulls its department up.
  const groups = groupByDepartment(shownAgents).flatMap((g) => [...(g.head ? [{ agent: g.head, member: false }] : []), ...g.agents.map((l) => ({ agent: l, member: !!g.head }))]);
  const retiredCount = agents.filter(isRetired).length;
  const agentIsCurrent = shownAgents.some((l) => path === `/agents/${l.slug}`);

  return (
    <aside className="sidebar">
      <Brand>
        {design && MockBadge && (
          <Suspense fallback={null}>
            <MockBadge />
          </Suspense>
        )}
      </Brand>

      <StatusCard phase={cardPhase} title={title} sub={sub} />

      <nav className="sidebar__nav" aria-label="Main">
      <div className="sidebar__scroll" ref={scroller} onScroll={measure}>
        {main.length > 0 && (
          <ul className="sidebar__group" role="list">
            {main.map((r) => (
              <NavItem
                key={r.id}
                route={r}
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
              {groups.map(({ agent: l, member }) => {
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
                return (
                  <li key={l.slug}>
                    <a
                      className={cx("sidebar__item sidebar__item--line", member && "sidebar__item--topic", path === to && "is-current")}
                      href={hrefFor(to)}
                      aria-current={path === to ? "page" : undefined}
                      title={l.job || undefined}
                    >
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
              {retiredCount > 0 && (
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
              <NavItem
                key={r.id}
                route={r}
                current={current === r.id}
                section={inSettings && current !== r.id}
                // A waiting okay stays visible while the Session row is folded away.
                badge={!settingsOpen && pending && connected ? 1 : undefined}
                fold={
                  under.length
                    ? { open: settingsOpen, onToggle: toggleSettings, controls: "sidebar-settings-list", label: settingsOpen ? "Hide the settings screens" : "Show the settings screens" }
                    : undefined
                }
              />
            ))}
            {settingsOpen &&
              under.map((r) => <NavItem key={r.id} route={r} current={current === r.id} sub badge={r.id === "session" && pending && connected ? 1 : undefined} />)}
          </ul>
        </nav>
        {design && Switcher && (
          <Suspense fallback={null}>
            <Switcher />
          </Suspense>
        )}
        {/* Always one row tall, so the footer holds still. Empty until Ghost first answers. */}
        <div className="sidebar__version t-caption t-3 t-num" aria-hidden={version.current ? undefined : true}>{version.current ? `Ghost ${version.current}` : " "}</div>
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

/** A fold control at the row's end: the row still navigates; the chevron opens or closes the group under it. */
interface Fold {
  open: boolean;
  onToggle: () => void;
  controls: string;
  label: string;
}

function NavItem({ route, current, section, badge, sub, fold }: { route: RouteDef; current: boolean; section?: boolean; badge?: number; sub?: boolean; fold?: Fold }) {
  const Icon = route.nav!.icon;
  return (
    <li className={cx(fold && "sidebar__head")}>
      <a className={cx("sidebar__item", sub && "sidebar__item--sub", current && "is-current", section && "is-section")} href={hrefFor(route.path)} aria-current={current ? "page" : undefined}>
        <Icon size={16} aria-hidden />
        <span className="sidebar__item-label">{route.nav!.label}</span>
        {badge != null && (
          <span className="sidebar__count" aria-label={`${badge} waiting`}>
            {badge}
          </span>
        )}
      </a>
      {fold && (
        <button type="button" className={cx("sidebar__fold", fold.open && "is-open")} aria-expanded={fold.open} aria-controls={fold.controls} aria-label={fold.label} onClick={fold.onToggle}>
          <ChevronDown size={14} aria-hidden />
        </button>
      )}
    </li>
  );
}
