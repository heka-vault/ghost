/**
 * Owner: conversations. Page state that lives in the query string, so every state has a
 * URL (`?mock=session&tab=memory#/conversations/accounting`). The hash is the route and
 * takes one segment for the slug, so views and dialogs ride in the search part instead.
 *
 * The params belong to one line at a time. A direct load honors them for whichever line
 * the hash names; once a line has claimed them, another line starts clean (shared request:
 * a `/:slug/:tab` route segment would make this unnecessary).
 */
const PAGE_PARAMS = ["tab", "open"];
let owner: string | null = null;

export function readParam(name: string, forSlug?: string): string | null {
  if (forSlug !== undefined && owner !== null && owner !== forSlug) return null;
  return new URLSearchParams(window.location.search).get(name);
}

export function writeParam(name: string, value: string | null, forSlug?: string) {
  if (forSlug !== undefined) owner = forSlug;
  const q = new URLSearchParams(window.location.search);
  if (value == null) q.delete(name);
  else q.set(name, value);
  const s = q.toString();
  const url = `${window.location.pathname}${s ? `?${s}` : ""}${window.location.hash}`;
  window.history.replaceState(window.history.state, "", url);
}

/** A line's page opened: stale params left by another line are dropped from the URL. */
export function claimParams(slug: string) {
  if (owner !== null && owner !== slug) for (const p of PAGE_PARAMS) writeParam(p, null);
  owner = slug;
}

/** Leaving the conversations screen: nothing in the query string should outlive it. */
export function clearParams() {
  for (const p of PAGE_PARAMS) writeParam(p, null);
  owner = null;
}

/**
 * Where focus goes when a line's page opens, once: set before navigating to a line just
 * created, so keyboard and screen-reader users land in its persona instead of on <body>.
 */
export const focusOnOpen: { slug: string | null } = { slug: null };

/**
 * A settled line to start the next one from (ADR 0019): set by a settled line's Memory tab
 * before navigating to the index, which opens New line with that line preselected, once.
 */
export const newLineSeed: { slug: string | null } = { slug: null };

/**
 * Design mode only (`&fail=lines` or `&fail=line`): that page's loads fail until Try again
 * is pressed, so the failed state can be seen and screenshotted. Try again then loads.
 */
const retried = new Set<string>();
export function designFail(design: unknown, what: "lines" | "line"): boolean {
  return !!design && readParam("fail") === what && !retried.has(what);
}
export function designRetry(what: "lines" | "line") {
  retried.add(what);
}
