/**
 * Owner: watching. When a schedule runs, as plain choices (every weekday at a time, every day,
 * some days, every few hours, once on a date), turned into what the daemon takes (`every`,
 * `cron`, `at`) and read back from it. The cron form is one click further, for a schedule the
 * plain choices can't say; one they can say opens as the plain choice. Pure and tested.
 */
import type { ScheduleInfo } from "@ghost/client-ts";

export type PlainMode = "weekdays" | "daily" | "days" | "interval" | "once" | "cron";
export type Unit = "minutes" | "hours" | "days";

export interface Plain {
  mode: PlainMode;
  /** 24-hour "HH:MM" for weekdays, daily and days. */
  time: string;
  /** 0 Sunday … 6 Saturday, for "days". */
  days: number[];
  /** For "interval". */
  n: number;
  unit: Unit;
  /** "YYYY-MM-DDTHH:MM" in local time, for "once". */
  date: string;
  /** The raw expression, for "cron". */
  cron: string;
}

export const DAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
export const DAY_LONG = ["Sundays", "Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays"];

export const PLAIN_DEFAULT: Plain = { mode: "weekdays", time: "09:00", days: [1], n: 2, unit: "hours", date: "", cron: "" };

const TIME = /^([01]\d|2[0-3]):([0-5]\d)$/;
const UNIT_LETTER: Record<Unit, string> = { minutes: "m", hours: "h", days: "d" };

/** What the daemon takes, or a sentence saying what is missing. */
export function requestOf(p: Plain): { kind: ScheduleInfo["kind"]; when: string } | { problem: string } {
  const hm = () => {
    const m = TIME.exec(p.time);
    return m ? { h: Number(m[1]), m: Number(m[2]) } : null;
  };
  switch (p.mode) {
    case "weekdays":
    case "daily":
    case "days": {
      const t = hm();
      if (!t) return { problem: "Pick a time." };
      if (p.mode === "days" && p.days.length === 0) return { problem: "Pick at least one day." };
      const dow = p.mode === "weekdays" ? "1-5" : p.mode === "daily" ? "*" : [...new Set(p.days)].sort((a, b) => a - b).join(",");
      return { kind: "cron", when: `${t.m} ${t.h} * * ${dow}` };
    }
    case "interval": {
      if (!Number.isInteger(p.n) || p.n < 1) return { problem: "A whole number, 1 or more." };
      if (p.unit === "minutes" && p.n < 5) return { problem: "Every 5 minutes at the most often." };
      return { kind: "every", when: `${p.n}${UNIT_LETTER[p.unit]}` };
    }
    case "once":
      if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(p.date)) return { problem: "Pick a date and a time." };
      return { kind: "at", when: p.date };
    case "cron":
      if (p.cron.trim().split(/\s+/).length !== 5) return { problem: "Five fields: minute, hour, day of month, month, day of week." };
      return { kind: "cron", when: p.cron.trim() };
  }
}

/** A stored schedule as the plain choice that says it, or the cron form when none does. */
export function plainOf(kind: ScheduleInfo["kind"], when: string): Plain {
  const base = { ...PLAIN_DEFAULT };
  if (kind === "at") return { ...base, mode: "once", date: when };
  if (kind === "every") {
    const m = /^(\d+)\s*([mhd])$/.exec(when.trim());
    if (m) return { ...base, mode: "interval", n: Number(m[1]), unit: m[2] === "m" ? "minutes" : m[2] === "d" ? "days" : "hours" };
    return { ...base, mode: "cron", cron: "" };
  }
  const c = /^(\d{1,2}) (\d{1,2}) \* \* (\*|1-5|[0-6](?:,[0-6])*)$/.exec(when.trim());
  if (!c || Number(c[1]) > 59 || Number(c[2]) > 23) return { ...base, mode: "cron", cron: when };
  const time = `${c[2].padStart(2, "0")}:${c[1].padStart(2, "0")}`;
  if (c[3] === "1-5") return { ...base, mode: "weekdays", time };
  if (c[3] === "*") return { ...base, mode: "daily", time };
  return { ...base, mode: "days", time, days: c[3].split(",").map(Number) };
}

/** "9:00 AM", "6:30 PM". */
export function clock(time: string): string {
  const m = TIME.exec(time);
  if (!m) return time;
  const h = Number(m[1]);
  const mm = m[2];
  return `${h % 12 || 12}${mm === "00" ? "" : `:${mm}`} ${h < 12 ? "AM" : "PM"}`;
}

/** The choice read back in words, as the row will say it: “Weekdays at 9 AM”. */
export function plainWords(p: Plain): string | null {
  const r = requestOf(p);
  if ("problem" in r) return null;
  switch (p.mode) {
    case "weekdays":
      return `Weekdays at ${clock(p.time)}`;
    case "daily":
      return `Every day at ${clock(p.time)}`;
    case "days": {
      const d = [...new Set(p.days)].sort((a, b) => a - b);
      if (d.length === 7) return `Every day at ${clock(p.time)}`;
      const names = d.map((x) => DAY_LONG[x]);
      const list = names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
      return `${list} at ${clock(p.time)}`;
    }
    case "interval": {
      const unit = p.n === 1 ? p.unit.slice(0, -1) : p.unit;
      return p.n === 1 ? `Every ${unit}` : `Every ${p.n} ${unit}`;
    }
    case "once": {
      const d = new Date(p.date);
      return Number.isNaN(d.getTime()) ? null : `Once, ${d.toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}`;
    }
    case "cron":
      return null;
  }
}

/** How long a run took, in words: “4 s”, “1 min 12 s”. */
export function tookWords(started: string | null | undefined, finished: string | null | undefined): string | null {
  if (!started || !finished) return null;
  const s = Math.max(0, Math.round((Date.parse(finished) - Date.parse(started)) / 1000));
  if (Number.isNaN(s)) return null;
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
}
