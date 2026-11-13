/**
 * The floating surface's own commands and the shell's word (`src-tauri/src/floating/`). In a
 * plain browser (the mock, screenshots) they do nothing and the URL stands in for the shell:
 * `&main=active` (the main window is the one in use), `&capture=yes|no|unknown`,
 * `&covered=1` (a full-screen app has the only monitor), `&side=left|right`,
 * `&placement=beside|centre|corner`, `&captions=off|muted|always`, `&sound=muted`,
 * `&sharing=hidden|shown|asking` (with `&capture=yes`: what the operator chose for the share).
 */
import { useEffect, useState } from "react";

import type { FloatingCaptions, FloatingPlacement } from "../app/floating";
import { inTauri } from "../transport";
import { type Capture, type Sharing, sharingOf } from "./scene";

export interface ShellWord {
  mainActive: boolean;
  capture: Capture | null;
  /** What to do about a share now (the shell's `share.rs`). */
  sharing: Sharing;
  placement: FloatingPlacement;
  captions: FloatingCaptions;
}

export interface Sound {
  volume: number;
  muted: boolean;
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

/** "Send it away": the session ends, as `ghostd sleep` or "Ghost, goodbye" would. */
export function sendAway(sleep: () => Promise<unknown>): void {
  void sleep().catch((e) => log(`send away failed: ${String(e)}`));
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

/** The share card's answer: Ghost in this share, or kept out of it (for this share only). */
export function answerShare(show: boolean): void {
  void invoke("floating_share", { show });
}

/** "Open in Ghost": the shell brings the main window to this workspace and to the front. */
export function openApp(): void {
  void invoke("floating_open_app");
}

/**
 * The default output's volume and mute (`wpctl`, read only), for captions "when muted"; null
 * when it can't be read.
 */
export async function sound(): Promise<Sound | null> {
  if (!inTauri) return params().get("sound") === "muted" ? { volume: 0.4, muted: true } : { volume: 0.4, muted: false };
  return invoke<Sound>("floating_sound");
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
    const pl = p.get("placement");
    const cap = p.get("captions");
    const sh = p.get("sharing");
    const capture: Capture | null = c === "yes" || c === "no" || c === "unknown" ? c : null;
    return {
      mainActive: p.get("main") === "active",
      capture,
      sharing: sharingOf(capture, sh === "hidden" || sh === "shown" || sh === "asking" ? sh : null),
      placement: pl === "centre" || pl === "corner" ? pl : "beside",
      captions: cap === "muted" || cap === "always" ? cap : "off",
    };
  });
  useEffect(() => {
    if (!inTauri) return;
    let stop: (() => void) | null = null;
    let live = true;
    void (async () => {
      type Settings = { placement?: FloatingPlacement; captions?: FloatingCaptions };
      const first = await invoke<{ main_active: boolean; capture: Capture | null; sharing?: Sharing; settings: Settings }>("floating_shell");
      if (live && first)
        setWord({
          mainActive: first.main_active,
          capture: first.capture,
          sharing: sharingOf(first.capture, first.sharing),
          placement: first.settings?.placement ?? "beside",
          captions: first.settings?.captions ?? "off",
        });
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<{ t: string; active?: boolean; capture?: Capture; sharing?: Sharing; settings?: Settings }>("floating://shell", (e) => {
        const m = e.payload;
        if (m.t === "main_active") setWord((w) => ({ ...w, mainActive: !!m.active }));
        else if (m.t === "capture" && m.capture) setWord((w) => ({ ...w, capture: m.capture!, sharing: sharingOf(m.capture!, m.sharing) }));
        else if (m.t === "settings" && m.settings) setWord((w) => ({ ...w, placement: m.settings!.placement ?? "beside", captions: m.settings!.captions ?? "off" }));
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
