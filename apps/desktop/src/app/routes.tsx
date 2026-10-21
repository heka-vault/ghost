/**
 * The route registry. One entry per route; the shell builds navigation, document titles,
 * error boundaries and connection gating from it. Every screen is React.lazy, so a screen
 * that throws while loading or rendering takes down only itself.
 *
 * Owned by the foundation. To add a route, follow docs/design-system.md#adding-a-route.
 */
import {
  AudioLines,
  BookOpen,
  Briefcase,
  Ghost,
  Inbox,
  LayoutGrid,
  type LucideIcon,
  MessagesSquare,
  PanelTop,
  ScrollText,
  Server,
  Settings2,
  ShieldCheck,
  Smartphone,
  Sparkles,
} from "lucide-react";
import { type ComponentType, lazy, type LazyExoticComponent } from "react";

import { matchPath } from "./router";

export interface RouteProps {
  /** Path parameters, e.g. { slug: "legal" } for #/lines/legal. */
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
  nav?: { label: string; icon: LucideIcon; group: "main" | "footer" | "prototype" };
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
    nav: { label: "Session", icon: AudioLines, group: "main" },
    component: lazy(() => import("../screens/session")),
  },
  {
    id: "conversations",
    patterns: ["/lines/:slug?"],
    path: "/lines",
    owner: "conversations",
    title: "Lines",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Lines", icon: MessagesSquare, group: "main" },
    component: lazy(() => import("../screens/conversations")),
  },
  {
    id: "policy",
    patterns: ["/policy"],
    path: "/policy",
    owner: "policy",
    title: "What Ghost may do",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "What Ghost may do", icon: ShieldCheck, group: "main" },
    component: lazy(() => import("../screens/policy")),
  },
  {
    id: "audit",
    patterns: ["/audit"],
    path: "/audit",
    owner: "audit",
    title: "Audit log",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Audit log", icon: ScrollText, group: "main" },
    component: lazy(() => import("../screens/audit")),
  },
  {
    id: "servers",
    patterns: ["/servers"],
    path: "/servers",
    owner: "servers",
    title: "Servers",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Servers", icon: Server, group: "main" },
    component: lazy(() => import("../screens/servers")),
  },
  {
    id: "skills",
    patterns: ["/skills"],
    path: "/skills",
    owner: "skills",
    title: "Skills",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Skills", icon: BookOpen, group: "main" },
    component: lazy(() => import("../screens/skills")),
  },
  {
    id: "toolkits",
    patterns: ["/toolkits"],
    path: "/toolkits",
    owner: "toolkits",
    title: "Toolkits",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Toolkits", icon: Briefcase, group: "main" },
    component: lazy(() => import("../screens/toolkits")),
  },
  {
    id: "settings",
    patterns: ["/settings"],
    path: "/settings",
    owner: "settings",
    title: "Settings",
    kind: "screen",
    chrome: "app",
    needsDaemon: true,
    nav: { label: "Settings", icon: Settings2, group: "footer" },
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

export interface ResolvedRoute {
  route: RouteDef;
  params: Record<string, string>;
}

/** The route for a path, or null for a 404. Routes absent from this build resolve to null. */
export function resolve(path: string): ResolvedRoute | null {
  for (const route of ROUTES) {
    if (!route.component) continue;
    for (const p of route.patterns) {
      const params = matchPath(p, path);
      if (params) return { route, params };
    }
  }
  return null;
}

/** The route whose nav item should look selected for a path (prefix match). */
export function navMatch(path: string): string | null {
  const r = resolve(path);
  return r?.route.id ?? null;
}
