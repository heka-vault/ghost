/**
 * Owner: watching. Pure helpers for the Watching screen and for Ghost's folder on the Agents
 * screen (ADR 0035): paths written from ~, words for counts and intervals, and revealing a
 * folder or opening a file through Tauri's opener (a browser build cannot, and says so).
 */
import type { HookInfo, ScheduleInfo, ScheduleRunInfo, WatchInfo } from "@ghost/client-ts";

/** Ghost's folder, as the design system writes it. */
export const CONFIG_DIR = "~/.config/ghost";

/** A path as the design system writes it: the operator's home collapsed to ~. */
export function tildePath(path: string | null | undefined): string {
  if (!path) return "";
  return path.replace(/^(?:\/home\/[^/]+|\/Users\/[^/]+|\/root)(?=\/|$)/, "~");
}

/** The folder a file is in: "~/.config/ghost" for "~/.config/ghost/AGENTS.md". */
export const folderOf = (path: string) => path.replace(/\/[^/]+$/, "") || path;

export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "a", "a and b", "a, b and c". */
export function listWords(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** "every 60 s", "every 5 min", "every 2 h". */
export function everyWords(secs: number): string {
  if (secs < 120) return `every ${secs} s`;
  if (secs < 7200) return `every ${Math.round(secs / 60)} min`;
  return `every ${Math.round(secs / 3600)} h`;
}

/** What a watch does with what it finds, in the operator's words. */
export const RESPONSE: Record<WatchInfo["response"], string> = { tell: "Tells you", record: "Keeps a record" };
/** How loud an item may get (ADR 0031). */
export const URGENCY: Record<WatchInfo["urgency"], string> = { digest: "in the summary", notify: "with a notification", call: "with a call" };

/** The steps a watch decides in, cheapest first: "a check, then Jev, then the model". */
export function stepWords(steps: string[]): string {
  const word: Record<string, string> = { check: "a check", jev: "Jev", model: "the model" };
  const out = steps.map((s) => word[s] ?? s);
  return out.length <= 1 ? (out[0] ?? "a check") : `${out.slice(0, -1).join(", then ")}, then ${out[out.length - 1]}`;
}

// --- schedules (ADR 0047) ---------------------------------------------------------------------

type RunStatus = ScheduleRunInfo["status"];
type BadgeTone = "ok" | "warn" | "danger" | "info" | "neutral";

/** How a run ended, in the operator's words, and the badge's tone. */
export const OUTCOME: Record<RunStatus, { label: string; tone: BadgeTone }> = {
  queued: { label: "Waiting to run", tone: "info" },
  running: { label: "Running", tone: "info" },
  ok: { label: "Done", tone: "ok" },
  unchanged: { label: "Nothing changed", tone: "neutral" },
  failed: { label: "Failed", tone: "danger" },
  timed_out: { label: "Ran out of time", tone: "danger" },
  cancelled: { label: "Stopped", tone: "neutral" },
  interrupted: { label: "Cut short", tone: "warn" },
  missed: { label: "Missed", tone: "warn" },
};

/** What came of a run's answer. */
export const DELIVERY: Record<string, string> = {
  delivered: "On What needs you",
  silent: "Nothing to tell",
  empty: "No answer",
  by_agent: "The agent told you itself",
  failure: "On What needs you",
  failure_not_told: "Not told",
  none: "",
};

/** “30m” as “30 minutes”; anything else as written. */
export function intervalWords(every: string): string {
  const m = /^(\d+)\s*([smhdw])$/i.exec(every.trim());
  if (!m) return every;
  const unit = { s: "second", m: "minute", h: "hour", d: "day", w: "week" }[m[2].toLowerCase()] ?? m[2];
  return count(Number(m[1]), unit);
}

/** When a schedule runs, in words: “Every 2 hours”, “Once, Dec 1 at 9:00 AM”, or the cron. */
export function whenWords(s: Pick<ScheduleInfo, "kind" | "at" | "every" | "cron" | "time_zone">, zone: string): { text: string; mono?: string } {
  if (s.kind === "every" && s.every) return { text: `Every ${intervalWords(s.every).replace(/^1 /, "")}` };
  if (s.kind === "at" && s.at) {
    const at = new Date(s.at);
    const day = at.toLocaleDateString("en-US", { month: "short", day: "numeric", year: at.getFullYear() === new Date().getFullYear() ? undefined : "numeric" });
    return { text: `Once, ${day} at ${at.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" })}` };
  }
  // A cron expression is an identifier: shown as written, with its zone when it isn't this machine's.
  return { text: s.time_zone === zone ? "On" : `In ${s.time_zone}, on`, mono: s.cron ?? "" };
}

/** A time to come: “in under a minute”, “in 48 min”, “in 14 h”, “tomorrow”, “in 3 days”, “Dec 1”. */
export function untilWords(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const mins = Math.round((t - now) / 60_000);
  if (mins <= 0) return "now";
  if (mins < 1) return "in under a minute";
  if (mins < 60) return `in ${mins} min`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `in ${hours} h`;
  const days = Math.round(hours / 24);
  if (days === 1) return "tomorrow";
  if (days < 7) return `in ${days} days`;
  return new Date(t).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

/** Who made a schedule: “you”, or the agent by its slug. */
export function madeBy(createdBy: string): { you: true } | { you: false; slug: string } {
  return createdBy.startsWith("agent:") ? { you: false, slug: createdBy.slice("agent:".length) } : { you: true };
}

/** The kinds a schedule can be, for the form: its label, an example and what to say under it. */
export const KINDS = {
  every: { label: "Every", placeholder: "30m", help: "An interval: 30m, 2h, 1d." },
  at: { label: "Once", placeholder: "2026-12-01T09:00", help: "A date and time, or a delay from now such as 20m." },
  cron: { label: "Cron", placeholder: "0 9 * * 1-5", help: "Five fields: minute, hour, day, month, weekday. 0 9 * * 1-5 is nine on weekdays." },
} as const;
export type ScheduleKind = keyof typeof KINDS;

/** How the webhook proves who sent it. */
export const VERIFY: Record<HookInfo["verify"], string> = { github: "GitHub signature", hmac: "Ghost’s own signature" };

/** Deliveries counted by outcome or reason, added up. */
export function sumOf(counts: Record<string, number>): number {
  return Object.values(counts).reduce((a, b) => a + b, 0);
}

/** "2 not signed, 1 too old": the refusals by reason, largest first. */
export function reasonWords(counts: Record<string, number>): string {
  return Object.entries(counts)
    .filter(([, n]) => n > 0)
    .sort((a, b) => b[1] - a[1])
    .map(([reason, n]) => `${n} ${reason.replace(/_/g, " ")}`)
    .join(", ");
}

export function formatBytes(n: number): string {
  return n < 1024 ? `${n} bytes` : n < 1024 * 1024 ? `${Math.round(n / 1024)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * Reveal a folder (or a file, selected) in the operator's file manager. Inside Tauri the
 * opener plugin does it; in a browser (`make desktop-web`) there is no way, so the caller
 * shows the path instead.
 */
export async function revealFolder(path: string): Promise<{ ok: true } | { ok: false; path: string }> {
  const shown = tildePath(path);
  if (!("__TAURI_INTERNALS__" in window)) return { ok: false, path: shown };
  try {
    const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
    await revealItemInDir(path);
    return { ok: true };
  } catch {
    return { ok: false, path: shown };
  }
}

/** Open a file Ghost made with the operator's default app (ADR 0033); a browser can't. */
export async function openFile(path: string): Promise<{ ok: true } | { ok: false; path: string }> {
  const shown = tildePath(path);
  if (!("__TAURI_INTERNALS__" in window)) return { ok: false, path: shown };
  try {
    const { openPath } = await import("@tauri-apps/plugin-opener");
    await openPath(path);
    return { ok: true };
  } catch {
    return { ok: false, path: shown };
  }
}
