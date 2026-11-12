/**
 * The floating room's switch and state, as the shell keeps them (`src-tauri/src/floating/`,
 * ADR 0059): whether this desktop can float a window, the operator's choice, whether the
 * surface is up, whether the screen is being captured, the monitors, and the GNOME Shell
 * helper. In a browser (the mock) `&floating=on|off|refused` and `&capture=yes` stand in.
 */
import { useCallback, useEffect, useState } from "react";

import { inTauri } from "../transport";

export type MonitorPref = "other" | "here" | { named: string };

export interface FloatingSettings {
  on: boolean;
  monitor: MonitorPref;
}

export interface FloatingStatus {
  support: { kind: "x11" | "xwayland" } | { kind: "no"; why: string };
  settings: FloatingSettings;
  running: boolean;
  capture: "yes" | "no" | "unknown";
  monitors: { name: string; geometry: { x: number; y: number; width: number; height: number }; primary: boolean }[];
  helper: boolean;
}

/** The posture rule's `floating` input: on, up, and nothing capturing the screen. */
export function canFloat(s: FloatingStatus | null): boolean {
  return !!s && s.settings.on && s.running && s.capture !== "yes";
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
  return {
    support: f === "refused" ? { kind: "no", why: "The floating room needs XWayland, and this desktop isn't running it." } : { kind: "xwayland" },
    settings: { on: f === "on", monitor: "other" },
    running: f === "on",
    capture: c === "yes" ? "yes" : c === "unknown" ? "unknown" : "no",
    monitors: [
      { name: "HDMI-1", geometry: { x: 0, y: 0, width: 1920, height: 1080 }, primary: true },
      { name: "HDMI-3", geometry: { x: 1920, y: 0, width: 1920, height: 1080 }, primary: false },
    ],
    helper: false,
  };
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
