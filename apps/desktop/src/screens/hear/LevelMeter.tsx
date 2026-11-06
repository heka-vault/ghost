/**
 * Owner: hear. The mic's level while the recorder is open, and whether the VAD hears speech.
 * It reads `useGhost().hear` (a store outside React state) on its own animation frame, so
 * twenty levels a second never re-render the screen. Reduced motion draws one still frame;
 * the "Hearing you" dot and word still follow the VAD, since they are a state, not motion.
 */
import { type HearLive, useGhost } from "@ghost/ui";
import { useEffect, useRef } from "react";

import { prefersReducedMotion } from "./recorder";

export function LevelMeter({ active }: { active: boolean }) {
  const { hear } = useGhost();
  const fill = useRef<HTMLSpanElement>(null);
  const peak = useRef<HTMLSpanElement>(null);
  const vad = useRef<HTMLSpanElement>(null);
  const word = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    let shown = 0;
    let held = 0;
    let heldAt = 0;
    const speaking = (on: boolean) => {
      vad.current?.classList.toggle("is-on", on);
      if (word.current) word.current.textContent = on ? "Hearing you" : "Quiet";
    };
    const paint = (now: number, smooth: boolean) => {
      const l = active ? hear.level() : { level: 0, speaking: false };
      // Up at once, down gently: a meter that flickers reads as noise.
      shown = smooth ? Math.max(l.level, shown * 0.86) : l.level;
      if (shown >= held || now - heldAt > 1200) {
        held = shown;
        heldAt = now;
      }
      if (fill.current) fill.current.style.transform = `scaleX(${shown.toFixed(3)})`;
      if (peak.current) peak.current.style.left = `${(held * 100).toFixed(1)}%`;
      speaking(l.speaking);
    };
    if (prefersReducedMotion()) {
      paint(performance.now(), false);
      let was = hear.level().speaking;
      return hear.subscribeLevel((l) => {
        if (l.speaking !== was) speaking((was = l.speaking) && active);
      });
    }
    let raf = 0;
    const loop = (t: number) => {
      paint(t, true);
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [hear, active]);

  return (
    <div className="hear-meter" aria-hidden>
      <span className="hear-meter__vad">
        <span ref={vad} className="hear-meter__dot" />
        <span ref={word} className="hear-meter__word">
          Quiet
        </span>
      </span>
      <span className="hear-meter__track">
        <span ref={fill} className="hear-meter__fill" />
        <span ref={peak} className="hear-meter__peak" />
      </span>
    </div>
  );
}

/** Seconds into the take, written straight into the DOM from the store (no re-render). */
export function Elapsed({ live }: { live: HearLive }) {
  const el = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    let last = -1;
    const put = (s: number) => {
      const tenth = Math.floor(s * 10);
      if (tenth === last || !el.current) return;
      last = tenth;
      el.current.textContent = `${(tenth / 10).toFixed(1)} s`;
    };
    put(live.level().elapsed);
    return live.subscribeLevel((l) => put(l.elapsed));
  }, [live]);
  return <span ref={el} className="t-num" />;
}
