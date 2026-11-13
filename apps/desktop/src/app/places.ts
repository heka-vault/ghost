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

/** The route and words for an item's place, or null. */
export function placeOf(item: Pick<WaitingItemView, "place">): { path: string; label: string } | null {
  const p = item.place;
  return p && Object.prototype.hasOwnProperty.call(PLACES, p) ? PLACES[p as Place] : null;
}
