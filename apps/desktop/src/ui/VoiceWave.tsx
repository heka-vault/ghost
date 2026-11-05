/**
 * Ghost speaking, drawn as a waveform that follows its voice. A test of three designs
 * against each other (the phone uses Wisp instead); the operator picks one on Settings ›
 * Sound, and `?wave=` sets it for a screenshot.
 *
 *   ribbon     a scrolling trace, the newest loudness entering on the right (a voice memo)
 *   strings    three soft sine lines whose height is the loudness (an oscilloscope)
 *   pulse      five bars beating together around the centre (an equalizer)
 *
 * The loudness is Ghost's own output, sent by the daemon about twenty times a second while it
 * speaks (`useGhost().voice`, never React state). The drawing is a canvas on one animation
 * frame loop that runs only while Ghost speaks or the wave is settling; at rest it draws once
 * and stops. Reduced motion draws one still frame of the design: the state still shows.
 */
import { type VoiceLevel, useGhost } from "@ghost/ui";
import { type CSSProperties, useEffect, useRef, useState, useSyncExternalStore } from "react";

import { cx } from "./cx";

export type WaveVariant = "ribbon" | "strings" | "pulse";
export const WAVE_VARIANTS: { value: WaveVariant; label: string; description: string }[] = [
  { value: "ribbon", label: "Ribbon", description: "A scrolling trace of Ghost’s voice, newest on the right." },
  { value: "strings", label: "Strings", description: "Three soft lines that rise and fall with the voice." },
  { value: "pulse", label: "Pulse", description: "Five bars beating together in the middle." },
];

// ---------------------------------------------------------------- the operator's pick

const KEY = "ghost.voiceWave";
const picked = new Set<() => void>();
const isVariant = (v: unknown): v is WaveVariant => v === "ribbon" || v === "strings" || v === "pulse";

function readVariant(): WaveVariant {
  const url = new URLSearchParams(window.location.search).get("wave");
  if (isVariant(url)) return url;
  const saved = localStorage.getItem(KEY);
  return isVariant(saved) ? saved : "ribbon";
}

export function setWaveVariant(v: WaveVariant) {
  localStorage.setItem(KEY, v);
  for (const fn of picked) fn();
}

/** Which design draws Ghost speaking: kept on this computer, the same in every window. */
export function useWaveVariant(): WaveVariant {
  return useSyncExternalStore(
    (fn) => {
      picked.add(fn);
      const onStorage = (e: StorageEvent) => e.key === KEY && fn();
      window.addEventListener("storage", onStorage);
      return () => {
        picked.delete(fn);
        window.removeEventListener("storage", onStorage);
      };
    },
    readVariant,
  );
}

// ---------------------------------------------------------------- a stand-in voice

const hash = (n: number) => {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
};

/**
 * A speech-like loudness at `t` seconds: syllables at about four a second, words with short
 * gaps between them, a longer pause every few seconds. For previews and the mock daemon, so
 * the designs can be judged without Ghost talking.
 */
export function speechEnvelope(t: number): number {
  const cycle = 0.9;
  const i = Math.floor(t / cycle);
  if (i % 7 === 6) return 0;
  const u = t / cycle - i;
  const word = 0.55 + 0.3 * hash(i);
  if (u > word) return 0;
  const syllable = Math.pow(Math.abs(Math.sin(Math.PI * t * (3.6 + 1.8 * hash(i + 7)))), 0.7);
  const loud = 0.5 + 0.5 * hash(i + 3);
  const edge = Math.min(1, u / 0.05, (word - u) / 0.08);
  return Math.max(0, Math.min(1, loud * (0.3 + 0.7 * syllable) * edge));
}

// ---------------------------------------------------------------- the drawing

export interface VoiceWaveProps {
  /** Ghost is speaking. Off: the wave settles to rest and stops drawing. */
  active: boolean;
  variant?: WaveVariant;
  /** Pixel size; width follows the element (CSS) when omitted. */
  width?: number;
  height: number;
  /** Draw the stand-in voice instead of Ghost's (previews, the gallery). */
  simulate?: boolean;
  /** Freeze the frame (reduced motion is followed by default). */
  still?: boolean;
  /** Accessible name; decorative when omitted (the phase is said elsewhere). */
  label?: string;
  className?: string;
}

/** A level older than this is stale: the voice stopped without its last 0 arriving yet. */
const STALE_MS = 250;
/** Ribbon: one bar per 50 ms of voice, the daemon's own pace. */
const RIBBON_STEP_MS = 50;

export function VoiceWave({ active, variant: forced, width, height, simulate = false, still, label, className }: VoiceWaveProps) {
  const own = useWaveVariant();
  const variant = forced ?? own;
  const { voice } = useGhost();
  const canvas = useRef<HTMLCanvasElement>(null);
  const reduced = useReducedMotion();
  const frozen = still ?? reduced;
  // Inputs the loop reads without restarting.
  const live = useRef({ active, simulate, voice, variant, frozen });
  live.current = { active, simulate, voice, variant, frozen };
  // Starts the frame loop if it sleeps; set by the drawing effect.
  const wake = useRef<() => void>(() => {});

  // A level while the loop sleeps wakes it (Ghost's voice can arrive before the phase does).
  // No state: twenty a second must not re-render anything.
  useEffect(() => voice.subscribe((l) => l > 0 && wake.current()), [voice]);
  useEffect(() => {
    if (active) wake.current();
  }, [active]);

  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = el.getContext("2d");
    if (!ctx) return;
    let raf = 0;
    let last = performance.now();
    let start = last;
    let env = 0;
    let phase = 0;
    let history: number[] = [];
    let step = 0;
    let color = "";
    let colorAt = -Infinity;
    let w = 0;
    let h = 0;

    const size = () => {
      const dpr = window.devicePixelRatio || 1;
      w = width ?? el.clientWidth;
      h = height;
      el.width = Math.max(1, Math.round(w * dpr));
      el.height = Math.max(1, Math.round(h * dpr));
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    size();
    const ro = width == null ? new ResizeObserver(() => (size(), draw(performance.now(), true))) : null;
    ro?.observe(el);

    const target = (now: number): number => {
      const s = live.current;
      if (!s.active) return 0;
      if (s.simulate) return speechEnvelope((now - start) / 1000);
      const { level, at } = s.voice.get();
      return now - at < STALE_MS ? level : 0;
    };

    function draw(now: number, once = false) {
      const s = live.current;
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      if (now - colorAt > 1000) {
        color = getComputedStyle(el!).color;
        colorAt = now;
      }
      if (s.frozen) {
        env = s.active ? 0.6 : 0;
      } else {
        // Quick to rise, slower to fall, at any frame rate.
        const t = target(now);
        const k = 1 - Math.pow(1 - (t > env ? 0.55 : 0.14), dt * 60);
        env += (t - env) * k;
        phase += dt * (2.2 + env * 5);
      }
      ctx!.clearRect(0, 0, w, h);
      ctx!.fillStyle = color;
      ctx!.strokeStyle = color;
      if (s.variant === "ribbon") {
        const bar = 3;
        const gap = 3;
        const n = Math.max(1, Math.floor((w + gap) / (bar + gap)));
        if (s.frozen) history = Array.from({ length: n }, (_, i) => (s.active ? 0.25 + 0.6 * speechEnvelope(i * 0.05 + 3) : 0));
        else {
          step += dt * 1000;
          while (step >= RIBBON_STEP_MS) {
            step -= RIBBON_STEP_MS;
            history.push(env);
          }
        }
        if (history.length > n) history = history.slice(-n);
        const x0 = w - n * (bar + gap) + gap;
        for (let i = 0; i < n; i++) {
          const v = history[history.length - n + i] ?? 0;
          const bh = Math.max(bar, v * h * 0.9);
          // Older bars fade toward the left: the eye reads the right edge as now.
          ctx!.globalAlpha = 0.25 + 0.75 * ((i + 1) / n);
          roundRect(ctx!, x0 + i * (bar + gap), (h - bh) / 2, bar, bh, bar / 2);
        }
        ctx!.globalAlpha = 1;
      } else if (s.variant === "strings") {
        const lines = [
          { f: 1.6, p: 0, a: 1, lw: 2, alpha: 1 },
          { f: 2.3, p: 1.7, a: 0.7, lw: 1.5, alpha: 0.55 },
          { f: 1.1, p: 3.1, a: 0.5, lw: 1.25, alpha: 0.3 },
        ];
        const amp = Math.max(0.02, env) * (h / 2 - 2);
        for (const l of lines) {
          ctx!.globalAlpha = l.alpha;
          ctx!.lineWidth = l.lw;
          ctx!.beginPath();
          for (let x = 0; x <= w; x += 2) {
            const u = x / w;
            // Pinned at both ends, fullest in the middle.
            const taper = Math.pow(Math.sin(Math.PI * u), 1.6);
            const y = h / 2 + Math.sin(u * Math.PI * 2 * l.f + phase * (1 + l.p * 0.15) + l.p) * amp * l.a * taper;
            if (x === 0) ctx!.moveTo(x, y);
            else ctx!.lineTo(x, y);
          }
          ctx!.stroke();
        }
        ctx!.globalAlpha = 1;
      } else {
        const profile = [0.45, 0.75, 1, 0.75, 0.45];
        const bar = Math.max(3, Math.round(h * 0.14));
        const gap = Math.round(bar * 0.8);
        const total = profile.length * bar + (profile.length - 1) * gap;
        const x0 = (w - total) / 2;
        profile.forEach((p, i) => {
          // Each bar wanders a little on its own, so they beat together without moving as one block.
          const wobble = s.frozen ? 1 : 0.78 + 0.22 * Math.sin(phase * (1.3 + i * 0.37) + i * 2.1);
          const bh = Math.max(bar, env * p * wobble * h);
          roundRect(ctx!, x0 + i * (bar + gap), (h - bh) / 2, bar, bh, bar / 2);
        });
      }
      if (once || s.frozen) return;
      const settled = !s.active && env < 0.002 && (s.variant !== "ribbon" || history.every((v) => v < 0.002));
      if (settled) {
        env = 0;
        history = [];
        raf = 0;
        return;
      }
      raf = requestAnimationFrame((t) => draw(t));
    }

    wake.current = () => {
      if (raf) return;
      last = performance.now();
      if (env === 0) start = last;
      raf = requestAnimationFrame((t) => draw(t));
    };
    // The first frame, at rest or not; a frozen frame redraws when its inputs change.
    draw(performance.now(), true);
    if (!frozen) wake.current();
    return () => {
      cancelAnimationFrame(raf);
      raf = 0;
      wake.current = () => {};
      ro?.disconnect();
    };
  }, [width, height, variant, frozen, simulate, frozen ? active : null]);

  return (
    <canvas
      ref={canvas}
      className={cx("g-wave", `g-wave--${variant}`, active && "is-active", className)}
      style={{ height, ...(width != null ? { width } : {}) } as CSSProperties}
      role={label ? "img" : undefined}
      aria-label={label || undefined}
      aria-hidden={label ? undefined : true}
    />
  );
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, Math.min(r, w / 2, h / 2));
  ctx.fill();
}

function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches);
  useEffect(() => {
    const q = matchMedia("(prefers-reduced-motion: reduce)");
    const on = () => setReduced(q.matches);
    q.addEventListener("change", on);
    return () => q.removeEventListener("change", on);
  }, []);
  return reduced;
}

export type { VoiceLevel };

/**
 * The wave where Ghost's presence sits while it speaks: a capsule in the speaking color that
 * grows out of the presence's circle (the composer's leading edge, the start state), with the
 * operator's chosen design inside. `size` is the presence it stands in for.
 */
export function SpeakingWave({ size, className }: { size: 48 | 96; className?: string }) {
  const inner = size === 48 ? { width: 92, height: 26 } : { width: 184, height: 52 };
  return (
    <span className={cx("g-wave-pill", `g-wave-pill--${size}`, className)} style={{ "--wp-size": `${size}px` } as CSSProperties}>
      <VoiceWave active width={inner.width} height={inner.height} />
    </span>
  );
}
