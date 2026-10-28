/**
 * The route registry. One entry per route; the shell builds navigation, document titles,
 * error boundaries and connection gating from it. Every screen is React.lazy, so a screen
 * that throws while loading or rendering takes down only itself.
 *
 * Owned by the foundation. To add a route, follow docs/design-system.md#adding-a-route.
 */
import {
  BellRing,
  Blocks,
  BookOpen,
  Cpu,
  Eye,
  Ghost,
  Inbox,
  KeyRound,
  LayoutGrid,
  type LucideIcon,
  MessagesSquare,
  PanelTop,
  ScrollText,
  Settings,
  Settings2,
  ShieldCheck,
  Smartphone,
  Sparkles,
  Volume2,
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
    id: "session",
    patterns: ["/"],
    path: "/",
    owner: "session",
    title: "Session",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    // No row of its own: the sidebar's status card is the way here.
    component: lazy(() => import("../screens/session")),
  },
  {
    id: "waiting",
    patterns: ["/waiting"],
    path: "/waiting",
    owner: "waiting",
    title: "What needs you",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "What needs you", icon: Inbox, group: "main" },
    component: lazy(() => import("../screens/waiting")),
  },
  {
    id: "conversations",
    patterns: ["/agents/:slug?"],
    path: "/agents",
    owner: "conversations",
    title: "Agents",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Agents", icon: MessagesSquare, group: "main" },
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
    patterns: ["/settings/apps/:name?"],
    path: "/settings/apps",
    owner: "apps",
    title: "Apps",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    // A page of Settings that keeps a one-click row in the main nav (the operator's call, 2026-10-28).
    nav: { label: "Apps", icon: Blocks, group: "main" },
    area: "settings",
    component: lazy(() => import("../screens/apps")),
  },
  {
    id: "skills",
    patterns: ["/settings/skills"],
    path: "/settings/skills",
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
    patterns: ["/settings/permissions"],
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
    patterns: ["/settings/notifications"],
    path: "/settings/notifications",
    owner: "routes",
    title: "Notifications",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    area: "settings",
    component: lazy(() => import("../screens/routes")),
  },
  {
    id: "audit",
    patterns: ["/settings/audit"],
    path: "/settings/audit",
    owner: "audit",
    title: "Audit log",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    area: "settings",
    component: lazy(() => import("../screens/audit")),
  },
  {
    // General, Model, Passwords, Sound and Phone: one screen, so moving between them keeps
    // one draft (a typed password included) and one save bar.
    id: "settings",
    patterns: ["/settings/:page"],
    path: "/settings/general",
    owner: "settings",
    title: "Settings",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Settings", icon: Settings2, group: "footer" },
    area: "settings",
    accept: (p) => SETTINGS_SCREEN_PAGES.includes(p.page),
    component: lazy(() => import("../screens/settings")),
  },
  // ------------------------------------------------------------- prototypes (dev + mock only)
  {
    id: "welcome",
    patterns: ["/welcome"],
    path: "/welcome",
    owner: "onboarding",
    title: "Welcome",
    kind: "prototype",
    chrome: "app",
    needsDaemon: false,
    nav: { label: "Welcome", icon: Sparkles, group: "prototype" },
    component: DEV ? lazy(() => import("../prototypes/onboarding")) : null,
  },
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
 * 2026-10-28), in this order. Each page is `#/settings/<id>`; `#/settings` is the first.
 */
export interface SettingsPage {
  id: string;
  label: string;
  icon: LucideIcon;
  path: string;
}
const page = (id: string, label: string, icon: LucideIcon): SettingsPage => ({ id, label, icon, path: `/settings/${id}` });
export const SETTINGS_PAGES: SettingsPage[] = [
  page("general", "General", Settings),
  page("model", "Model", Cpu),
  page("passwords", "Passwords", KeyRound),
  page("apps", "Apps", Blocks),
  page("skills", "Skills", BookOpen),
  page("permissions", "Permissions", ShieldCheck),
  page("sound", "Sound", Volume2),
  page("phone", "Phone", Smartphone),
  page("notifications", "Notifications", BellRing),
  page("audit", "Audit log", ScrollText),
];
/** The pages the settings screen itself draws; the rest are their owners' screens. */
export const SETTINGS_SCREEN_PAGES = ["general", "model", "passwords", "sound", "phone"];

/** The settings page a path is on ("sound" for /settings/sound, "apps" for /settings/apps/github), or null. */
export function settingsPageOf(path: string): SettingsPage | null {
  const [, area, id] = path.split("/");
  return area === "settings" ? (SETTINGS_PAGES.find((p) => p.id === id) ?? null) : null;
}

/**
 * Old addresses and where they live now, so links in notes, docs and bookmarks keep working.
 * The shell replaces the hash; the query before it (a mock scenario, an audit filter) stays.
 */
const REDIRECTS: { from: string; to: (p: Record<string, string>) => string }[] = [
  { from: "/settings", to: () => SETTINGS_PAGES[0].path },
  { from: "/apps/:name?", to: (p) => (p.name ? `/settings/apps/${encodeURIComponent(p.name)}` : "/settings/apps") },
  // #/servers and #/policy were these screens' addresses before ADR 0034.
  { from: "/servers", to: () => "/settings/apps" },
  { from: "/skills", to: () => "/settings/skills" },
  { from: "/permissions", to: () => "/settings/permissions" },
  { from: "/policy", to: () => "/settings/permissions" },
  { from: "/routes", to: () => "/settings/notifications" },
  { from: "/audit", to: () => "/settings/audit" },
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
