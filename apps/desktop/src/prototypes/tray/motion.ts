/**
 * Owner: tray. Durations for timers that have to agree with CSS: read the token from the
 * element, so reduced motion (every duration token is 0ms there) applies to JS as well.
 */
export function tokenMs(el: Element | null, name: string): number {
  if (!el) return 0;
  const v = getComputedStyle(el).getPropertyValue(name).trim();
  const n = parseFloat(v);
  return Number.isFinite(n) ? (v.endsWith("ms") ? n : n * 1000) : 0;
}
