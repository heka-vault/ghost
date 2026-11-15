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

/** "22:00" in the person's own clock ("10 PM", or "22:00" where the locale says so). */
export function clockWords(hhmm: string, locale?: string): string {
  const m = /^(\d{1,2}):(\d{2})$/.exec(hhmm.trim());
  if (!m) return hhmm;
  const d = new Date(2000, 0, 1, Number(m[1]), Number(m[2]));
  // A 12-hour clock drops “:00” (“10 PM”); a 24-hour clock keeps it (“22:00”).
  const twelve = new Intl.DateTimeFormat(locale, { hour: "numeric" }).resolvedOptions().hour12;
  return d.toLocaleTimeString(locale, twelve ? { hour: "numeric", minute: m[2] === "00" ? undefined : "2-digit" } : { hour: "2-digit", minute: "2-digit" });
}

/** What the routes do, in a sentence or two. Times in the locale's clock. */
export function reachSentence(r: RoutesView, locale?: string): string {
  const on = EVERYDAY.filter((x) => r.enabled.includes(x.name)).map((x) => x.title.replace(/^Ghost /, "").toLowerCase());
  const extra = r.enabled.filter((n) => !["digest", "desk", "desktop", "phone"].includes(n));
  const ways = on.length ? ` If you don’t hear about it, it ${on.length === 1 ? "uses" : "tries, in order:"} ${on.join(", ")}` : " It doesn’t reach out otherwise";
  const more = extra.length ? `, and ${extra.length} more way${extra.length === 1 ? "" : "s"} you set up` : "";
  const [from, to] = (r.quiet_hours ?? "").split("-");
  const quiet = r.quiet_hours && to ? ` Quiet from ${clockWords(from, locale)} to ${clockWords(to, locale)}, when only the quiet ways are used.` : "";
  return `When something needs you, Ghost tells you the next time you wake it.${ways}${more}.${quiet}`;
}


/* ------------------------------------------------------------------ devices, in words */

/** Ghost's own echo-cancelled input (infra/pipewire), as a person would name it. */
export const ECHO_MIC_WORDS = "Your microphone, without Ghost’s voice";

/**
 * A sound device as the system's own name for it, without the ids under it (the flow review,
 * 2026-11-14: `ghost.mic`, `hw:2,0` and `#0` on Sound): `Shure MV7+: USB Audio (hw:2,0)` is
 * “Shure MV7+”, the board's own card “Built-in audio”, `ghost.mic` the echo-cancelled
 * microphone, the sound servers by name, and a first channel said in words. The raw name stays
 * the value, and the Detected devices disclosure shows it.
 */
export function deviceWords(name: string | null | undefined): string {
  const raw = (name ?? "").trim();
  if (!raw) return "System default";
  const [base, channel] = raw.split("#");
  const ch = channel !== undefined ? `, channel ${Number(channel) + 1} alone` : "";
  if (base === "ghost.mic") return ECHO_MIC_WORDS + ch;
  if (base === "pipewire") return `The sound server (PipeWire)${ch}`;
  if (base === "pulse") return `The sound server (PulseAudio)${ch}`;
  if (base === "default") return `System default${ch}`;
  const noHw = base.replace(/\s*\(hw:\d+(?:,\d+)?\)\s*$/, "");
  const [card, rest] = noHw.split(/:\s+/, 2);
  if (/^(HDA Intel|HD-Audio|sof-hda|PCH)/i.test(card)) return `Built-in audio${rest ? ` (${rest.replace(/\s*Analog$/i, "")})` : ""}${ch}`;
  return `${card || noHw}${ch}`;
}

/** A sentence from Ghost that names devices by id: the ids said in words (`(ghost.mic)` dropped, `hw:` gone). */
export function plainDeviceText(text: string): string {
  return text
    .replace(/\s*\((?:ghost\.mic|hw:\d+(?:,\d+)?)\)/g, "")
    .replace(/\bghost\.mic\b/g, ECHO_MIC_WORDS.replace(/^Your/, "your"));
}
