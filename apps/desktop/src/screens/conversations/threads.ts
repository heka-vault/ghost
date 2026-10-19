/**
 * Owner: conversations. The ADR 0019 additions to a line (scope, settled, seed_from) as the
 * generated client carries them, plus the helpers every screen reads them through: a daemon
 * that predates the fields still renders (no scope reads as unscoped, no settled_at as
 * active), and GET /abilities is narrowed to the abilities that can be scoped.
 */
import type { AbilityInfo, ConversationCreate, ConversationDetail, ConversationUpdate, GhostClient } from "@ghost/client-ts";

/** Per ability, the items this line may touch: `{ github_issues: ["heka-vault/ghost"] }`. */
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

/** An ability that can be scoped: how ("repository"), and the whole allowlist a scope picks from. */
export interface ScopedAbility {
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
  for (const [ability, items] of Object.entries(raw as Record<string, unknown>)) {
    if (Array.isArray(items)) out[ability] = items.filter((i): i is string => typeof i === "string");
  }
  return out;
}

export function settledAt(d: MaybeThread | null | undefined): string | null {
  const v = field(d, "settled_at");
  return typeof v === "string" && v ? v : null;
}

export const isSettled = (d: MaybeThread | null | undefined) => settledAt(d) !== null;

/** Every item in a scope, across abilities, in a stable order: what a row or a tooltip lists. */
export function scopeItems(scope: Scope): string[] {
  return Object.keys(scope)
    .sort()
    .flatMap((k) => [...scope[k]].sort());
}

/** Scopes with the same abilities and items, in any order. */
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

/** The abilities a scope can name, from GET /abilities, with items this daemon reports. */
export function scopedAbilities(abilities: AbilityInfo[]): ScopedAbility[] {
  return abilities.flatMap((a) => {
    const x = a as AbilityInfo & { scope_kind?: unknown; scope_items?: unknown };
    if (typeof x.scope_kind !== "string" || !x.scope_kind) return [];
    const items = Array.isArray(x.scope_items) ? x.scope_items.filter((i): i is string => typeof i === "string") : [];
    return [{ name: a.name, scope_kind: x.scope_kind, scope_items: items }];
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

/** "GitHub issues" for github_issues: the ability's name in words, used as a card heading. */
export function abilityWords(name: string): string {
  const s = name.replace(/[_-]+/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1).replace(/\bgithub\b/i, "GitHub");
}

/** Settle a line: it leaves voice routing and the active list; everything stays readable. */
export const settleLine = (client: GhostClient, slug: string) => client.settleConversation(slug);
/** Bring a settled line back. */
export const unsettleLine = (client: GhostClient, slug: string) => client.unsettleConversation(slug);
