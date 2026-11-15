/**
 * The route registry. One entry per route; the shell builds navigation, document titles,
 * error boundaries and connection gating from it. Every screen is React.lazy, so a screen
 * that throws while loading or rendering takes down only itself.
 *
 * Owned by the foundation. To add a route, follow docs/design-system.md#adding-a-route.
 */
import {
  Activity,
  AudioWaveform,
  BellRing,
  Blocks,
  BookOpen,
  Cpu,
  Eye,
  Ghost,
  Inbox,
  Info,
  KeyRound,
  LayoutGrid,
  type LucideIcon,
  PanelTop,
  PlugZap,
  ScrollText,
  Settings,
  Settings2,
  ShieldCheck,
  Smartphone,
  Sun,
  Volume2,
  Wrench,
} from "lucide-react";
import { type ComponentType, lazy, type LazyExoticComponent } from "react";

import { matchPath } from "./router";

export interface RouteProps {
  /** Path parameters, e.g. { slug: "legal" } for #/agents/legal. */
  params: Record<string, string>;
}

export type RouteKind = "screen" | "prototype" | "foundation";

export interface RouteDef {
  id: string;
  /** Patterns tried in order; `:name` is a segment, `:name?` optional. */
  patterns: string[];
  /** Path used for links to this route. */
  path: string;
  owner: string;
  /** Document title and default nav label. */
  title: string;
  kind: RouteKind;
  /** `app`: inside the sidebar shell. `bare`: rendered alone (tray popover, phone frames). */
  chrome: "app" | "bare";
  /** Show the "Ghost isn't running" state instead of the screen when the daemon is down. */
  needsDaemon: boolean;
  /** `footer`: the pinned Settings row. */
  nav?: { label: string; icon: LucideIcon; group: "main" | "footer" | "prototype" };
  /** A page of Settings: the shell puts the settings nav beside it (SETTINGS_PAGES). */
  area?: "settings";
  /** Narrows a pattern's match, for a `:param` that takes only some values. */
  accept?: (params: Record<string, string>) => boolean;
  /** Null when the route is not part of this build (prototypes in production). */
  component: LazyExoticComponent<ComponentType<RouteProps>> | null;
}

const DEV = import.meta.env.DEV;

export const ROUTES: RouteDef[] = [
  {
    // Today: the briefing, the desk's home (docs/design/overhaul-2026-11.md, the combined direction).
    id: "today",
    patterns: ["/"],
    path: "/",
    owner: "today",
    title: "Today",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Today", icon: Sun, group: "main" },
    component: lazy(() => import("../screens/today")),
  },
  {
    // The conversation screen: Ghost's own page, where every wake lands. The rail's presence is the way here.
    id: "session",
    patterns: ["/session"],
    path: "/session",
    owner: "session",
    title: "Conversation",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    component: lazy(() => import("../screens/session")),
  },
  {
    id: "conversations",
    // The conversation with one agent; the rail lists them (the owner, 2026-11-12).
    patterns: ["/agents/:slug"],
    path: "/agents/ghost",
    owner: "conversations",
    title: "Agent",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    component: lazy(() => import("../screens/conversations")),
  },
  {
    id: "agents",
    // Every agent, and each one's own page (its profile: name, look, voice, job, memory, what
    // it does on its own, its activity). Out of Settings since 2026-11-14 (ADR 0072): an
    // agent's settings live on the agent. The rail's agents and its + lead here.
    patterns: ["/agents", "/agents/:slug/settings"],
    path: "/agents",
    owner: "conversations",
    title: "Agents",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    component: lazy(() => import("../screens/conversations")),
  },
  {
    id: "watching",
    // #/departments was this slot before ADR 0035; it, and anything under it, lands on #/watching.
    patterns: ["/watching", "/departments/:slug?/:more?"],
    path: "/watching",
    owner: "watching",
    title: "Watching",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Watching", icon: Eye, group: "main" },
    component: lazy(() => import("../screens/watching")),
  },
  {
    id: "apps",
    // Its own place, not a page of Settings: too important to hide there (the owner, 2026-11-12).
    patterns: ["/apps/:name?"],
    path: "/apps",
    owner: "apps",
    title: "Apps",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Apps", icon: Blocks, group: "main" },
    component: lazy(() => import("../screens/apps")),
  },
  {
    id: "skills",
    patterns: ["/settings/developer/skills"],
    path: "/settings/developer/skills",
    owner: "skills",
    title: "Skills",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    area: "settings",
    component: lazy(() => import("../screens/skills")),
  },
  {
    id: "policy",
    // `/settings/permissions/advanced` opens with Advanced open (Developer links there).
    patterns: ["/settings/permissions/:section?"],
    accept: (p) => !p.section || p.section === "advanced",
    path: "/settings/permissions",
    owner: "policy",
    title: "Permissions",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    area: "settings",
    component: lazy(() => import("../screens/policy")),
  },
  {
    id: "routes",
    patterns: ["/settings/developer/notifications"],
    path: "/settings/developer/notifications",
    owner: "routes",
    title: "Notification routes",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    area: "settings",
    component: lazy(() => import("../screens/routes")),
  },
  {
    id: "audit",
    patterns: ["/settings/developer/audit"],
    path: "/settings/developer/audit",
    owner: "audit",
    title: "Audit log",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    area: "settings",
    component: lazy(() => import("../screens/audit")),
  },
  {
    // The Hearing test (ADR 0042): record takes, check each, score how well Ghost hears.
    id: "hear",
    patterns: ["/hear/:tab?/:id?"],
    path: "/hear",
    owner: "hear",
    title: "Hearing test",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Hearing test", icon: AudioWaveform, group: "main" },
    accept: (p) => !p.tab || (["sessions", "runs", "plan", "wake"].includes(p.tab) && (!p.id || p.tab === "runs")),
    component: lazy(() => import("../screens/hear")),
  },
  {
    // General, Model, Sound, About, Developer, and under them Phone, Passwords and Outside tools: one
    // screen, so moving between them keeps one draft (a typed password included) and one save bar.
    id: "settings",
    // `/settings/sound/audio` lands on a section, as `?section=audio` does: a waiting note's place.
    patterns: ["/settings/:page/:section?"],
    path: "/settings/general",
    owner: "settings",
    title: "Settings",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Settings", icon: Settings2, group: "footer" },
    area: "settings",
    accept: (p) => SETTINGS_SCREEN_PAGES.includes(p.page) && (!p.section || /^[a-z-]+$/.test(p.section)),
    component: lazy(() => import("../screens/settings")),
  },
  {
    // The first run (2026-11-12): from nothing to a first spoken answer. Not on the rail; the
    // shell's "Finish setup" and Settings › General lead here.
    id: "welcome",
    patterns: ["/welcome"],
    path: "/welcome",
    owner: "onboarding",
    title: "Set up Ghost",
    kind: "screen",
    chrome: "app",
    needsDaemon: false,
    component: lazy(() => import("../screens/onboarding")),
  },
  // ------------------------------------------------------------- prototypes (dev + mock only)
  {
    id: "review",
    patterns: ["/review"],
    path: "/review",
    owner: "review",
    title: "Review queue",
    kind: "prototype",
    chrome: "app",
    needsDaemon: false,
    nav: { label: "Review queue", icon: Inbox, group: "prototype" },
    component: DEV ? lazy(() => import("../prototypes/review")) : null,
  },
  {
    id: "tray",
    patterns: ["/tray"],
    path: "/tray",
    owner: "tray",
    title: "Tray",
    kind: "prototype",
    chrome: "bare",
    needsDaemon: false,
    nav: { label: "Tray", icon: PanelTop, group: "prototype" },
    component: DEV ? lazy(() => import("../prototypes/tray")) : null,
  },
  {
    id: "mobile",
    patterns: ["/mobile/:screen?"],
    path: "/mobile",
    owner: "mobile",
    title: "Mobile",
    kind: "prototype",
    chrome: "bare",
    needsDaemon: false,
    nav: { label: "Mobile", icon: Smartphone, group: "prototype" },
    component: DEV ? lazy(() => import("../prototypes/mobile")) : null,
  },
  {
    id: "mascot",
    patterns: ["/mascot/:view?"],
    path: "/mascot",
    owner: "mascot",
    title: "Mascot",
    kind: "prototype",
    chrome: "app",
    needsDaemon: false,
    nav: { label: "Mascot", icon: Ghost, group: "prototype" },
    component: DEV ? lazy(() => import("../prototypes/mascot")) : null,
  },
  { id: "concepts", patterns: ["/concepts/:name?"], path: "/concepts/instrument", owner: "design", title: "Concepts", kind: "prototype", chrome: "bare", needsDaemon: false, component: DEV ? lazy(() => import("../design/concepts")) : null }, // Two far concepts for review only (docs/design/concepts-2026-11.md).
  {
    id: "gallery",
    patterns: ["/gallery"],
    path: "/gallery",
    owner: "foundation",
    title: "Gallery",
    kind: "foundation",
    chrome: "app",
    needsDaemon: false,
    nav: { label: "Gallery", icon: LayoutGrid, group: "prototype" },
    component: DEV ? lazy(() => import("../screens/gallery")) : null,
  },
];

/**
 * Settings: one place with a page per topic and its own nav (the operator's call,
 * 2026-10-28). Since 2026-11-14 (ADR 0072) five pages a newcomer sees, in this order, and a
 * sixth, Developer, once the version on About has been clicked seven times. Each page is
 * `#/settings/<id>`; `#/settings` is the first.
 *
 * Under a page, a sub-page keeps its parent's place in the nav: Phone under General, and the
 * advanced pages (Passwords, Skills, Outside tools, the Audit log, notification routes) under
 * Developer, each at `#/settings/developer/<id>`. A sub-page is reachable by its address and
 * from where it is used whether or not Developer is in the nav: Developer is not a lock.
 */
export interface SettingsPage {
  id: string;
  label: string;
  icon: LucideIcon;
  path: string;
  /** A sub-page: the nav marks this page as the current one. */
  parent?: string;
}
const page = (id: string, label: string, icon: LucideIcon): SettingsPage => ({ id, label, icon, path: `/settings/${id}` });
const sub = (parent: string, id: string, label: string, icon: LucideIcon): SettingsPage => ({
  id,
  label,
  icon,
  parent,
  path: parent === "developer" ? `/settings/developer/${id}` : `/settings/${id}`,
});
/** The pages in the nav for everyone. */
export const SETTINGS_PAGES: SettingsPage[] = [
  page("general", "General", Settings),
  page("model", "Model", Cpu),
  page("sound", "Sound", Volume2),
  page("permissions", "Permissions", ShieldCheck),
  page("about", "About", Info),
];
/** The page the version's seven clicks add to the nav (app/developer.ts). */
export const DEVELOPER_PAGE: SettingsPage = page("developer", "Developer", Wrench);
/** The sub-pages: in no nav, each under its parent. */
export const SETTINGS_SUBPAGES: SettingsPage[] = [
  sub("general", "phone", "Phone", Smartphone),
  sub("developer", "passwords", "Passwords", KeyRound),
  sub("developer", "skills", "Skills", BookOpen),
  sub("developer", "tools", "Outside tools", PlugZap),
  sub("developer", "audit", "Audit log", ScrollText),
  sub("developer", "notifications", "Notification routes", BellRing),
  sub("developer", "live", "Live views", Activity),
];
/** The pages the settings screen itself draws; the rest are their owners' screens. */
export const SETTINGS_SCREEN_PAGES = ["general", "model", "sound", "about", "developer", "phone"];
/** The developer sub-pages the settings screen draws (`/settings/developer/<id>`). */
export const DEVELOPER_SCREEN_PAGES = ["passwords", "tools", "live"];

/**
 * The settings page a path is on, sub-pages included ("sound" for /settings/sound,
 * "passwords" for /settings/developer/passwords), or null.
 */
export function settingsPageOf(path: string): SettingsPage | null {
  const [, area, id, more] = path.split("/");
  if (area !== "settings") return null;
  if (id === "developer" && more) return SETTINGS_SUBPAGES.find((p) => p.parent === "developer" && p.id === more) ?? DEVELOPER_PAGE;
  if (id === "developer") return DEVELOPER_PAGE;
  return SETTINGS_PAGES.find((p) => p.id === id) ?? SETTINGS_SUBPAGES.find((p) => p.parent !== "developer" && p.id === id) ?? null;
}

/** The address of a settings page or sub-page by its id ("passwords" → /settings/developer/passwords). */
export function settingsPath(id: string): string {
  return [...SETTINGS_PAGES, DEVELOPER_PAGE, ...SETTINGS_SUBPAGES].find((p) => p.id === id)?.path ?? `/settings/${id}`;
}

/** The nav item a settings page belongs under: itself, or its parent. */
export function settingsNavId(p: SettingsPage): string {
  return p.parent ?? p.id;
}

/**
 * Old addresses and where they live now, so links in notes, docs and bookmarks keep working.
 * The shell replaces the hash; the query before it (a mock scenario, an audit filter) stays.
 */
export const REDIRECTS: { from: string; to: (p: Record<string, string>) => string }[] = [
  { from: "/settings", to: () => SETTINGS_PAGES[0].path },
  // The simpler Settings (ADR 0072, 2026-11-14): eleven pages became five, and Developer.
  { from: "/settings/agents", to: () => "/agents" },
  { from: "/settings/agents/:slug", to: (p) => `/agents/${encodeURIComponent(p.slug)}/settings` },
  { from: "/settings/passwords/:section?", to: () => "/settings/developer/passwords" },
  { from: "/settings/skills", to: () => "/settings/developer/skills" },
  { from: "/settings/tools/:section?", to: () => "/settings/developer/tools" },
  { from: "/settings/audit", to: () => "/settings/developer/audit" },
  { from: "/settings/notifications", to: () => "/settings/developer/notifications" },
  // The version, its build and the updates moved from General to About.
  { from: "/settings/general/about", to: () => "/settings/about" },
  { from: "/settings/general/updates", to: () => "/settings/about/updates" },
  // What needs you is Today's first block now (the combined direction, 2026-11-12).
  { from: "/waiting", to: () => "/" },
  // Apps was a page of Settings until the owner moved it out (2026-11-12).
  { from: "/settings/apps/:name?", to: (p) => (p.name ? `/apps/${encodeURIComponent(p.name)}` : "/apps") },
  // #/servers and #/policy were these screens' addresses before ADR 0034.
  { from: "/servers", to: () => "/apps" },
  { from: "/skills", to: () => "/settings/developer/skills" },
  { from: "/permissions", to: () => "/settings/permissions" },
  { from: "/policy", to: () => "/settings/permissions" },
  { from: "/routes", to: () => "/settings/developer/notifications" },
  { from: "/audit", to: () => "/settings/developer/audit" },
];

/** Where an old address lives now, or null when the path is current. */
export function redirectFor(path: string): string | null {
  for (const r of REDIRECTS) {
    const params = matchPath(r.from, path);
    if (params) return r.to(params);
  }
  return null;
}

export interface ResolvedRoute {
  route: RouteDef;
  params: Record<string, string>;
}

/**
 * The route for a path, or null for a 404. Routes absent from this build resolve to null. An
 * old address resolves to the route it redirects to, so nothing flashes while the shell
 * replaces the hash.
 */
export function resolve(path: string): ResolvedRoute | null {
  const to = redirectFor(path) ?? path;
  for (const route of ROUTES) {
    if (!route.component) continue;
    for (const p of route.patterns) {
      const params = matchPath(p, to);
      if (params && (!route.accept || route.accept(params))) return { route, params };
    }
  }
  return null;
}

/** The route whose nav item should look selected for a path (prefix match). */
export function navMatch(path: string): string | null {
  const r = resolve(path);
  return r?.route.id ?? null;
}
