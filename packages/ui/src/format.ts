/**
 * Plain-word helpers shared by the desktop and phone apps: no React, no DOM, so React Native
 * can import them too.
 */
import type { Build, Status } from "@ghost/client-ts";
import { phaseLabel } from "./labels";

/** A badge or dot tone. Semantic colors only mean what they say. */
export type Tone = "neutral" | "accent" | "ok" | "warn" | "danger" | "info";

/** Ghost's phase as the apps show it: the daemon's phases plus offline and starting. */
export type GhostPhase = "starting" | "passive" | "listening" | "thinking" | "speaking" | "hold" | "asking" | "offline";

/**
 * How the daemon was built, after the version's middle dot: "dev a1b2c3d", "deploy a1b2c3d",
 * or "release". Uncommitted changes aren't in it; say them where there is room.
 */
export function buildText(build: Build): string {
  return build.commit && build.kind !== "release" ? `${build.kind} ${build.commit.slice(0, 7)}` : build.kind;
}

/**
 * Ghost's version line, as the sidebar says it: "Ghost 2026.10.0 · dev a1b2c3d". A release
 * names no commit; its version is the tag (docs/releasing.md).
 */
export function versionLine(version: string, build: Build): string {
  return build.kind === "release" ? `Ghost ${version}` : `Ghost ${version} · ${buildText(build)}`;
}

/** The phase to show: offline when the daemon is unreachable, starting until models load. */
export function phaseOf(status: Status | null, connected: boolean): GhostPhase {
  if (!connected || !status) return "offline";
  if (!status.models_ready && status.phase === "passive") return "starting";
  return status.phase;
}

/** Plain words for a phase, in the operator's voice. */
export function phaseText(phase: GhostPhase): string {
  if (phase === "offline") return "Not running";
  if (phase === "starting") return "Starting up";
  return phaseLabel(phase);
}

/** Display name for an agent slug when only the slug is known. */
export function agentName(slug: string | null | undefined, name?: string): string {
  if (!slug || slug === "ghost") return "Ghost";
  if (name) return name;
  return slug.replace(/-/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

/** The older name of agentName, kept for the phone app. */
export const lineName = agentName;

/**
 * Short relative time for meta lines, written out by hand so it reads cleanly at 12px:
 * "now", "4 min ago", "3 h ago", "yesterday", "5 days ago", then "Oct 9". Future times
 * (an expiry) read "in 4 min". The one helper for relative time; don't use Intl's
 * abbreviations ("4 min. ago").
 */
export function relativeTime(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const s = Math.round((now - t) / 1000);
  const a = Math.abs(s);
  const say = (n: number, unit: string) => (s >= 0 ? `${n} ${unit} ago` : `in ${n} ${unit}`);
  if (a < 45) return "now";
  if (a < 3600) return say(Math.max(1, Math.round(a / 60)), "min");
  if (a < 86400) return say(Math.round(a / 3600), "h");
  const days = calendarDays(t, now);
  if (days === 1) return "yesterday";
  if (days === -1) return "tomorrow";
  if (Math.abs(days) < 7) return s >= 0 ? `${Math.abs(days)} days ago` : `in ${Math.abs(days)} days`;
  const d = new Date(iso);
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return d.toLocaleDateString(undefined, sameYear ? { month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" });
}

/** Whole calendar days from t to now in local time (1 = yesterday). */
function calendarDays(t: number, now: number): number {
  const day = (x: number) => {
    const d = new Date(x);
    return Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / 86400000;
  };
  return day(now) - day(t);
}

export function formatTime(iso: string): string {
  return new Date(iso).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

export function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

/** A readable message from anything thrown by the client. */
export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/** Curly quotes around spoken words in UI copy: quoted("Hey Ghost") is “Hey Ghost”. */
export function quoted(text: string): string {
  return `\u201c${text}\u201d`;
}

const STATUS: Record<string, { label: string; tone: Tone }> = {
  pending: { label: "Waiting", tone: "warn" },
  claimed: { label: "Working", tone: "info" },
  executed: { label: "Done", tone: "ok" },
  expired: { label: "Expired", tone: "neutral" },
  cancelled: { label: "Canceled", tone: "neutral" },
  stale: { label: "Changed first", tone: "warn" },
  failed: { label: "Failed", tone: "danger" },
};

/**
 * A change's status in plain words, for badges: statusLabel("executed") is "Done". Covers
 * every proposal status the gate writes (pending, claimed, executed, expired,
 * cancelled, stale) plus failed; anything else is shown sentence-cased as it came.
 */
export function statusLabel(status: string): string {
  return STATUS[status]?.label ?? (status ? status[0].toUpperCase() + status.slice(1).replace(/_/g, " ") : "");
}

/** The Badge tone that goes with statusLabel(). */
export function statusTone(status: string): Tone {
  return STATUS[status]?.tone ?? "neutral";
}
