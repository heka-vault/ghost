/**
 * Owner: session. Ghost's presence in a live session: Wisp, in the agent's color, on a halo,
 * doing what the session is doing. One sign that reads at a glance from across a room and
 * stays calm beside other work in a small window.
 *
 *   speaking   Wisp talks with Ghost's own voice (its mouth and lift follow the daemon's level,
 *              never the microphone) and the halo swells with every syllable
 *   you        Wisp leans in, wide-eyed, and rings flow in toward it at a steady pace: a sign
 *              that it is taking the operator in, the same at any volume, and never words (#73)
 *   working    the thinking pose, an arc turning round the halo
 *   listening  Wisp at rest, breathing; on hold it dozes; hanging up, it settles
 *
 * The level is written to a CSS variable (`--lvl`) from one animation-frame loop while Ghost
 * speaks, so nothing re-renders twenty times a second. Reduced motion: Wisp holds the mood's one
 * pose, the halo stands at a middle size, the rings and the arc stand still.
 */
import { lineHue, MARK_STATES, type MarkState, NIGHT, useGhost } from "@ghost/ui";
import { type CSSProperties, useEffect, useRef } from "react";

import { Mascot, cx } from "../../ui";
import type { StageState } from "./Stage";

/** A level older than this is stale: the voice stopped before its last 0 arrived. */
const STALE_MS = 250;

/** The mark's light per state, for Ghost's own lit Wisp (an agent's stays flat in its colour). */
const LIGHT: Record<StageState, MarkState> = {
  speaking: "speaking",
  you: "listening",
  working: "reaching",
  hold: "idle",
  listening: "listening",
  ending: "idle",
  phone: "listening",
  offline: "off",
};

const MOOD: Record<StageState, string | null> = {
  speaking: "speaking",
  you: "listening",
  working: "thinking",
  hold: "hold",
  listening: null,
  ending: null,
  phone: null,
  offline: null,
};

export function Presence({ state, slug, size, className }: { state: StageState; slug: string; size: number; className?: string }) {
  const { voice } = useGhost();
  const ref = useRef<HTMLSpanElement>(null);
  const hue = lineHue(slug);
  const ghost = slug === "ghost";
  // Ghost is lit in the mark's colour for the state on the night; an agent keeps its own hue.
  const tint = state === "offline" ? "#9aa3b2" : MARK_STATES[LIGHT[state]].tint;

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (state !== "speaking") {
      el.style.setProperty("--lvl", "0");
      return;
    }
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
      el.style.setProperty("--lvl", "0.4");
      return;
    }
    let raf = 0;
    let v = 0;
    let shown = "";
    const step = (now: number) => {
      const { level, at } = voice.get();
      const target = now - at > STALE_MS ? 0 : level;
      v += (target - v) * (target > v ? 0.5 : 0.12);
      const s = v.toFixed(3);
      if (s !== shown) {
        shown = s;
        el.style.setProperty("--lvl", s);
      }
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [state, voice]);

  return (
    <span
      ref={ref}
      className={cx("presence", `presence--${state}`, ghost && "presence--ghost", className)}
      style={
        (ghost
          ? { "--presence": `${size}px`, "--presence-fg": tint, "--mark-night": NIGHT }
          : { "--presence": `${size}px`, "--presence-fg": `var(--hue-${hue}-fg)`, "--presence-bg": `var(--hue-${hue}-bg)` }) as Record<string, string> as CSSProperties
      }
      aria-hidden
    >
      <span className="presence__halo" />
      <span className="presence__halo presence__halo--outer" />
      {state === "you" && (
        <>
          <span className="presence__ring" />
          <span className="presence__ring presence__ring--2" />
          <span className="presence__ring presence__ring--3" />
        </>
      )}
      {state === "working" && <span className="presence__arc" />}
      <span className="presence__wisp">
        <Mascot size={Math.round(size * 0.6)} mood={MOOD[state]} idle={state === "listening" || state === "phone"} level={state === "speaking" ? voice : undefined} lit={ghost ? tint : undefined} />
      </span>
    </span>
  );
}
