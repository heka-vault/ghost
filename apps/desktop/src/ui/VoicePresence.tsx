import type { CSSProperties } from "react";

import { GhostGlyph } from "./Brand";
import { cx } from "./cx";
import type { GhostPhase } from "./Display";
import { phaseText } from "./format";

export interface VoicePresenceProps {
  phase: GhostPhase;
  /** 48 (tray, compact headers), 96 (session, phone), 160 (a hero moment). */
  size?: 48 | 96 | 160;
  /**
   * Live input or output level, 0 to 1. Listening: the mic ring swells with it. Speaking:
   * the bars follow it. Omit and the loop runs on its own gentle animation.
   */
  level?: number;
  /** Accessible name; defaults to the phase in words. Pass "" to make it decorative. */
  label?: string;
  className?: string;
}

/**
 * Ghost's voice state as one presence, shared by session, tray and phone. It is still when
 * nothing is happening (passive, offline) and moves only to mean something: one quiet ring
 * for listening, a slow breath for thinking, bars for speaking, a turning arc while
 * starting. Reduced motion freezes every loop at rest; color and glyph still tell the state.
 */
export function VoicePresence({ phase, size = 96, level, label, className }: VoicePresenceProps) {
  const name = label ?? phaseText(phase);
  const lv = level == null ? null : Math.max(0, Math.min(1, level));
  const style = { "--vp-size": `${size}px`, ...(lv != null ? { "--level": lv } : {}) } as CSSProperties;
  return (
    <span
      className={cx("g-voice", `g-voice--${size}`, `is-${phase}`, lv != null && "has-level", className)}
      style={style}
      role={name ? "img" : undefined}
      aria-label={name || undefined}
      aria-hidden={name ? undefined : true}
    >
      <span className="g-voice__halo" />
      <span className="g-voice__ring" />
      <span className="g-voice__ripple" />
      <span className="g-voice__core">
        {phase === "speaking" ? (
          <span className="g-voice__bars">
            {[0.55, 0.95, 0.7, 0.4].map((w, i) => (
              <span key={i} className="g-voice__bar" style={{ "--w": w, "--i": i } as CSSProperties} />
            ))}
          </span>
        ) : (
          /* 30% of the presence: 14 at 48 (the simplified small drawing), 29 at 96, 48 at 160. */
          <GhostGlyph size={Math.round(size * 0.3)} className="g-voice__glyph" />
        )}
      </span>
    </span>
  );
}
