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
  passwords: { path: "/settings/developer/passwords", label: "Open Passwords" },
  "outside-tools": { path: "/settings/developer/tools", label: "Open Outside tools" },
  developer: { path: "/settings/developer/debug", label: "Open Settings › Developer" },
  apps: { path: "/apps", label: "Open Apps" },
  // The daemon still calls this place `watching`; the operator meets it as On its own.
  watching: { path: "/on-its-own", label: "See what Ghost does on its own" },
};

/**
 * Ghost's own notes, by their key (Ghost's, never an app's text), to the section of the place
 * that fixes them: the microphone's note to Microphone and speakers, not the top of Sound.
 */
const SECTIONS: Record<string, string> = {
  "input:degraded": "/settings/sound/audio",
  "output:unavailable": "/settings/sound/audio",
  "models:missing": "/settings/sound/models",
  "voice:realtime": "/settings/sound/voice-mode",
  "dictation:key": "/settings/sound/dictation",
};

/**
 * An app's own note (`app:<name>`, `app:<name>:sign-in`, `app:<name>:client`): its page. A
 * sign-in that lapsed is one action, Sign in again, right there (`signIn`); one that needs the
 * person's own client ID leads to the page that asks for it (ADR 0076).
 */
const APP_KEY = /^app:([A-Za-z0-9_-]{1,64})(?::(sign-in|client))?$/;

/** The route and words for an item's place, or null. */
export function placeOf(item: Pick<WaitingItemView, "place"> & { key?: string }): { path: string; label: string; signIn?: string } | null {
  const p = item.place;
  if (!p || !Object.prototype.hasOwnProperty.call(PLACES, p)) return null;
  const base = PLACES[p as Place];
  const key = item.key ?? "";
  const app = p === "apps" ? APP_KEY.exec(key) : null;
  if (app) {
    const path = `/apps/${encodeURIComponent(app[1])}`;
    if (app[2] === "sign-in") return { path, label: "Sign in again", signIn: app[1] };
    if (app[2] === "client") return { path, label: "Give it a client ID" };
    return { path, label: "Open the app" };
  }
  const section = SECTIONS[key];
  return section && section.startsWith(base.path) ? { path: section, label: base.label } : base;
}
