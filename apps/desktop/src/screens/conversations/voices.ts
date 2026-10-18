/**
 * Owner: conversations. Kokoro voices by accent and sex. ghostd labels each voice
 * "American female: Heart" (ghost_api.conversations.voice_label); the id carries the same
 * facts ("af_heart": a = American, f = female), so either is enough.
 */
import type { VoiceInfo } from "@ghost/client-ts";

export type Sex = "female" | "male" | "other";

export interface Voice {
  id: string;
  name: string;
  accent: string;
  sex: Sex;
}

const LANG: Record<string, string> = {
  a: "American",
  b: "British",
  e: "Spanish",
  f: "French",
  h: "Hindi",
  i: "Italian",
  j: "Japanese",
  p: "Portuguese",
  z: "Chinese",
};

/** English first, then the rest in a stable, readable order. */
const ACCENT_ORDER = ["American", "British", "Spanish", "French", "Italian", "Portuguese", "Hindi", "Japanese", "Chinese"];
export const ENGLISH = new Set(["American", "British"]);

const cap = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s);

export function parseVoice(v: VoiceInfo | string): Voice {
  const id = typeof v === "string" ? v : v.id;
  const label = typeof v === "string" ? "" : v.label;
  const m = /^(\w+)\s+(female|male)\s*:\s*(.+)$/i.exec(label);
  if (m) return { id, accent: cap(m[1]), sex: m[2].toLowerCase() as Sex, name: m[3].trim() };
  const [head = "", rest = id] = id.includes("_") ? id.split("_", 2) : ["", id];
  return {
    id,
    accent: LANG[head[0]] ?? "Other",
    sex: head[1] === "f" ? "female" : head[1] === "m" ? "male" : "other",
    name: cap(rest),
  };
}

/** "British male", the voice's description everywhere it is shown. */
export function voiceKind(v: Voice): string {
  return v.sex === "other" ? v.accent : `${v.accent} ${v.sex}`;
}

export interface AccentGroup {
  accent: string;
  female: Voice[];
  male: Voice[];
  other: Voice[];
}

export function groupVoices(voices: Voice[]): AccentGroup[] {
  const by = new Map<string, AccentGroup>();
  for (const v of voices) {
    const g = by.get(v.accent) ?? { accent: v.accent, female: [], male: [], other: [] };
    g[v.sex].push(v);
    by.set(v.accent, g);
  }
  const rank = (a: string) => {
    const i = ACCENT_ORDER.indexOf(a);
    return i < 0 ? ACCENT_ORDER.length : i;
  };
  const byName = (a: Voice, b: Voice) => a.name.localeCompare(b.name);
  return [...by.values()]
    .sort((a, b) => rank(a.accent) - rank(b.accent) || a.accent.localeCompare(b.accent))
    .map((g) => ({ ...g, female: g.female.sort(byName), male: g.male.sort(byName), other: g.other.sort(byName) }));
}
