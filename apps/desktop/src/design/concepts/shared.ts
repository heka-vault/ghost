/**
 * Owner: design. Pieces both concepts use: the frame loop, coarse React subscriptions over the
 * engine, the design system's colours resolved for canvas, and the URL options.
 */
import { createContext, useContext, useEffect, useRef, useState } from "react";

import type { EngineHandle } from "./engine";

export interface Frame {
  handle: EngineHandle;
  /** Register a function run every frame after the clock ticks. */
  onFrame(f: (now: number) => void): () => void;
  reduced: boolean;
  theme: "dark" | "light";
}

export const FrameContext = createContext<Frame | null>(null);
export const useFrame = () => useContext(FrameContext)!;

/** Re-render when `key(engine)` changes, checked every frame. */
export function useKey<T>(key: (h: EngineHandle) => T): T {
  const { handle, onFrame } = useFrame();
  const [v, setV] = useState(() => key(handle));
  const ref = useRef(v);
  const fn = useRef(key);
  fn.current = key;
  useEffect(
    () =>
      onFrame(() => {
        const next = fn.current(handle);
        if (next !== ref.current) {
          ref.current = next;
          setV(next);
        }
      }),
    [handle, onFrame],
  );
  return v;
}

/** Resolve a CSS colour (a token like `var(--phase-speaking)`) to rgb() for canvas. */
export function resolveColors<K extends string>(el: HTMLElement, vars: Record<K, string>): Record<K, string> {
  const probe = document.createElement("span");
  el.appendChild(probe);
  const out = {} as Record<K, string>;
  for (const k of Object.keys(vars) as K[]) {
    probe.style.color = vars[k];
    out[k] = getComputedStyle(probe).color;
  }
  probe.remove();
  return out;
}

/** rgb(a, b, c) → rgba(a, b, c, alpha). */
export function alpha(rgb: string, a: number): string {
  const m = rgb.match(/[\d.]+/g);
  if (!m) return rgb;
  return `rgba(${m[0]}, ${m[1]}, ${m[2]}, ${Math.max(0, Math.min(1, a)).toFixed(3)})`;
}

export function urlOption(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

/** Size a canvas to its box at the device pixel ratio (capped at 2); returns css size. */
export function fitCanvas(c: HTMLCanvasElement): { w: number; h: number; dpr: number } {
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const w = c.clientWidth;
  const h = c.clientHeight;
  if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
  }
  return { w, h, dpr };
}
