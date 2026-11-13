/**
 * The floating surface's own commands and the shell's word (`src-tauri/src/floating/`). In a
 * plain browser (the mock, screenshots) they do nothing and the URL stands in for the shell:
 * `&main=active` (the main window is the one in use), `&capture=yes|no|unknown`,
 * `&covered=1` (a full-screen app has the only monitor), `&side=left|right`.
 */
import { useEffect, useState } from "react";

import { inTauri } from "../transport";
import type { Capture } from "./scene";

export interface ShellWord {
  mainActive: boolean;
  capture: Capture | null;
}

export interface Summoned {
  choice: { monitor: number; side: "centre" | "left" | "right"; why: string; covered: boolean } | null;
  monitor: { name: string } | null;
  workspace: number | null;
  source: string;
}

const params = () => new URLSearchParams(window.location.search);

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  if (!inTauri) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args).catch(() => null);
}

/** Where Ghost is drawn, in CSS pixels: everywhere else passes clicks through. */
export function setRegion(rects: [number, number, number, number][]): void {
  void invoke("floating_region", { rects });
}

/** The wake: the shell picks the monitor from what it knows now and moves there. */
export async function summon(): Promise<Summoned> {
  const got = await invoke<Summoned>("floating_summon");
  if (got) return got;
  const p = params();
  const side = p.get("side");
  return {
    choice: { monitor: 0, side: side === "left" || side === "right" ? side : "centre", why: "only", covered: p.get("covered") === "1" },
    monitor: null,
    workspace: null,
    source: "nothing",
  };
}

/** "Open in Ghost": the shell brings the main window to this workspace and to the front. */
export function openApp(): void {
  void invoke("floating_open_app");
}

/** A line for the journal (frame timings, decisions). */
export function log(line: string): void {
  if (inTauri) void invoke("floating_log", { line });
  else if (params().has("log")) console.info(`floating ${line}`);
}

/** What the shell last said: whether the main window is in use, and whether the screen is captured. */
export function useShellWord(): ShellWord {
  const [word, setWord] = useState<ShellWord>(() => {
    const p = params();
    const c = p.get("capture");
    return { mainActive: p.get("main") === "active", capture: c === "yes" || c === "no" || c === "unknown" ? c : null };
  });
  useEffect(() => {
    if (!inTauri) return;
    let stop: (() => void) | null = null;
    let live = true;
    void (async () => {
      const first = await invoke<{ main_active: boolean; capture: Capture | null }>("floating_shell");
      if (live && first) setWord({ mainActive: first.main_active, capture: first.capture });
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<{ t: string; active?: boolean; capture?: Capture }>("floating://shell", (e) => {
        const m = e.payload;
        if (m.t === "main_active") setWord((w) => ({ ...w, mainActive: !!m.active }));
        else if (m.t === "capture" && m.capture) setWord((w) => ({ ...w, capture: m.capture! }));
      });
      if (live) stop = un;
      else un();
    })();
    return () => {
      live = false;
      stop?.();
    };
  }, []);
  return word;
}

/** Rectangles of every element marked `data-hit` that is showing. */
export function hitRects(root: HTMLElement): [number, number, number, number][] {
  const out: [number, number, number, number][] = [];
  root.querySelectorAll<HTMLElement>("[data-hit]").forEach((el) => {
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== "hidden") out.push([r.x, r.y, r.width, r.height]);
  });
  return out;
}
