/**
 * Owner: watching. Pure helpers for the Watching screen and for Ghost's folder on the Agents
 * screen (ADR 0035): paths written from ~, words for counts and intervals, and revealing a
 * folder or opening a file through Tauri's opener (a browser build cannot, and says so).
 */
import type { HookInfo, WatchInfo } from "@ghost/client-ts";

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
