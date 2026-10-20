/**
 * Ghost's voice state as one presence, as on desktop (VoicePresence.tsx and .g-voice in
 * components.css). Still for passive and offline; one quiet ripple for listening, a slow
 * breath for thinking, bars for speaking, a turning arc while starting. With `float`, the
 * core drifts up and down slowly and the mark is lit in the accent, not passive grey: the
 * phone's Home uses it for the ghost that is the Start control, so it reads as something to
 * tap (and, under reduced motion, still does, by color). Reduced motion freezes every loop at
 * rest; color and glyph still tell the state.
 *
 * 320 is the hero (Home's Start control, the session screen): no bubble, no rings, no bars. It is
 * Wisp, the live mascot, in the phase's color over a soft glow, and Wisp itself shows the phase:
 * wide-eyed while listening, thought dots while thinking, a talking mouth while speaking, eyes
 * shut with a rising "z" when checked out (hold), waking while starting, slumped when offline.
 * With `float` Wisp drifts up and down and blinks now and then; bumping `wave` plays its wave
 * (Home bumps it when a tap starts a session, so the wait for the desk has something to watch).
 */
import type { GhostPhase } from "@ghost/ui";
import { phaseText } from "@ghost/ui";
import { useEffect, useId, useRef } from "react";
import { Animated, Easing, View } from "react-native";
import Svg, { Defs, RadialGradient, Rect, Stop } from "react-native-svg";

import { phaseColor, useColors } from "../theme";
import { GhostGlyph } from "./Brand";
import { Mascot } from "./Mascot";
import { useReducedMotion } from "./motion";

/** A hex color at an alpha, for the CSS's color-mix(var(--c) N%, transparent). */
function alpha(hex: string, a: number): string {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

/** A looping 0 to 1 value while `on`; held at `rest` otherwise. */
function useLoop(on: boolean, ms: number, opts: { alternate?: boolean; linear?: boolean; rest?: number } = {}) {
  const v = useRef(new Animated.Value(opts.rest ?? 0)).current;
  useEffect(() => {
    if (!on) {
      v.setValue(opts.rest ?? 0);
      return;
    }
    const easing = opts.linear ? Easing.linear : Easing.inOut(Easing.cubic);
    const anim = opts.alternate
      ? Animated.loop(Animated.sequence([Animated.timing(v, { toValue: 1, duration: ms, easing, useNativeDriver: true }), Animated.timing(v, { toValue: 0, duration: ms, easing, useNativeDriver: true })]))
      : Animated.loop(Animated.timing(v, { toValue: 1, duration: ms, easing, useNativeDriver: true }), { iterations: -1, resetBeforeIteration: true });
    v.setValue(0);
    anim.start();
    return () => anim.stop();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [on, ms]);
  return v;
}

export interface VoicePresenceProps {
  phase: GhostPhase;
  size?: 48 | 96 | 160 | 320;
  label?: string;
  float?: boolean;
  /** Bump to play Wisp's wave (the 320 hero only). */
  wave?: number;
}

export function VoicePresence(props: VoicePresenceProps) {
  return props.size === 320 ? <Hero {...props} /> : <Orb {...props} />;
}

/** The hero: Wisp alone, floating over a glow, acting out the phase. */
function Hero({ phase, label, float = false, wave = 0 }: VoicePresenceProps) {
  const size = 320;
  const c = useColors();
  const still = useReducedMotion();
  const color = float ? c.accent : phaseColor(c, phase);
  const name = label ?? phaseText(phase);
  const breathe = useLoop(!still && phase === "thinking", 1200, { alternate: true });
  const drift = useLoop(!still && float, 2200, { alternate: true });
  const passive = (phase === "passive" || phase === "hold") && !float;
  const offline = phase === "offline";
  // Its own gradient id: Home and the session screen are mounted together.
  const haloId = `halo-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const ink = passive ? c.text2 : offline ? c.text3 : color;
  // Lit while listening and speaking (and as the Start control), breathing while thinking.
  const haloOpacity = phase === "listening" || phase === "speaking" || float ? 1 : phase === "thinking" ? (still ? 1 : breathe.interpolate({ inputRange: [0, 1], outputRange: [0.35, 1] })) : phase === "starting" ? 0.5 : 0;
  return (
    <View
      accessible={!!name}
      accessibilityRole={name ? "image" : undefined}
      accessibilityLabel={name || undefined}
      importantForAccessibility={name ? "yes" : "no-hide-descendants"}
      style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}
    >
      <Animated.View style={{ position: "absolute", width: size, height: size, opacity: haloOpacity }}>
        <Svg width={size} height={size}>
          <Defs>
            <RadialGradient id={haloId} cx="50%" cy="50%" r="50%">
              <Stop offset="0" stopColor={color} stopOpacity={0.2} />
              <Stop offset="1" stopColor={color} stopOpacity={0} />
            </RadialGradient>
          </Defs>
          <Rect width={size} height={size} fill={`url(#${haloId})`} />
        </Svg>
      </Animated.View>
      <Animated.View style={{ transform: float && !still ? [{ translateY: drift.interpolate({ inputRange: [0, 1], outputRange: [size * 0.03, -size * 0.03] }) }] : [] }}>
        <Mascot size={Math.round(size * 0.8)} color={ink} mood={float ? "passive" : phase} trigger={wave} blinks={float || phase === "passive"} />
      </Animated.View>
    </View>
  );
}

/** The smaller sizes: the glyph in a bubble, with rings and bars for the phase. */
function Orb({ phase, size = 96, label, float = false }: VoicePresenceProps) {
  const c = useColors();
  const still = useReducedMotion();
  const color = float ? c.accent : phaseColor(c, phase);
  const name = label ?? phaseText(phase);

  const ripple = useLoop(!still && phase === "listening", 2400);
  const breathe = useLoop(!still && phase === "thinking", 1200, { alternate: true });
  const spin = useLoop(!still && phase === "starting", 1600, { linear: true });
  const drift = useLoop(!still && float, 2200, { alternate: true });

  const ring = size === 48 ? 0.88 : 0.76;
  const core = size === 48 ? 0.64 : 0.56;
  // Checked out (hold) Ghost is quiet by choice: the still, grey mark, the same as passive.
  const passive = (phase === "passive" || phase === "hold") && !float;
  const offline = phase === "offline";
  const quiet = passive || offline;

  const ringStyle = {
    position: "absolute" as const,
    width: size * ring,
    height: size * ring,
    borderRadius: size,
    borderWidth: 1,
    borderStyle: offline ? ("dashed" as const) : ("solid" as const),
    borderColor: passive ? c.border : offline ? c.borderStrong : phase === "starting" ? c.border : alpha(color, 0.34),
  };
  // The halo is lit while listening and speaking, and breathes while thinking.
  const haloOpacity = phase === "listening" || phase === "speaking" || float ? 1 : phase === "thinking" ? (still ? 1 : breathe.interpolate({ inputRange: [0, 1], outputRange: [0.35, 1] })) : 0;

  return (
    <View
      accessible={!!name}
      accessibilityRole={name ? "image" : undefined}
      accessibilityLabel={name || undefined}
      importantForAccessibility={name ? "yes" : "no-hide-descendants"}
      style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}
    >
      <Animated.View style={{ position: "absolute", width: size, height: size, opacity: haloOpacity }}>
        <Svg width={size} height={size}>
          <Defs>
            <RadialGradient id="halo" cx="50%" cy="50%" r="50%">
              <Stop offset="0" stopColor={color} stopOpacity={0.16} />
              <Stop offset="1" stopColor={color} stopOpacity={0} />
            </RadialGradient>
          </Defs>
          <Rect width={size} height={size} fill="url(#halo)" />
        </Svg>
      </Animated.View>

      <Animated.View
        style={[
          ringStyle,
          phase === "starting" && { borderTopColor: color, transform: [{ rotate: spin.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] }) }] },
        ]}
      />
      {phase === "listening" && !still && (
        <Animated.View
          style={[
            ringStyle,
            {
              opacity: ripple.interpolate({ inputRange: [0, 1], outputRange: [0.7, 0] }),
              transform: [{ scale: ripple.interpolate({ inputRange: [0, 1], outputRange: [1, 1.3] }) }],
            },
          ]}
        />
      )}

      <Animated.View
        style={{
          width: size * core,
          height: size * core,
          borderRadius: size,
          alignItems: "center",
          justifyContent: "center",
          borderWidth: 1,
          backgroundColor: quiet ? c.bgSurface : c.bgRaised,
          borderColor: passive ? c.borderStrong : offline ? c.border : alpha(color, 0.4),
          transform:
            phase === "thinking" && !still
              ? [{ scale: breathe.interpolate({ inputRange: [0, 1], outputRange: [0.96, 1.04] }) }]
              : float && !still
                ? [{ translateY: drift.interpolate({ inputRange: [0, 1], outputRange: [size * 0.03, -size * 0.03] }) }]
                : [],
          overflow: "hidden",
        }}
      >
        {!quiet && <View style={{ position: "absolute", top: 0, left: 0, right: 0, bottom: 0, backgroundColor: alpha(color, 0.14) }} />}
        {phase === "speaking" ? (
          <Bars size={size} color={color} still={still} />
        ) : (
          <GhostGlyph size={Math.round(size * 0.3)} color={passive ? c.text2 : offline ? c.text3 : color} />
        )}
      </Animated.View>
    </View>
  );
}

const BARS = [0.55, 0.95, 0.7, 0.4];

function Bars({ size, color, still }: { size: number; color: string; still: boolean }) {
  const loops = [useLoop(!still, 900, { alternate: true }), useLoop(!still, 900, { alternate: true }), useLoop(!still, 900, { alternate: true }), useLoop(!still, 900, { alternate: true })];
  const height = size * 0.44;
  return (
    <View style={{ flexDirection: "row", alignItems: "center", gap: size * 0.035, height }}>
      {BARS.map((w, i) => (
        <Animated.View
          key={i}
          style={{
            width: Math.max(2, size * 0.045),
            height: height * w,
            borderRadius: 999,
            backgroundColor: color,
            // Staggered like the CSS's animation-delay: a fixed offset into each bar's swing.
            transform: still ? [] : [{ scaleY: loops[i].interpolate({ inputRange: [0, 1], outputRange: i % 2 ? [1, 0.45] : [0.45, 1] }) }],
          }}
        />
      ))}
    </View>
  );
}
