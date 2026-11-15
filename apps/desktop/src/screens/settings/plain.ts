/**
 * Owner: settings. The plain logic of the simpler Settings (ADR 0073, ADR 0074), with no React
 * and no window, so tests run it in Node.
 */
import type { RoutesView, Status } from "@ghost/client-ts";

/** Minutes and seconds left until `until`, for the countdown. */
export function timeLeft(until: string | null | undefined, now: number): string | null {
  if (!until) return null;
  const ms = Date.parse(until) - now;
  if (!Number.isFinite(ms) || ms <= 0) return null;
  const m = Math.floor(ms / 60_000);
  const s = Math.floor((ms % 60_000) / 1000);
  return m >= 1 ? `${m} min ${String(s).padStart(2, "0")} s` : `${s} s`;
}

/** "42 min left", "under a minute left", or "on until Ghost stops" for the variable's. */
export function debugWords(mark: Status["debug_audio"], now: number): string | null {
  if (!mark) return null;
  if (!mark.until) return "on until Ghost stops";
  const ms = Date.parse(mark.until) - now;
  if (!Number.isFinite(ms) || ms <= 0) return "ending";
  const m = Math.ceil(ms / 60_000);
  return m <= 1 ? "under a minute left" : `${m} min left`;
}

export type Route = RoutesView["enabled"][number];

/** The everyday ways, as switches here; the others are on the detailed page. */
export const EVERYDAY: { name: Route; title: string; help: string }[] = [
  { name: "desk", title: "Ghost asks at the desk", help: "A chime and a greeting at your computer, then it waits for your reply." },
  { name: "desktop", title: "Desktop notification", help: "On this computer; “Got it” in it counts as heard." },
  { name: "phone", title: "The phone app", help: "The list on your phone, and a notification while the app is open." },
];

/** What the routes do, in a sentence or two. */
export function reachSentence(r: RoutesView): string {
  const on = EVERYDAY.filter((x) => r.enabled.includes(x.name)).map((x) => x.title.replace(/^Ghost /, "").toLowerCase());
  const extra = r.enabled.filter((n) => !["digest", "desk", "desktop", "phone"].includes(n));
  const ways = on.length ? ` If you don’t hear about it, it ${on.length === 1 ? "uses" : "tries, in order:"} ${on.join(", ")}` : " It doesn’t reach out otherwise";
  const more = extra.length ? `, and ${extra.length} more way${extra.length === 1 ? "" : "s"} you set up` : "";
  const quiet = r.quiet_hours ? ` Quiet hours: ${r.quiet_hours.replace("-", " to ")}, when only the quiet ways are used.` : "";
  return `When something needs you, Ghost tells you the next time you wake it.${ways}${more}.${quiet}`;
}

