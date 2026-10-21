/**
 * Owner: conversations. The editable copy of a line, its dirty fields, and how a draft
 * survives a reload: a field the operator hasn't touched follows the server, a field
 * they have touched keeps their text. Drafts live for the app's life per slug, so moving
 * between lines never throws away unsaved words.
 */
import type { ConversationDetail } from "@ghost/client-ts";

import { type Scope, type ThreadUpdate, sameScope, scopeOf } from "./threads";

export interface Draft {
  name: string;
  aliases: string;
  voice: string;
  speed: number;
  persona: string;
  memory: string;
  notes: string[];
  /** Per server, what the line may touch (ADR 0019). Empty: it sees everything. */
  scope: Scope;
}

export type DraftKey = keyof Draft;

/**
 * The line's own name and slug are left out of "Also answers to": Ghost already answers to
 * the name (ghost_core matches on name and aliases), so listing it there reads like a bug.
 * Base and draft are filtered alike, so nothing is dirty and Save sends no aliases unless
 * the operator edits the field.
 */
export function fromDetail(d: ConversationDetail): Draft {
  const own = new Set([d.name.trim().toLowerCase(), d.slug]);
  const aliases = d.aliases.filter((a) => !own.has(a.trim().toLowerCase()));
  return { name: d.name, aliases: aliases.join(", "), voice: d.voice, speed: d.speed, persona: d.persona, memory: d.memory, notes: [...d.notes], scope: scopeOf(d) };
}

export const splitAliases = (s: string) => s.split(",").map((a) => a.trim()).filter(Boolean);
const cleanNotes = (n: string[]) => n.map((x) => x.trim()).filter(Boolean);

function same(k: DraftKey, a: Draft, b: Draft): boolean {
  if (k === "aliases") return splitAliases(a.aliases).join("|") === splitAliases(b.aliases).join("|");
  if (k === "notes") return cleanNotes(a.notes).join("\n") === cleanNotes(b.notes).join("\n");
  if (k === "name") return a.name.trim() === b.name.trim();
  if (k === "speed") return Math.abs(a.speed - b.speed) < 0.001;
  if (k === "scope") return sameScope(a.scope, b.scope);
  return a[k] === b[k];
}

const KEYS: DraftKey[] = ["name", "aliases", "voice", "speed", "persona", "memory", "notes", "scope"];

/** Ghost keeps its name and is never scoped (ADR 0019): those keys can't be dirty on it. */
export function dirtyKeys(draft: Draft, base: Draft, isGhost: boolean): DraftKey[] {
  return KEYS.filter((k) => !(isGhost && (k === "name" || k === "aliases" || k === "scope")) && !same(k, draft, base));
}

/** The PATCH body for the dirty fields, shaped like the old screen's save. */
export function updateBody(draft: Draft, keys: DraftKey[]): ThreadUpdate {
  const body: ThreadUpdate = {};
  for (const k of keys) {
    if (k === "name") body.name = draft.name.trim();
    else if (k === "aliases") body.aliases = splitAliases(draft.aliases);
    else if (k === "notes") body.notes = cleanNotes(draft.notes);
    else if (k === "speed") body.speed = draft.speed;
    else if (k === "scope") body.scope = cleanScope(draft.scope);
    else body[k] = draft[k];
  }
  return body;
}

/** A scope with no empty lists: a server without items is simply absent (unscoped for it). */
export function cleanScope(scope: Scope): Scope {
  const out: Scope = {};
  for (const [k, items] of Object.entries(scope)) if (items.length) out[k] = [...items];
  return out;
}

/** Carry the operator's edits onto fresh server data. */
export function rebase(draft: Draft, oldBase: Draft, next: Draft): Draft {
  const out = { ...next };
  for (const k of KEYS) {
    if (same(k, draft, oldBase)) continue;
    if (k === "notes") {
      // Keep their edits, and still show notes pinned since (by voice or "Remember this").
      const added = next.notes.filter((n) => !oldBase.notes.includes(n) && !draft.notes.includes(n));
      out.notes = [...draft.notes, ...added];
    } else {
      (out as Record<DraftKey, unknown>)[k] = draft[k];
    }
  }
  return out;
}

export const FIELD_WORDS: Record<DraftKey, string> = {
  name: "name",
  aliases: "other names",
  voice: "voice",
  speed: "speed",
  persona: "persona",
  memory: "memory",
  notes: "pinned notes",
  scope: "scope",
};

/** "persona, voice and memory" */
export function listWords(keys: DraftKey[]): string {
  const w = [...new Set(keys.map((k) => FIELD_WORDS[k]))];
  if (w.length <= 1) return w.join("");
  return `${w.slice(0, -1).join(", ")} and ${w[w.length - 1]}`;
}

/** Unsaved drafts by slug, for the app's life. */
export const drafts = new Map<string, { draft: Draft; base: Draft }>();
