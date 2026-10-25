/**
 * Owner: departments. Pure helpers for the departments screens (ADR 0024): paths written
 * from ~, words for counts, and revealing a folder in the file manager through Tauri's
 * opener (a browser build cannot, and says so).
 */
import type { DepartmentView } from "@ghost/client-ts";

/** A path as the design system writes it: the operator's home collapsed to ~. */
export function tildePath(path: string | null | undefined): string {
  if (!path) return "";
  return path.replace(/^(?:\/home\/[^/]+|\/Users\/[^/]+|\/root)(?=\/|$)/, "~");
}

export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "a", "a and b", "a, b and c". */
export function listWords(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** What a department reaches, in a line: "2 servers, 1 shared · 3 skills · 4 agents". */
export function reachWords(d: DepartmentView): string {
  const servers = d.servers.length ? count(d.servers.length, "server") : d.uses.length ? "" : "no servers";
  const shared = d.uses.length ? `${d.uses.length} shared` : "";
  return [[servers, shared].filter(Boolean).join(", "), count(d.skills.length, "skill"), count(d.agents.length, "agent")].filter(Boolean).join(" · ");
}

/**
 * Reveal a folder in the operator's file manager. Inside Tauri the opener plugin does it;
 * in a browser (`make desktop-web`) there is no way, so the caller shows the path instead.
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
