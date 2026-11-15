/**
 * Owner: settings. The pure parts of "Remove Ghost from this computer" (#147), for the section
 * and its tests.
 */
import type { UninstallView } from "@ghost/client-ts";

/** Bytes as a person reads them (`ghostd::uninstall::human`). */
export function humanSize(bytes: number): string {
  if (bytes < 1000) return bytes === 0 ? "nothing" : `${bytes} bytes`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1000;
  let u = 0;
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000;
    u += 1;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[u]}`;
}

/** What is chosen when the section opens: everything there but the person's own. */
export const chosenAtFirst = (v: UninstallView): Set<string> => new Set(v.categories.filter((c) => c.items.length > 0 && !c.persons).map((c) => c.id));

