/**
 * Hash routing. URLs look like `/?mock=session#/agents/legal`: the query (mock
 * scenario) survives navigation, the hash is the route. Tauri serves a single file, so
 * hash routing needs no server support.
 */
import { type AnchorHTMLAttributes, useSyncExternalStore } from "react";

let last = "/";

/**
 * Only hashes that start with "/" are routes. Anything else ("#main", "#gal-color") is an
 * in-page anchor and leaves the current route alone, so no anchor can navigate.
 */
function read(): string {
  const h = window.location.hash.replace(/^#/, "");
  if (!h || h === "/") return (last = "/");
  if (!h.startsWith("/")) return last;
  return (last = h.replace(/\/+$/, "") || "/");
}

function subscribe(cb: () => void) {
  window.addEventListener("hashchange", cb);
  return () => window.removeEventListener("hashchange", cb);
}

/** The current route path, e.g. "/conversations/legal". */
export function usePath(): string {
  return useSyncExternalStore(subscribe, read, () => "/");
}

export function navigate(path: string, opts: { replace?: boolean } = {}) {
  const url = `${window.location.pathname}${window.location.search}#${path}`;
  if (opts.replace) {
    window.history.replaceState(null, "", url);
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  } else {
    window.location.hash = path;
  }
}

export function hrefFor(path: string): string {
  return `#${path}`;
}

/**
 * Match "/conversations/:slug?" against a path. Returns params, or null.
 * `:name` is one segment; a trailing `?` makes it optional.
 */
export function matchPath(pattern: string, path: string): Record<string, string> | null {
  const p = pattern.split("/").filter(Boolean);
  const s = path.split("/").filter(Boolean);
  const params: Record<string, string> = {};
  if (s.length > p.length) return null;
  for (let i = 0; i < p.length; i++) {
    const seg = p[i];
    const optional = seg.endsWith("?");
    const name = seg.replace(/^:/, "").replace(/\?$/, "");
    if (i >= s.length) {
      if (optional) continue;
      return null;
    }
    if (seg.startsWith(":")) params[name] = decodeURIComponent(s[i]);
    else if (seg !== s[i]) return null;
  }
  return params;
}

export interface LinkProps extends AnchorHTMLAttributes<HTMLAnchorElement> {
  to: string;
}

/** An anchor to a route. Marks itself aria-current="page" when the route is exact. */
export function Link({ to, ...rest }: LinkProps) {
  const path = usePath();
  return <a href={hrefFor(to)} aria-current={path === to ? "page" : undefined} {...rest} />;
}
