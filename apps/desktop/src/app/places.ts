/**
 * Where a waiting item leads inside the app (`WaitingItemView.place`): a closed set the daemon
 * gives only to Ghost's own notes, each mapped here to a route and the words of its button.
 * Never a URL from an item: an unknown place leads nowhere.
 */
import type { WaitingItemView } from "@ghost/client-ts";

type Place = NonNullable<WaitingItemView["place"]>;

export const PLACES: Record<Place, { path: string; label: string }> = {
  model: { path: "/settings/model", label: "Open Settings › Model" },
  sound: { path: "/settings/sound", label: "Open Settings › Sound" },
  passwords: { path: "/settings/passwords", label: "Open Settings › Passwords" },
  "outside-tools": { path: "/settings/tools", label: "Open Settings › Outside tools" },
  apps: { path: "/apps", label: "Open Apps" },
  watching: { path: "/watching", label: "Open Watching" },
};

/**
 * Ghost's own notes, by their key (Ghost's, never an app's text), to the section of the place
 * that fixes them: the microphone's note to Microphone and speakers, not the top of Sound.
 */
const SECTIONS: Record<string, string> = {
  "input:degraded": "/settings/sound/audio",
  "models:missing": "/settings/sound/models",
  "voice:realtime": "/settings/sound/voice-mode",
};

/** An app's own note (`app:<name>`): its page, where Sign in again is. */
const APP_KEY = /^app:([A-Za-z0-9_-]{1,64})$/;

/** The route and words for an item's place, or null. */
export function placeOf(item: Pick<WaitingItemView, "place"> & { key?: string }): { path: string; label: string } | null {
  const p = item.place;
  if (!p || !Object.prototype.hasOwnProperty.call(PLACES, p)) return null;
  const base = PLACES[p as Place];
  const key = item.key ?? "";
  const app = p === "apps" ? APP_KEY.exec(key) : null;
  if (app) return { path: `/apps/${encodeURIComponent(app[1])}`, label: "Open the app" };
  const section = SECTIONS[key];
  return section && section.startsWith(base.path) ? { path: section, label: base.label } : base;
}
