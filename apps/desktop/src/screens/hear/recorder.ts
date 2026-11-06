/**
 * Owner: hear. One line for every call that moves the recorder (open, close, a take's start,
 * stop and cancel), shared by every mount of the screen: React's StrictMode mounts, unmounts
 * and mounts again at once, and a release can come before the start it ends has answered, so
 * each call waits for the one before it. A call that fails doesn't stop the next.
 */
let line: Promise<unknown> = Promise.resolve();

export function serial<T>(f: () => Promise<T>): Promise<T> {
  const p = line.then(f, f);
  line = p.catch(() => undefined);
  return p;
}

let closing: ReturnType<typeof setTimeout> | null = null;

/** Close the recorder in a moment, unless an open comes first (see cancelClose). */
export function closeSoon(close: () => void) {
  if (closing) clearTimeout(closing);
  closing = setTimeout(() => {
    closing = null;
    close();
  }, 120);
}

/** An open is about to follow: the close waiting for it is not needed. */
export function cancelClose() {
  if (closing) clearTimeout(closing);
  closing = null;
}

/** Where focus is typing, so Space and the single keys belong to it. */
export function isTyping(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  if (!el || typeof el.closest !== "function") return false;
  return !!el.closest("input, textarea, select, [contenteditable=''], [contenteditable='true']");
}

/** Where Enter already does something (a button, a link, a tab). */
export function isActivatable(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  if (!el || typeof el.closest !== "function") return false;
  return !!el.closest("button, a[href], [role='tab'], [role='menuitem'], summary");
}

/** A dialog is open over the page: keys belong to it. */
export function dialogOpen(): boolean {
  return !!document.querySelector("dialog[open]");
}

export function prefersReducedMotion(): boolean {
  return typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}
