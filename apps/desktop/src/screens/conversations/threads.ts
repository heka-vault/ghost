/**
 * Owner: conversations. The ADR 0019 additions to a line (scope, settled, seed_from) as the
 * generated client carries them, plus the helpers every screen reads them through: a daemon
 * that predates the fields still renders (no scope reads as unscoped, no settled_at as
 * active), and GET /sources is narrowed to the sources that can be scoped (ADR 0022).
 */
import type { ConversationCreate, ConversationDetail, ConversationUpdate, ConversationView, GhostClient, SourceInfo, ToolkitView } from "@ghost/client-ts";

/** Per source (a server's name in the policy file), the items this line may touch: `{ github_heka: ["heka-vault/ghost"] }`. */
export type Scope = Record<string, string[]>;

/** The two fields a line record always carries in the mock (the API marks settled_at optional). */
export interface ThreadFields {
  scope: Scope;
  /** When the line was settled (left voice routing and the active list), or null. */
  settled_at: string | null;
}
export type ThreadDetail = ConversationDetail & ThreadFields;
export type ThreadCreate = ConversationCreate;
export type ThreadUpdate = ConversationUpdate;

/** A source that can be scoped: how ("repository"), and the whole allowlist a scope picks from. */
export interface ScopedSource {
  name: string;
  scope_kind: string;
  scope_items: string[];
}

// `object`, not an all-optional shape: a ConversationView typed without the fields must pass
// the weak-type check until the generated client carries them.
type MaybeThread = object;
const field = (d: MaybeThread | null | undefined, k: "scope" | "settled_at"): unknown => (d ? (d as Record<string, unknown>)[k] : undefined);

export function scopeOf(d: MaybeThread | null | undefined): Scope {
  const raw = field(d, "scope");
  if (!raw || typeof raw !== "object") return {};
  const out: Scope = {};
  for (const [source, items] of Object.entries(raw as Record<string, unknown>)) {
    if (Array.isArray(items)) out[source] = items.filter((i): i is string => typeof i === "string");
  }
  return out;
}

export function settledAt(d: MaybeThread | null | undefined): string | null {
  const v = field(d, "settled_at");
  return typeof v === "string" && v ? v : null;
}

export const isSettled = (d: MaybeThread | null | undefined) => settledAt(d) !== null;

/** Every item in a scope, across sources, in a stable order: what a row or a tooltip lists. */
export function scopeItems(scope: Scope): string[] {
  return Object.keys(scope)
    .sort()
    .flatMap((k) => [...scope[k]].sort());
}

/** Scopes with the same sources and items, in any order. */
export function sameScope(a: Scope, b: Scope): boolean {
  return scopeKey(a) === scopeKey(b);
}

/** A stable string for a scope, for comparisons and grouping. Empty lists count as no entry. */
export function scopeKey(scope: Scope): string {
  return Object.keys(scope)
    .filter((k) => scope[k].length)
    .sort()
    .map((k) => `${k}:${[...scope[k]].sort().join(",")}`)
    .join(";");
}

/** The sources a scope can name, from GET /sources: the servers that declare a scope, with the items they allow. */
export function scopedSources(sources: SourceInfo[]): ScopedSource[] {
  return sources.flatMap((a) => {
    if (typeof a.scope_kind !== "string" || !a.scope_kind) return [];
    const items = Array.isArray(a.scope_items) ? a.scope_items.filter((i): i is string => typeof i === "string") : [];
    return [{ name: a.name, scope_kind: a.scope_kind, scope_items: items }];
  });
}

/** What a scope kind's items are called: "repository" / "repositories". */
export function kindWords(kind: string): { one: string; many: string } {
  const KNOWN: Record<string, [string, string]> = {
    repository: ["repository", "repositories"],
    directory: ["folder", "folders"],
    workspace: ["workspace", "workspaces"],
  };
  const k = KNOWN[kind];
  return k ? { one: k[0], many: k[1] } : { one: kind, many: `${kind}s` };
}

/** "GitHub heka" for github_heka: the server's name in words, used as a card heading. */
export function sourceWords(name: string): string {
  if (name === "conversations") return "Lines";
  const s = name.replace(/[_-]+/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1).replace(/\bgithub\b/i, "GitHub");
}

/* ------------------------------------------------------------------ departments (ADR 0023) */

/** The fields a line carries about its department; a daemon that predates them reads as a department with no toolkits. */
export type DepartmentFields = Pick<ConversationView, "slug" | "is_ghost"> & Partial<Pick<ConversationView, "department" | "is_department" | "toolkits">>;

/** The department a topic line is in (its slug), or null for a department itself and for Ghost. */
export const departmentOf = (l: DepartmentFields | null | undefined): string | null => (l && typeof l.department === "string" && l.department ? l.department : null);

/** A department: its own primary line (ADR 0023). Ghost is line zero, not a department. */
export const isDepartment = (l: DepartmentFields | null | undefined): boolean => !!l && !l.is_ghost && departmentOf(l) === null;

/** A department's toolkits; empty means it reaches everything, like Ghost. */
export const toolkitsOf = (l: DepartmentFields | null | undefined): string[] => (l && Array.isArray(l.toolkits) ? l.toolkits.filter((t): t is string => typeof t === "string") : []);

/** A department's name as the operator says it: the line's name. */
export function departmentName(slug: string | null, lines: Pick<ConversationView, "slug" | "name">[]): string {
  if (!slug) return "Ghost";
  return lines.find((l) => l.slug === slug)?.name ?? slug.replace(/-/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

export interface DepartmentGroup<T extends DepartmentFields> {
  /** The department's primary line, or an orphan topic line whose department is gone. */
  head: T;
  /** Its topic lines, in the order given. */
  lines: T[];
}

/**
 * Lines grouped the way the sidebar and the phone show them: Ghost alone, then each
 * department with its topic lines under it. A topic line whose department isn't in the list
 * (settled, deleted, or filtered out) stands on its own so it is never lost. Order follows
 * the input: a group sits where its first member (head or line) would have.
 */
export function groupByDepartment<T extends DepartmentFields>(lines: T[]): DepartmentGroup<T>[] {
  const heads = new Map<string, DepartmentGroup<T>>();
  const order: DepartmentGroup<T>[] = [];
  const pending = new Map<string, T[]>();
  const slugs = new Set(lines.map((l) => l.slug));
  for (const l of lines) {
    const dept = departmentOf(l);
    if (dept && slugs.has(dept) && dept !== l.slug) {
      const g = heads.get(dept);
      if (g) g.lines.push(l);
      else pending.set(dept, [...(pending.get(dept) ?? []), l]);
      continue;
    }
    const g: DepartmentGroup<T> = { head: l, lines: pending.get(l.slug) ?? [] };
    pending.delete(l.slug);
    heads.set(l.slug, g);
    order.push(g);
  }
  // A group whose topic line came first in the input moves up to where that line was.
  return order.sort((a, b) => firstIndex(a, lines) - firstIndex(b, lines));
}
const firstIndex = <T extends DepartmentFields>(g: DepartmentGroup<T>, lines: T[]) => Math.min(lines.indexOf(g.head), ...g.lines.map((l) => lines.indexOf(l)));

/**
 * The scope choices a topic line gets (ADR 0023): only what its department's toolkits grant,
 * where that can be read off the toolkits. A department with no toolkits reaches everything,
 * so the sources stay as they are; a toolkit that grants a server with `items: null` grants
 * every item; a server no toolkit grants is out of reach and not offered. Without the toolkit
 * list (not loaded, or failed) the sources are left as they are.
 */
export function narrowSources(sources: ScopedSource[], toolkitNames: string[], toolkits: ToolkitView[] | null): ScopedSource[] {
  if (!toolkitNames.length || !toolkits) return sources;
  const granted = toolkits.filter((t) => toolkitNames.includes(t.name));
  if (!granted.length) return sources;
  return sources.flatMap((src) => {
    const grants = granted.flatMap((t) => t.grants.filter((g) => g.server === src.name));
    if (!grants.length) return [];
    if (grants.some((g) => g.items == null)) return [src];
    const items = new Set(grants.flatMap((g) => g.items ?? []));
    return [{ ...src, scope_items: src.scope_items.filter((i) => items.has(i)) }];
  });
}

/** Settle a line: it leaves voice routing and the active list; everything stays readable. */
export const settleLine = (client: GhostClient, slug: string) => client.settleConversation(slug);
/** Bring a settled line back. */
export const unsettleLine = (client: GhostClient, slug: string) => client.unsettleConversation(slug);
