/**
 * The floating room's switch and state, as the shell keeps them (`src-tauri/src/floating/`,
 * ADR 0059): whether this desktop can float a window, the operator's choice, whether the
 * surface is up, whether the screen is being captured, the monitors, and the GNOME Shell
 * helper. In a browser (the mock) `&floating=on|off|refused` (on unless said, as the shell is
 * since 2026-11-25), `&capture=yes` and `&while_shared=hide|show|ask` stand in.
 */
import { useCallback, useEffect, useRef, useState } from "react";

import type { Sharing } from "../floating/scene";
import { inTauri } from "../transport";

export type MonitorPref = "other" | "here" | { named: string };

/** Where on the monitor it comes forward (`src/floating/layout.ts`). */
export type FloatingPlacement = "beside" | "centre" | "corner";
/** When Ghost's spoken words are written under it. */
export type FloatingCaptions = "off" | "muted" | "always";
/** While the screen is shared: shown with the shared mark (default since 2026-11-25), kept out, or asked each time. */
export type WhileShared = "hide" | "show" | "ask";

export interface FloatingSettings {
  on: boolean;
  monitor: MonitorPref;
  /** Older shells don't send these: beside the work, no captions. */
  placement?: FloatingPlacement;
  captions?: FloatingCaptions;
  /** Older shells don't send it: shown while shared (the default since 2026-11-25). */
  while_shared?: WhileShared;
  /** The first run offered it, 2026-11-14 to 2026-11-25; kept in the file, decides nothing now. */
  offered?: boolean;
}

export interface FloatingStatus {
  support: { kind: "x11" | "xwayland" } | { kind: "no"; why: string };
  settings: FloatingSettings;
  running: boolean;
  capture: "yes" | "no" | "unknown";
  /** What the floating room does about a share now; older shells don't send it. */
  sharing?: Sharing;
  monitors: { name: string; geometry: { x: number; y: number; width: number; height: number }; primary: boolean }[];
  helper: boolean;
}

/** The posture rule's `floating` input: on, up, and not kept out of a share. */
export function canFloat(s: FloatingStatus | null): boolean {
  if (!s || !s.settings.on || !s.running) return false;
  const sharing = s.sharing ?? (s.capture === "yes" ? "hidden" : "none");
  return sharing === "none" || sharing === "shown";
}

/** The monitor choice as the select's value: `other`, `here`, or `named:HDMI-3`. */
export function prefValue(p: MonitorPref): string {
  return typeof p === "string" ? p : `named:${p.named}`;
}

export function prefOf(v: string): MonitorPref {
  return v.startsWith("named:") ? { named: v.slice(6) } : v === "here" ? "here" : "other";
}

function mockStatus(): FloatingStatus {
  const p = new URLSearchParams(window.location.search);
  const f = p.get("floating");
  const c = p.get("capture");
  const sh = p.get("sharing");
  const ws = p.get("while_shared");
  const whileShared: WhileShared = ws === "hide" || ws === "ask" ? ws : "show";
  return {
    support: f === "refused" ? { kind: "no", why: "Ghost needs XWayland to float, and this desktop isn't running it." } : { kind: "xwayland" },
    settings: { on: f !== "off" && f !== "refused", monitor: "other", placement: "beside", captions: "off", while_shared: whileShared },
    running: f !== "off" && f !== "refused",
    capture: c === "yes" ? "yes" : c === "unknown" ? "unknown" : "no",
    sharing: c !== "yes" ? "none" : sh === "shown" || sh === "asking" || sh === "hidden" ? sh : whileShared === "hide" ? "hidden" : whileShared === "ask" ? "asking" : "shown",
    monitors: [
      { name: "HDMI-1", geometry: { x: 0, y: 0, width: 1920, height: 1080 }, primary: true },
      { name: "HDMI-3", geometry: { x: 1920, y: 0, width: 1920, height: 1080 }, primary: false },
    ],
    helper: false,
  };
}

/** The first run's wake beat is open: the surface comes forward over the window (`floating_first_meeting`). */
export async function setFirstMeeting(on: boolean): Promise<void> {
  if (!inTauri) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("floating_first_meeting", { on }).catch(() => undefined);
}

/** Ghost came forward over the desktop (the surface told the shell): for the first run. */
export function useFloatingForward(onForward: () => void): void {
  const cb = useRef(onForward);
  cb.current = onForward;
  useEffect(() => {
    if (!inTauri) return;
    let live = true;
    let stop: (() => void) | null = null;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen("ghost://floating-forward", () => cb.current());
      if (live) stop = un;
      else un();
    })();
    return () => {
      live = false;
      stop?.();
    };
  }, []);
}

export function useFloating(): { status: FloatingStatus | null; set: (s: FloatingSettings) => Promise<void>; error: string | null } {
  const [status, setStatus] = useState<FloatingStatus | null>(() => (inTauri ? null : mockStatus()));
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!inTauri) return;
    let live = true;
    let stop: (() => void) | null = null;
    void (async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");
      const first = await invoke<FloatingStatus>("floating_status").catch(() => null);
      if (live && first) setStatus(first);
      const un = await listen<FloatingStatus>("ghost://floating", (e) => setStatus(e.payload));
      if (live) stop = un;
      else un();
    })();
    return () => {
      live = false;
      stop?.();
    };
  }, []);
  const set = useCallback(async (settings: FloatingSettings) => {
    setError(null);
    if (!inTauri) {
      setStatus((s) => (s ? { ...s, settings, running: settings.on && s.support.kind !== "no" } : s));
      return;
    }
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      setStatus(await invoke<FloatingStatus>("floating_set", { settings }));
    } catch (e) {
      setError(String(e));
    }
  }, []);
  return { status, set, error };
}

/** This window has the keyboard and is on screen: the operator is using it. */
export function useWindowActive(): boolean {
  const read = () => typeof document !== "undefined" && document.hasFocus() && document.visibilityState === "visible";
  const [active, setActive] = useState(read);
  useEffect(() => {
    const update = () => setActive(read());
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  return active;
}
