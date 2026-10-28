/**
 * Marks on the settings nav: a page with unsaved changes, one that fails a check, or one
 * that waits for a restart. The settings screen publishes them (it owns the draft); the
 * settings nav reads them. A tiny store, so the nav in the shell needn't know the screen.
 */
import { useSyncExternalStore } from "react";

export type SettingsMark = "dirty" | "restart" | "error";
export type SettingsMarks = Readonly<Record<string, SettingsMark | undefined>>;

let marks: SettingsMarks = {};
const listeners = new Set<() => void>();

/** Replace every mark; a page left out has none. */
export function setSettingsMarks(next: SettingsMarks) {
  const keys = new Set([...Object.keys(marks), ...Object.keys(next)]);
  if ([...keys].every((k) => marks[k] === next[k])) return;
  marks = next;
  for (const l of listeners) l();
}

export function useSettingsMarks(): SettingsMarks {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => marks,
  );
}
