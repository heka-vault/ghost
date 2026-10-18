/**
 * Scroll the page panel so `el` is in view, without `element.scrollIntoView()`.
 *
 * Chrome moves its sequential focus navigation starting point to whatever scrollIntoView
 * reveals, so the next Tab lands there instead of on "Skip to content". Use this for
 * programmatic scrolling (keeping a transcript pinned to its end, revealing a new row).
 * A click that should also move Tab order, like a table of contents, may still use
 * scrollIntoView. `block: "panel-end"` scrolls the whole panel to its end, so content under
 * a Page dock (the session composer) is in view. Smooth unless the operator prefers reduced motion.
 */
export function scrollIntoPanel(el: Element | null | undefined, opts: { block?: "start" | "end" | "nearest" | "panel-end"; smooth?: boolean } = {}) {
  if (!el) return;
  const scroller = (el.closest(".shell__main") as HTMLElement | null) ?? (document.scrollingElement as HTMLElement | null);
  if (!scroller) return;
  const { block = "nearest", smooth = true } = opts;
  const s = scroller.getBoundingClientRect();
  const r = el.getBoundingClientRect();
  const top = scroller === document.scrollingElement ? 0 : s.top;
  const view = scroller === document.scrollingElement ? window.innerHeight : scroller.clientHeight;
  const offset = r.top - top;
  let delta = 0;
  // "panel-end": the very bottom of the panel, past a Page dock (a transcript under its composer).
  if (block === "panel-end") delta = scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop;
  else if (block === "start") delta = offset;
  else if (block === "end") delta = offset + r.height - view;
  else if (offset < 0) delta = offset;
  else if (offset + r.height > view) delta = offset + r.height - view;
  if (Math.abs(delta) < 1) return;
  const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  scroller.scrollBy({ top: delta, behavior: smooth && !reduce ? "smooth" : "auto" });
}

/** Scroll the page panel to its top (a new wizard step, a fresh list). Instant unless `smooth`. */
export function scrollPanelToTop(opts: { smooth?: boolean } = {}) {
  const scroller = (document.getElementById("main") as HTMLElement | null) ?? (document.scrollingElement as HTMLElement | null);
  if (!scroller) return;
  const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  scroller.scrollTo({ top: 0, behavior: opts.smooth && !reduce ? "smooth" : "auto" });
}
