/**
 * Owner: app. Updates (ADR 0066): what the app's own updater says (`updater.rs` in the
 * shell), and the operator's three actions on it: check now, the "Check for updates"
 * switch, and "Update and restart". Only this window calls these; nothing a model, a tool or
 * an app does reaches them.
 *
 * Outside Tauri (the browser, the mock) there is no updater; `?update=available|dev|failed`
 * on a mock address shows the designed states for screenshots.
 */
import { useEffect, useState } from "react";

import { inTauri } from "../transport";

export type UpdatePhase =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "waiting"; reason: string }
  | { state: "downloading"; done: number; total: number | null }
  | { state: "installing" }
  | { state: "restarting" }
  | { state: "failed"; error: string };

export interface UpdateView {
  current: string;
  installer: "deb" | "appimage" | null;
  unavailable: string | null;
  check: boolean;
  checked_at: number | null;
  available: { version: string; notes: string; date: string | null } | null;
  phase: UpdatePhase;
}

const NOTES = "## What's changed\n* The first run downloads Ghost's ears and voice itself\n* Ghost updates itself from Settings\n\n**Full Changelog**: v2026.11.0...v2026.11.1";

/** The designed states, for the mock only. */
export function designedUpdate(kind: string | null, current: string): UpdateView | null {
  const base: UpdateView = { current, installer: "deb", unavailable: null, check: true, checked_at: Date.now() / 1000 - 600, available: null, phase: { state: "idle" } };
  if (kind === "available") return { ...base, available: { version: "2026.11.1", notes: NOTES, date: "2026-11-14T10:00:00Z" } };
  if (kind === "downloading") return { ...base, available: { version: "2026.11.1", notes: NOTES, date: null }, phase: { state: "downloading", done: 61e6, total: 148e6 } };
  if (kind === "waiting") return { ...base, available: { version: "2026.11.1", notes: NOTES, date: null }, phase: { state: "waiting", reason: "Ghost will update when this conversation ends." } };
  if (kind === "failed") return { ...base, available: { version: "2026.11.1", notes: NOTES, date: null }, phase: { state: "failed", error: `Installing 2026.11.1 did not finish: the package could not be installed. Ghost ${current} keeps running; nothing was changed.` } };
  if (kind === "current") return base;
  if (kind === "dev") return { ...base, installer: null, unavailable: "This is a development build; it does not update itself. Pull and rebuild the checkout instead.", checked_at: null };
  return null;
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export const checkForUpdate = () => invoke<UpdateView>("update_check");
export const setUpdateCheck = (on: boolean) => invoke<UpdateView>("update_set_check", { on });
export const installUpdate = () => invoke<UpdateView>("update_install");
export const cancelUpdate = () => invoke<void>("update_cancel");

/** The updater's view, live; null outside Tauri (unless the mock designs one). */
export function useUpdate(current: string): [UpdateView | null, (v: UpdateView) => void] {
  const designed = typeof window === "undefined" ? null : new URLSearchParams(window.location.search).get("update");
  const [view, setView] = useState<UpdateView | null>(() => (inTauri ? null : designedUpdate(designed, current)));
  useEffect(() => {
    if (!inTauri) return;
    let off: (() => void) | undefined;
    let gone = false;
    void invoke<UpdateView>("update_view").then((v) => !gone && setView(v), () => undefined);
    void import("@tauri-apps/api/event").then(({ listen }) =>
      listen<UpdateView>("ghost://update", (e) => setView(e.payload)).then((u) => {
        if (gone) u();
        else off = u;
      }),
    );
    return () => {
      gone = true;
      off?.();
    };
  }, []);
  return [view, setView];
}

/** "Update available: 2026.11.1", or null when there is nothing to say. */
export function availableLine(v: UpdateView | null): string | null {
  return v?.available && !v.unavailable ? `Update available: ${v.available.version}` : null;
}

/** The notes as plain text: GitHub's markdown marks taken off, nothing rendered as HTML. */
export function plainNotes(notes: string): string {
  return notes
    .split("\n")
    .map((l) => l.replace(/^#{1,6}\s+/, "").replace(/^\s*[*-]\s+/, "• ").replace(/\*\*(.+?)\*\*/g, "$1").replace(/`([^`]+)`/g, "$1").replace(/\[([^\]]+)\]\([^)]+\)/g, "$1"))
    .join("\n")
    .trim();
}
