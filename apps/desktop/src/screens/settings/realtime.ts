/**
 * Owner: settings. The voice mode (ADR 0063) in words and settings: what a choice saves,
 * what the spend and the check say. Pure; no React.
 */
import type { RealtimeCheckView, RealtimeSpendView, RealtimeView } from "@ghost/client-ts";

export type VoiceMode = "local" | "realtime";

export const MODE_KEY = "GHOST_VOICE_MODE";
export const REALTIME_KEYS = [
  MODE_KEY,
  "GHOST_REALTIME_PROVIDER",
  "GHOST_REALTIME_MODEL",
  "GHOST_REALTIME_VOICE",
  "GHOST_REALTIME_PASSWORD",
  "GHOST_REALTIME_DAILY_CAP_USD",
] as const;

/** What choosing a mode saves. Local is the default, so it is saved as nothing set. */
export function modeValues(mode: VoiceMode): Record<string, string | null> {
  return { [MODE_KEY]: mode === "realtime" ? "realtime" : null };
}

/** Turning realtime on asks first; going back to this machine never does. */
export const needsConsent = (from: VoiceMode, to: VoiceMode) => from === "local" && to === "realtime";

export const usd = (n: number) => (n > 0 && n < 0.01 ? "under a cent" : `$${n.toFixed(2)}`);

/** Today's spend in one line, or why there is none. */
export function spendLine(s: RealtimeSpendView): string {
  if (!s.priced) return "Ghost doesn’t know this model’s prices, so it can’t estimate the spend.";
  if (s.sessions === 0 && s.usd === 0) return "Nothing spent today.";
  const sessions = s.sessions === 1 ? "1 session" : `${s.sessions} sessions`;
  return `About ${usd(s.usd)} today over ${sessions}; ${usd(s.last_session_usd)} the last one.`;
}

export const SPEND_NOTE = (s: RealtimeSpendView) =>
  `An estimate from the provider’s own counts at its prices as of ${s.prices_as_of}; the provider’s bill is the truth.`;

/** The daily cap typed in, or an error. Empty is no cap. */
export function parseCap(raw: string): { value: string | null } | { error: string } {
  const t = raw.trim().replace(/^\$/, "");
  if (t === "") return { value: null };
  const n = Number(t);
  if (!Number.isFinite(n) || n < 0) return { error: "An amount in US dollars, like 1.50." };
  return { value: String(n) };
}

/** The latest check in one line. */
export function checkLine(c: RealtimeCheckView): string {
  if (!c.reachable) return `Didn’t connect: ${c.error ?? "no answer"}.`;
  const parts: string[] = [];
  if (c.first_audio_ms != null) parts.push(`first audio ${(c.first_audio_ms / 1000).toFixed(2)} s`);
  if (c.tools === true) parts.push("hands requests to Ghost’s agent");
  if (c.tools === false) parts.push("answered a request itself instead of handing it on");
  return `Connected: ${parts.join(", ")}.`;
}

export const checkTone = (c: RealtimeCheckView): "ok" | "warn" | "danger" => (!c.reachable ? "danger" : c.tools === false ? "warn" : "ok");

export function providerOf(v: RealtimeView) {
  return v.providers.find((p) => p.id === v.provider) ?? v.providers[0];
}
