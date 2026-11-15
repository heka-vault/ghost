/**
 * Owner: app. Removing Ghost from this computer (#147): the shell's one command for it. The
 * window can't remove its own storage while it is open, so the shell copies `ghostd` out of
 * the bundle, starts `ghostd uninstall --only <kinds> --yes --after <its pid>` on its own, and
 * quits; that process waits for the app to be gone, removes what was chosen, and says so in a
 * desktop notification. Only this window calls it; nothing a model, a tool or an app does
 * reaches it.
 */
import { inTauri } from "../transport";

export type LeftoverKind = "running" | "other-tools" | "speech-models" | "app-storage" | "data" | "vault";

/** Start the removal and quit the app. Outside Tauri (the browser, the mock) it does nothing. */
export async function removeGhostAndQuit(kinds: LeftoverKind[]): Promise<void> {
  if (!inTauri) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke<void>("remove_ghost", { kinds });
}
