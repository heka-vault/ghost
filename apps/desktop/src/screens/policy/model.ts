/** Owner: policy. Pure helpers: how a level resolves, what changed, plain names. */
import type { AbilityInfo, Autonomy, OperationInfo } from "@ghost/client-ts";
import { Hand, Megaphone, ShieldOff, VolumeX } from "lucide-react";

export type Skills = Record<string, Record<string, string>>;
export type Targets = Record<string, Record<string, Record<string, string>>>;

/** Least to most autonomous, left to right. The icons are the design system's. */
export const LEVELS: { value: Autonomy; icon: typeof Hand }[] = [
  { value: "deny", icon: ShieldOff },
  { value: "confirm", icon: Hand },
  { value: "announce", icon: Megaphone },
  { value: "quiet", icon: VolumeX },
];
const LEVEL_SET = new Set<string>(LEVELS.map((l) => l.value));

export const LEVEL_ICON: Record<string, typeof Hand> = Object.fromEntries(LEVELS.map((l) => [l.value, l.icon]));

/** ghost_core.policy.RISK_DEFAULTS: what an operation does when nothing in the file names it. */
export const RISK_DEFAULTS: Record<string, Autonomy> = { low: "announce", medium: "confirm", high: "confirm" };

export const RISK_STEPS: Record<string, number> = { low: 1, medium: 2, high: 3 };

const ANY = "*";

export type InheritedFrom = "ability" | "default" | "risk";

/**
 * What an operation does when its own line is absent, following AutonomyPolicy.resolve
 * without a target: the ability's `*` rule, then [defaults], then the risk class.
 */
export function inherited(skills: Skills, defaultLevel: string, ability: string, op: OperationInfo): { level: string; from: InheritedFrom } {
  const any = skills[ability]?.[ANY];
  if (any) return { level: any, from: "ability" };
  if (defaultLevel) return { level: defaultLevel, from: "default" };
  return { level: RISK_DEFAULTS[op.risk] ?? op.autonomy, from: "risk" };
}

/** Per-target rules that touch this operation: [target, level]. */
export function targetRules(targets: Targets, ability: string, op: string): [string, string][] {
  return Object.entries(targets[ability] ?? {}).flatMap(([t, ops]) => {
    const lv = ops[op] ?? ops[ANY];
    return lv ? [[t, lv] as [string, string]] : [];
  });
}

/** Keys ("ability.op", or "default") whose value differs between two edits. */
export function changedKeys(a: Skills, b: Skills, defA: string, defB: string): Set<string> {
  const out = new Set<string>();
  if ((defA || "") !== (defB || "")) out.add("default");
  for (const ability of new Set([...Object.keys(a), ...Object.keys(b)])) {
    const x = a[ability] ?? {}, y = b[ability] ?? {};
    for (const op of new Set([...Object.keys(x), ...Object.keys(y)])) if (x[op] !== y[op]) out.add(`${ability}.${op}`);
  }
  return out;
}

const WORDS: Record<string, string> = { github: "GitHub", gitlab: "GitLab", dm: "DM", onnx: "ONNX", pr: "PR", prs: "PRs", url: "URL", ai: "AI", api: "API", tts: "TTS", stt: "STT", id: "ID" };

/** "github_issues" → "GitHub issues", "set_labels" → "Set labels". */
export function humanize(name: string): string {
  const words = name.split(/[_\-.]+/).filter(Boolean).map((w) => WORDS[w.toLowerCase()] ?? w.toLowerCase());
  if (!words.length) return name;
  const [first, ...rest] = words;
  return [WORDS[first.toLowerCase()] ? first : first[0].toUpperCase() + first.slice(1), ...rest].join(" ");
}

/** Straight quotes to typographer’s quotes, for text written by someone else. */
export function curly(text: string): string {
  return text
    .replace(/(^|[\s(\[{])"/g, "$1“")
    .replace(/"/g, "”")
    .replace(/(^|[\s(\[{])'/g, "$1‘")
    .replace(/'/g, "’");
}

/** A sentence ends with a period. */
function period(s: string): string {
  const t = s.trim();
  return !t || /[.!?…]$/.test(t) ? t : `${t}.`;
}

/** The first sentence of a description written for the model, without ADR references. */
export function firstSentence(text: string): string {
  const clean = text.replace(/\s*\(ADR \d+\)/g, "").trim();
  const m = clean.match(/^(.+?[.!?])(\s|$)/);
  return (m ? m[1] : clean).trim();
}

/**
 * Model-facing description → a line the operator reads: the first sentence, without a
 * leading “Title:” that repeats the row’s name, with typographer’s quotes and a period.
 */
export function plainDescription(text: string, title: string): string {
  let s = firstSentence(text);
  const prefix = new RegExp(`^${title.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*:\\s*`, "i");
  if (prefix.test(s)) {
    s = s.replace(prefix, "");
    s = s ? s[0].toUpperCase() + s.slice(1) : s;
  }
  s = s.replace(/\bthe operator's\b/gi, "your");
  return period(curly(s));
}

/* --------------------------------------------------------------- operator copy */

/** U+2011, so a label like needs‑signoff never breaks at its hyphen. */
const nb = (label: string) => label.replace(/-/g, "‑");

/** Label names are configurable; read them out of the model-facing description. */
function labelsIn(description: string): string[] {
  return [...description.matchAll(/'([^']+)'/g)].map((m) => nb(m[1]));
}

export interface OperationCopy {
  title?: string;
  text: string | ((description: string) => string);
  /** “Ghost won’t ask before …”: the act in a few words, for a high-risk operation set to act alone. */
  act?: string;
}

/**
 * The operations this repo ships, in the operator’s voice. Anything else falls back to
 * plainDescription() of what the ability wrote for the model.
 */
export const OPERATION_COPY: Record<string, Record<string, OperationCopy>> = {
  github_issues: {
    approve: {
      text: (d) => {
        const [off, on] = labelsIn(d);
        return off && on ? `Sign off on an issue: ${off} comes off, ${on} goes on.` : "Sign off on an issue.";
      },
      act: "approving issues",
    },
    request_changes: {
      text: (d) => {
        const [, on] = labelsIn(d);
        return on ? `Send an issue back with what must change, labeled ${on}.` : "Send an issue back with what must change.";
      },
      act: "sending issues back",
    },
    comment: { text: "Comment on an issue.", act: "commenting" },
    set_labels: { text: "Add or remove labels on an issue.", act: "changing labels" },
    close: { text: "Close an issue, with a comment if there’s one to leave.", act: "closing issues" },
    reopen: { text: "Reopen a closed issue.", act: "reopening issues" },
  },
  conversations: {
    create: { title: "New line", text: "Start a new line and switch you to it.", act: "starting lines" },
    rename: { text: "Rename a line or change what it answers to.", act: "renaming lines" },
    set_voice: { title: "Voice", text: "Change a line’s voice or how fast it speaks.", act: "changing voices" },
    set_persona: { title: "Persona", text: "Set who a line is, in your own words, kept word for word.", act: "changing who a line is" },
    remember: { text: "Pin something you asked Ghost to remember.", act: "pinning notes" },
    forget: { title: "Forget a note", text: "Drop a pinned note.", act: "dropping notes" },
    compact: { title: "Compact memory", text: "Fold a line’s memory into a fresh summary, when you ask.", act: "rewriting memory" },
    reset: { text: "Clear a line’s memory, notes and transcript. Its name and voice stay.", act: "clearing a line" },
    delete: { text: "Delete a line for good. Ghost itself can’t be deleted.", act: "deleting lines" },
    switch: { title: "Switch lines", text: "Switch you to another line.", act: "switching lines" },
  },
};

export const ABILITY_COPY: Record<string, string> = {
  github_issues: "Read what needs your sign-off, then label, comment on and close issues.",
  conversations: "Your saved lines: who each one is, how it sounds and what it remembers.",
};

export function operationTitle(ability: string, op: string): string {
  return OPERATION_COPY[ability]?.[op]?.title ?? humanize(op);
}

export function operationText(ability: string, op: OperationInfo): string {
  const c = OPERATION_COPY[ability]?.[op.name];
  if (c) return typeof c.text === "function" ? c.text(op.description) : c.text;
  return plainDescription(op.description, humanize(op.name));
}

/** “Ghost won’t ask before closing issues.” */
export function highRiskNote(ability: string, op: string): string {
  const act = OPERATION_COPY[ability]?.[op]?.act;
  return act ? `Ghost won’t ask before ${act}.` : "Ghost won’t ask first.";
}

export function abilityText(a: AbilityInfo): string {
  return ABILITY_COPY[a.name] ?? plainDescription(a.description, humanize(a.name));
}

/** "(at line 4, column 7)" in a tomllib error → 4. */
export function errorLine(message: string): number | null {
  const m = message.match(/line (\d+)/i);
  return m ? Number(m[1]) : null;
}

/** Abilities with write operations, and the ones that only read. */
export function splitAbilities(abilities: AbilityInfo[]): { writers: AbilityInfo[]; readers: AbilityInfo[] } {
  const on = abilities.filter((a) => a.enabled);
  return { writers: on.filter((a) => (a.operations ?? []).length > 0), readers: on.filter((a) => !(a.operations ?? []).length) };
}

/** "A", "A and B", "A, B and C". */
export function listWords(xs: string[]): string {
  if (xs.length <= 1) return xs.join("");
  return `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`;
}

/* ------------------------------------------------------------------- the file */

/**
 * The policy file, read the way ghost_core.policy.AutonomyPolicy.parse reads it, for the
 * subset of TOML the file uses: tables, quoted or bare keys, strings, arrays of strings,
 * inline tables, comments. Levels and TOML are two views of one text, so the page reads
 * levels out of the text it is editing and writes them back with setKey(), which touches
 * only the one line it changes: the operator’s comments and layout survive every save.
 */
export interface PolicyData {
  default: string | null;
  skills: Skills;
  targets: Targets;
  enabled: string[] | null;
  passive: string[];
}

export type ParseResult =
  | { ok: true; data: PolicyData; locs: Map<string, Loc>; headers: Map<string, Header>; implicit: Set<string> }
  /** `certain`: Ghost would refuse it too. Otherwise it is TOML this reader doesn’t follow; let Ghost decide. */
  | { ok: false; line: number; message: string; certain: boolean };

interface Loc {
  line: number;
  /** One `key = "value"` on its own line, directly under its table’s header. */
  plain: boolean;
}
interface Header {
  line: number;
  /** The last line holding one of this table’s keys (the header line when it has none). */
  last: number;
}

class Fail extends Error {
  line: number;
  certain: boolean;
  constructor(line: number, message: string, certain = true) {
    super(message);
    this.line = line;
    this.certain = certain;
  }
}

type Value = string | Value[] | Table;
interface Table {
  [k: string]: Value;
}

const isTable = (v: unknown): v is Table => typeof v === "object" && v !== null && !Array.isArray(v);
const pathKey = (path: string[]) => JSON.stringify(path);

/** A TOML key: bare when it can be, quoted otherwise (`"*"`, `"ghost-mock/app"`). */
export const tomlKey = (k: string) => (/^[A-Za-z0-9_-]+$/.test(k) ? k : JSON.stringify(k));

export function parsePolicy(text: string): ParseResult {
  const src = text.replace(/\r\n?/g, "\n");
  let i = 0;
  let line = 1;
  const root: Table = {};
  const locs = new Map<string, Loc>();
  const headers = new Map<string, Header>();
  const implicit = new Set<string>();
  const defined = new Set<string>();
  let current: string[] = [];
  let currentTable = root;

  const peek = (n = 0) => src[i + n] ?? "";
  const eof = () => i >= src.length;
  const ws = () => {
    while (peek() === " " || peek() === "\t") i++;
  };
  const comment = () => {
    if (peek() === "#") while (!eof() && peek() !== "\n") i++;
  };
  /** Whitespace, newlines and comments, inside arrays. */
  const gap = () => {
    for (;;) {
      ws();
      comment();
      if (peek() === "\n") {
        i++;
        line++;
      } else return;
    }
  };
  const endOfLine = () => {
    ws();
    comment();
    if (!eof() && peek() !== "\n") throw new Fail(line, `Line ${line}: there’s something extra after the value. Put each setting on its own line.`);
  };

  const basic = (): string => {
    if (src.startsWith('"""', i)) throw new Fail(line, `Line ${line}: multi-line strings aren’t something Levels can read.`, false);
    i++;
    let out = "";
    for (;;) {
      const c = peek();
      if (eof() || c === "\n") throw new Fail(line, `Line ${line}: a quote is missing at the end of the value.`);
      i++;
      if (c === '"') return out;
      if (c === "\\") {
        const e = peek();
        i++;
        const map: Record<string, string> = { n: "\n", t: "\t", '"': '"', "\\": "\\", r: "\r", b: "\b", f: "\f" };
        if (e in map) out += map[e];
        else if (e === "u" || e === "U") {
          const n = e === "u" ? 4 : 8;
          const hex = src.slice(i, i + n);
          if (!/^[0-9a-fA-F]+$/.test(hex) || hex.length !== n) throw new Fail(line, `Line ${line}: a \\${e} escape needs ${n} hex digits.`);
          out += String.fromCodePoint(parseInt(hex, 16));
          i += n;
        } else throw new Fail(line, `Line ${line}: \\${e} isn’t an escape TOML knows.`);
      } else out += c;
    }
  };
  const literal = (): string => {
    if (src.startsWith("'''", i)) throw new Fail(line, `Line ${line}: multi-line strings aren’t something Levels can read.`, false);
    i++;
    const end = src.indexOf("'", i);
    const nl = src.indexOf("\n", i);
    if (end < 0 || (nl >= 0 && nl < end)) throw new Fail(line, `Line ${line}: a quote is missing at the end of the value.`);
    const s = src.slice(i, end);
    i = end + 1;
    return s;
  };
  const keyPart = (): string => {
    ws();
    const c = peek();
    if (c === '"') return basic();
    if (c === "'") return literal();
    const m = /^[A-Za-z0-9_-]+/.exec(src.slice(i));
    if (!m) throw new Fail(line, `Line ${line}: expected a name here.`);
    i += m[0].length;
    return m[0];
  };
  const keyPath = (): string[] => {
    const parts = [keyPart()];
    for (;;) {
      ws();
      if (peek() !== ".") return parts;
      i++;
      parts.push(keyPart());
    }
  };

  const value = (): Value => {
    ws();
    const c = peek();
    if (c === '"') return basic();
    if (c === "'") return literal();
    if (c === "[") {
      i++;
      const out: Value[] = [];
      for (;;) {
        gap();
        if (peek() === "]") {
          i++;
          return out;
        }
        out.push(value());
        gap();
        if (peek() === ",") {
          i++;
          continue;
        }
        if (peek() === "]") {
          i++;
          return out;
        }
        throw new Fail(line, `Line ${line}: a list needs commas between its items and a ] at the end.`);
      }
    }
    if (c === "{") {
      i++;
      const t: Table = {};
      ws();
      if (peek() === "}") {
        i++;
        return t;
      }
      for (;;) {
        const k = keyPath();
        ws();
        if (peek() !== "=") throw new Fail(line, `Line ${line}: expected = after ${k.join(".")}.`);
        i++;
        assign(t, k, value(), null);
        ws();
        if (peek() === ",") {
          i++;
          continue;
        }
        if (peek() === "}") {
          i++;
          return t;
        }
        throw new Fail(line, `Line ${line}: an inline table needs commas between its items and a } at the end.`);
      }
    }
    const m = /^[^\s,\]}#]+/.exec(src.slice(i));
    const word = m?.[0] ?? "";
    if (LEVEL_SET.has(word)) throw new Fail(line, `Line ${line}: the level \`${word}\` needs quotes: \`"${word}"\`.`);
    if (!word) throw new Fail(line, `Line ${line}: there’s no value after the =.`);
    throw new Fail(line, `Line ${line}: \`${word}\` isn’t a level. Use \`"deny"\`, \`"confirm"\`, \`"announce"\` or \`"quiet"\`.`);
  };

  /** Set `path` under `t`, creating dotted tables; `full` is the absolute path for locations. */
  const assign = (t: Table, path: string[], v: Value, full: string[] | null) => {
    let at = t;
    for (const p of path.slice(0, -1)) {
      if (!(p in at)) at[p] = {};
      const next = at[p];
      if (!isTable(next)) throw new Fail(line, `Line ${line}: ${p} is already set to a value, so it can’t hold ${path.join(".")}.`);
      at = next;
    }
    const last = path[path.length - 1];
    if (last in at) throw new Fail(line, `Line ${line}: ${path.join(".")} is set twice${current.length ? ` in [${current.join(".")}]` : ""}.`);
    at[last] = v;
    if (full) locs.set(pathKey(full), { line, plain: path.length === 1 && typeof v === "string" });
  };

  try {
    while (!eof()) {
      ws();
      comment();
      if (eof()) break;
      if (peek() === "\n") {
        i++;
        line++;
        continue;
      }
      if (peek() === "[") {
        if (peek(1) === "[") throw new Fail(line, `Line ${line}: [[…]] makes a list of tables, and this file uses only [tables].`);
        const at = line;
        i++;
        const path = keyPath();
        ws();
        if (peek() !== "]") throw new Fail(line, `Line ${line}: a ] is missing at the end of the [${path.join(".")}] header.`);
        i++;
        endOfLine();
        const k = pathKey(path);
        if (defined.has(k)) throw new Fail(at, `Line ${at}: \`[${path.join(".")}]\` appears twice. Merge them into one.`);
        defined.add(k);
        let t = root;
        for (let n = 0; n < path.length; n++) {
          const p = path[n];
          if (!(p in t)) {
            t[p] = {};
            if (n < path.length - 1) implicit.add(pathKey(path.slice(0, n + 1)));
          }
          const next = t[p];
          if (!isTable(next)) throw new Fail(at, `Line ${at}: ${path.slice(0, n + 1).join(".")} is already a value, so it can’t be a table.`);
          t = next;
        }
        implicit.delete(k);
        headers.set(k, { line: at, last: at });
        current = path;
        currentTable = t;
        continue;
      }
      const at = line;
      const path = keyPath();
      ws();
      if (peek() !== "=") throw new Fail(at, `Line ${at}: expected = after ${path.join(".")}.`);
      i++;
      const v = value();
      endOfLine();
      assign(currentTable, path, v, [...current, ...path]);
      const h = headers.get(pathKey(current));
      if (h) h.last = line;
    }
  } catch (e) {
    if (e instanceof Fail) return { ok: false, line: e.line, message: e.message, certain: e.certain };
    throw e;
  }

  // The same rules as AutonomyPolicy.parse.
  const lineOf = (path: string[]) => locs.get(pathKey(path))?.line ?? headers.get(pathKey(path.slice(0, 1)))?.line ?? 1;
  const level = (v: Value, path: string[]): string => {
    if (typeof v === "string" && LEVEL_SET.has(v)) return v;
    const ln = lineOf(path);
    const shown = typeof v === "string" ? `\`"${v}"\`` : "That value";
    throw new Fail(ln, `Line ${ln}: ${shown} isn’t a level. Use \`"deny"\`, \`"confirm"\`, \`"announce"\` or \`"quiet"\`.`);
  };
  const names = (v: Value, path: string[]): string[] => {
    if (Array.isArray(v) && v.every((x) => typeof x === "string")) return v as string[];
    const ln = lineOf(path);
    throw new Fail(ln, `Line ${ln}: \`${path.join(".")}\` needs to be a list of ability names, each in quotes.`);
  };
  try {
    const data: PolicyData = { default: null, skills: {}, targets: {}, enabled: null, passive: [] };
    for (const [section, body] of Object.entries(root)) {
      if (!isTable(body)) {
        const ln = lineOf([section]);
        throw new Fail(ln, `Line ${ln}: \`${section}\` needs to sit under a table, like \`[github_issues]\`.`);
      }
      if (section === "defaults") {
        if ("autonomy" in body) data.default = level(body.autonomy, ["defaults", "autonomy"]);
        continue;
      }
      if (section === "abilities") {
        if ("enabled" in body) data.enabled = names(body.enabled, ["abilities", "enabled"]);
        if ("passive" in body) data.passive = names(body.passive, ["abilities", "passive"]);
        continue;
      }
      const ops: Record<string, string> = {};
      for (const [key, v] of Object.entries(body)) {
        if (key === "targets") {
          if (!isTable(v)) throw new Fail(lineOf([section, key]), `Line ${lineOf([section, key])}: [${section}.targets] needs to be a table.`);
          for (const [tname, tops] of Object.entries(v)) {
            if (!isTable(tops)) throw new Fail(lineOf([section, key, tname]), `Line ${lineOf([section, key, tname])}: [${section}.targets."${tname}"] needs to be a table.`);
            const out: Record<string, string> = {};
            for (const [op, lv] of Object.entries(tops)) out[op] = level(lv, [section, key, tname, op]);
            (data.targets[section] ??= {})[tname.toLowerCase()] = out;
          }
          continue;
        }
        ops[key] = level(v, [section, key]);
      }
      data.skills[section] = ops;
    }
    return { ok: true, data, locs, headers, implicit };
  } catch (e) {
    if (e instanceof Fail) return { ok: false, line: e.line, message: e.message, certain: e.certain };
    throw e;
  }
}

/** The first line of a file Ghost writes (ghost_core.policy.to_toml). */
export const FILE_HEADER = "# Ghost autonomy policy (ADR 0011). Edited through the desktop app or by hand.";

/**
 * Set or remove one `key = "level"` in `[table]`, changing only that line. A new key goes
 * after the table’s last key; a new table goes before its own sub-tables, or at the end.
 * Returns null when the file sets the key some other way (an inline table, a dotted key),
 * or doesn’t parse: then the edit belongs in TOML.
 */
export function setKey(text: string, table: string[], key: string, value: string | null): string | null {
  const r = parsePolicy(text);
  if (!r.ok) return null;
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const tk = pathKey(table);
  const loc = r.locs.get(pathKey([...table, key]));
  const header = r.headers.get(tk);

  if (loc) {
    if (!loc.plain || !header) return null;
    const idx = loc.line - 1;
    if (value == null) {
      lines.splice(idx, 1);
      // A table left with nothing but its header (no comments either) goes too.
      const h = header.line - 1;
      let end = h + 1;
      while (end < lines.length && !/^\s*\[/.test(lines[end])) end++;
      // Comments right above the next header belong to that header.
      let own = end;
      if (end < lines.length) while (own > h + 1 && /^\s*#/.test(lines[own - 1])) own--;
      const body = lines.slice(h + 1, own);
      const subTables = [...r.headers.keys()].some((k) => k !== tk && k.startsWith(tk.slice(0, -1) + ","));
      if (body.every((l) => !l.trim()) && !subTables) lines.splice(h, own - h);
      return tidy(lines);
    }
    const m = /^(\s*(?:"(?:[^"\\]|\\.)*"|'[^']*'|[A-Za-z0-9_-]+)\s*=\s*)("(?:[^"\\]|\\.)*"|'[^']*')(.*)$/.exec(lines[idx]);
    if (!m) return null;
    lines[idx] = `${m[1]}"${value}"${m[3]}`;
    return tidy(lines);
  }

  if (value == null) return text;
  const entry = `${tomlKey(key)} = "${value}"`;
  if (header) {
    lines.splice(header.last, 0, entry);
    return tidy(lines);
  }
  // The table doesn’t exist, or exists only through a dotted key or an inline table.
  if (hasTable(r, table) && !r.implicit.has(tk)) return null;
  const block = [`[${table.map(tomlKey).join(".")}]`, entry];
  if (lines.every((l) => !l.trim())) return [FILE_HEADER, "", ...block, ""].join("\n");

  let at: number | null = null;
  if (table[0] === "defaults") {
    // After [abilities], before the first ability table: where Ghost itself puts it.
    const ab = r.headers.get(pathKey(["abilities"]));
    const first = [...r.headers.entries()].filter(([k]) => k !== pathKey(["abilities"])).map(([, h]) => h.line).sort((a, b) => a - b)[0];
    if (ab) at = ab.last;
    else if (first) at = first - 1;
  } else {
    const prefix = tk.slice(0, -1) + ",";
    const subs = [...r.headers.entries()].filter(([k]) => k.startsWith(prefix)).map(([, h]) => h.line);
    if (subs.length) at = Math.min(...subs) - 1;
  }
  if (at == null) {
    while (lines.length && !lines[lines.length - 1].trim()) lines.pop();
    lines.push("", ...block, "");
    return tidy(lines);
  }
  // Skip back over comments that belong to the next header.
  while (at > 0 && /^\s*#/.test(lines[at - 1]) && at - 1 >= (r.headers.get(pathKey(["abilities"]))?.last ?? 0)) at--;
  lines.splice(at, 0, ...block, "");
  if (at > 0 && lines[at - 1].trim()) lines.splice(at, 0, "");
  return tidy(lines);
}

function hasTable(r: Extract<ParseResult, { ok: true }>, path: string[]): boolean {
  const k = pathKey(path);
  if (r.headers.has(k) || r.implicit.has(k)) return true;
  return [...r.locs.keys()].some((l) => l.startsWith(k.slice(0, -1) + ","));
}

/** No runs of blank lines, one newline at the end. */
function tidy(lines: string[]): string {
  const out: string[] = [];
  for (const l of lines) if (l.trim() || (out.length && out[out.length - 1].trim())) out.push(l);
  while (out.length && !out[out.length - 1].trim()) out.pop();
  return out.join("\n") + "\n";
}

/** The levels a text sets, or the saved view’s when the text can’t be read here. */
export function levelsOf(r: ParseResult, fallback: PolicyData): PolicyData {
  return r.ok ? r.data : fallback;
}

/** "a,b" equality for the enabled list, which is what decides a restart. */
export function sameList(a: string[] | null, b: string[] | null): boolean {
  return (a ?? []).join("\n") === (b ?? []).join("\n");
}
