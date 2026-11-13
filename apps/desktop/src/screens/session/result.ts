/**
 * What an opened call returned, in plain words where its shape is recognisable (ADR 0058's
 * preview, unchanged: the same 2,000 characters, redacted by the daemon). A list of items
 * becomes the items by their titles; one thing becomes its fields; a message becomes its
 * words. Anything else, or a preview cut mid-JSON, stays the raw view. Every word here is the
 * app's: shown as text only, never markup.
 */

export interface ListItem {
  title: string;
  meta: string[];
}

export type Shape =
  | { kind: "list"; count: number; total: number | null; items: ListItem[] }
  | { kind: "fields"; title: string | null; fields: [string, string][] }
  | { kind: "text"; text: string }
  | { kind: "raw" };

const TITLE_KEYS = ["title", "name", "subject", "summary", "full_name", "display_name", "login", "label", "text", "message", "description"];
const META_KEYS = ["number", "id", "state", "status", "repository", "repo", "author", "user", "from", "assignee", "updated_at", "created_at", "start", "date", "due"];
/** Keys a wrapper carries beside its list. */
const WRAPPER_KEYS = 4;
/** Items shown before “and N more”. */
export const LIST_SHOWN = 8;
const FIELDS_SHOWN = 10;

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function scalar(v: unknown): string | null {
  if (v == null) return null;
  if (typeof v === "string") return v.trim() ? v.trim() : null;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  if (isObj(v)) {
    for (const k of ["login", "name", "title", "full_name"]) if (typeof v[k] === "string") return v[k] as string;
  }
  if (Array.isArray(v) && v.every((x) => typeof x === "string" || isObj(x))) {
    const parts = v.map((x) => scalar(x)).filter((x): x is string => !!x);
    return parts.length ? parts.slice(0, 6).join(", ") : null;
  }
  return null;
}

/** “updated_at” → “Updated at”. */
export function humanKey(k: string): string {
  const s = k.replace(/[_-]+/g, " ").replace(/([a-z])([A-Z])/g, "$1 $2").trim().toLowerCase();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function short(s: string, n = 160): string {
  const one = s.replace(/\s+/g, " ").trim();
  return one.length > n ? `${one.slice(0, n - 1)}…` : one;
}

function when(s: string): string {
  const t = Date.parse(s);
  if (!/^\d{4}-\d{2}-\d{2}/.test(s) || Number.isNaN(t)) return s;
  return new Date(t).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

function itemOf(v: unknown): ListItem {
  if (!isObj(v)) return { title: short(scalar(v) ?? JSON.stringify(v)), meta: [] };
  const titleKey = TITLE_KEYS.find((k) => typeof v[k] === "string" && (v[k] as string).trim());
  const title = titleKey ? short(v[titleKey] as string) : short(Object.values(v).map(scalar).find((x) => !!x) ?? "(an item)");
  const meta: string[] = [];
  for (const k of META_KEYS) {
    if (k === titleKey || meta.length >= 3) continue;
    const s = scalar(v[k]);
    if (!s) continue;
    meta.push(k === "number" ? `#${s}` : k.endsWith("_at") || k === "date" || k === "start" || k === "due" ? when(s) : short(s, 40));
  }
  return { title, meta };
}

/** The one list a result is, or a small wrapper carries: `{"items": [...], "total_count": 3}`. */
function listOf(v: unknown): { list: unknown[]; total: number | null } | null {
  if (Array.isArray(v)) return { list: v, total: null };
  if (!isObj(v)) return null;
  const keys = Object.keys(v).filter((k) => !k.startsWith("_"));
  if (keys.length > WRAPPER_KEYS) return null;
  const lists = keys.filter((k) => Array.isArray(v[k]));
  if (lists.length !== 1) return null;
  const total = ["total_count", "total", "count"].map((k) => v[k]).find((x) => typeof x === "number") as number | undefined;
  return { list: v[lists[0]] as unknown[], total: total ?? null };
}

/** The shape of a preview's text, or `raw` when it isn't one this can say in words. */
export function shapeOf(preview: string): Shape {
  let v: unknown;
  try {
    v = JSON.parse(preview);
  } catch {
    return { kind: "raw" };
  }
  if (isObj(v) && Object.keys(v).length === 1 && "result" in v) v = v.result;
  const l = listOf(v);
  if (l && l.list.length > 0 && l.list.every((x) => isObj(x) || typeof x === "string")) {
    return { kind: "list", count: l.list.length, total: l.total != null && l.total !== l.list.length ? l.total : null, items: l.list.slice(0, LIST_SHOWN).map(itemOf) };
  }
  if (l && l.list.length === 0) return { kind: "list", count: 0, total: l.total, items: [] };
  if (isObj(v)) {
    const text = ["text", "message"].find((k) => typeof v[k] === "string");
    if (text && Object.keys(v).length <= 2) return { kind: "text", text: (v[text] as string).trim() };
    const titleKey = TITLE_KEYS.find((k) => typeof v[k] === "string" && (v[k] as string).trim() && k !== "description");
    const fields: [string, string][] = [];
    for (const [k, val] of Object.entries(v)) {
      if (k === titleKey || k.startsWith("_") || fields.length >= FIELDS_SHOWN) continue;
      const s = scalar(val);
      if (s) fields.push([humanKey(k), k.endsWith("_at") ? when(s) : short(s, 240)]);
    }
    if (fields.length || titleKey) return { kind: "fields", title: titleKey ? short(v[titleKey] as string) : null, fields };
  }
  if (typeof v === "string" && v.trim()) return { kind: "text", text: v.trim() };
  return { kind: "raw" };
}
