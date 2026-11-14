/**
 * Ghost's voice state as one presence: the mark (docs/design-system.md, The mark) brought to the
 * phone the way the floating room brings it to the desk. Wisp lit from within in the state's
 * colour on a night disc whose light rises behind it, inside the floating room's ring of ticks
 * with one arc lit in that colour (the whole ring while Ghost speaks). The disc is dark on the
 * light theme too: the mark is a dark chip on every surface.
 *
 * The state is a colour (`MARK_STATES`): cyan at rest and speaking, green listening, violet
 * thinking (reaching), amber while a change waits for a yes, grey with the red stroke when the
 * desk isn't listening, grey and dim when the desk can't be reached or Ghost is on hold.
 *
 * 320 is the hero (Home's Start control, the session screen): Wisp, the live mascot, acts out the
 * phase (wide-eyed listening, thought dots, a talking mouth, eyes shut with a rising "z" on
 * hold, waking while starting, slumped offline). With `float` Wisp drifts and blinks now and then;
 * bumping `wave` plays its wave (Home bumps it when a tap starts a session).
 *
 * The cost of a frame: only the Mascot redraws (a mood at 30 frames a second, one gradient fill,
 * no filter). The disc, its light and the 72 ticks are one static drawing, drawn again only when
 * the state changes; what moves around them (the arc turning while thinking or starting, the
 * ring's pulse while speaking, the light's breath, the drift) is a transform or an opacity on the
 * native driver. Nothing loops at rest: passive Wisp only blinks. Reduced motion freezes every
 * loop; the colour and the arc still tell the state.
 *
 * The smaller sizes (48, 96, 160) are the chip: the disc, a hairline ring with the lit arc and
 * Wisp in the phase's still pose.
 */
import type { GhostPhase } from "@ghost/ui";
import { INK, NIGHT, OFF_RED, phaseText, stillPose, WISP } from "@ghost/ui";
import { memo, useEffect, useId, useMemo, useRef } from "react";
import { Animated, Easing, View } from "react-native";
import Svg, { Circle, Defs, G, Line, Path, RadialGradient, Stop } from "react-native-svg";

import { type Look, lookOf, type PresenceMark } from "./presence";
import { Mascot } from "./Mascot";
import { useReducedMotion } from "./motion";

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
  /** The mark's state when it isn't the phase's: `waiting` while a change waits for a yes, `off` when the desk isn't listening. */
  mark?: PresenceMark;
}

export function VoicePresence(props: VoicePresenceProps) {
  return props.size === 320 ? <Hero {...props} /> : <Orb {...props} />;
}

/** A small deterministic noise (mark.ts's), so the lit ticks look like a voice and not a ramp. */
const jitter = (i: number) => {
  const x = Math.sin(i * 12.9898 + 4.1) * 43758.5453;
  return x - Math.floor(x);
};

const TICKS = 72;
const RING_R = 38;

/**
 * The disc, its light and the ring of ticks (the floating room's instrument), on a 100-unit
 * square. Static: drawn again only when the look changes.
 */
const Disc = memo(function Disc({ size, look, ticks }: { size: number; look: Look; ticks: boolean }) {
  const id = `disc-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const hair = 100 / size;
  const { light, glow } = look;
  const lines = [];
  // The unlit ring. The lit ticks are `Arc`'s, a layer above, so they can turn and pulse.
  if (ticks) {
    for (let i = 0; i < TICKS; i++) {
      const a = ((i / TICKS) * 360 - 180) * (Math.PI / 180);
      const major = i % 10 === 0;
      const len = major ? 3.2 : 1.8;
      lines.push(
        <Line
          key={i}
          x1={50 + RING_R * Math.cos(a)}
          y1={50 + RING_R * Math.sin(a)}
          x2={50 + (RING_R + len) * Math.cos(a)}
          y2={50 + (RING_R + len) * Math.sin(a)}
          stroke={INK}
          strokeOpacity={major ? 0.5 : 0.28}
          strokeWidth={Math.max(hair, 0.5)}
          strokeLinecap="round"
        />,
      );
    }
  }
  return (
    <Svg width={size} height={size} viewBox="0 0 100 100" style={{ position: "absolute" }}>
      <Defs>
        <RadialGradient id={`${id}-g`} cx={50} cy={50} r={50} gradientUnits="userSpaceOnUse">
          <Stop offset={0} stopColor={light} stopOpacity={0.24 * glow} />
          <Stop offset={0.5} stopColor={light} stopOpacity={0.07 * glow} />
          <Stop offset={1} stopColor={light} stopOpacity={0} />
        </RadialGradient>
      </Defs>
      <Circle cx={50} cy={50} r={49.5} fill={NIGHT} />
      <Circle cx={50} cy={50} r={49.5} fill={`url(#${id}-g)`} />
      <Circle cx={50} cy={50} r={49.5 - hair / 2} fill="none" stroke={light} strokeOpacity={0.12 + 0.2 * glow} strokeWidth={hair} />
      {ticks ? (
        <>
          <Circle cx={50} cy={50} r={RING_R - 1.4} fill="none" stroke={light} strokeOpacity={0.18} strokeWidth={Math.max(hair, 0.25)} />
          {lines}
        </>
      ) : (
        <Circle cx={50} cy={50} r={41} fill="none" stroke={INK} strokeOpacity={0.24} strokeWidth={hair} />
      )}
    </Svg>
  );
});

/** The state's light alone, over the disc, so it can breathe on the native driver without dimming the night. */
const Glow = memo(function Glow({ size, light }: { size: number; light: string }) {
  const id = `glow-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  return (
    <Svg width={size} height={size} viewBox="0 0 100 100">
      <Defs>
        <RadialGradient id={id} cx={50} cy={50} r={50} gradientUnits="userSpaceOnUse">
          <Stop offset={0} stopColor={light} stopOpacity={0.2} />
          <Stop offset={0.5} stopColor={light} stopOpacity={0.06} />
          <Stop offset={1} stopColor={light} stopOpacity={0} />
        </RadialGradient>
      </Defs>
      <Circle cx={50} cy={50} r={49.5} fill={`url(#${id})`} />
    </Svg>
  );
});

/** The lit arc alone, so it can turn on the native driver: ticks at the hero's size, a stroke on the chip. */
const Arc = memo(function Arc({ size, look, ticks }: { size: number; look: Look; ticks: boolean }) {
  const { light, full } = look;
  const arc: [number, number] | null = full ? [-180, 179.99] : look.arc;
  if (!arc) return null;
  const hair = 100 / size;
  if (!ticks && full) {
    return (
      <Svg width={size} height={size} viewBox="0 0 100 100">
        <Circle cx={50} cy={50} r={41} fill="none" stroke={light} strokeWidth={Math.max(hair * 2, 2.4)} />
      </Svg>
    );
  }
  if (!ticks) {
    const R = 41;
    const pt = (deg: number) => `${(50 + R * Math.cos((deg * Math.PI) / 180)).toFixed(2)} ${(50 + R * Math.sin((deg * Math.PI) / 180)).toFixed(2)}`;
    return (
      <Svg width={size} height={size} viewBox="0 0 100 100">
        <Path d={`M${pt(arc[0])}A${R} ${R} 0 0 1 ${pt(arc[1])}`} fill="none" stroke={light} strokeWidth={Math.max(hair * 2, 2.4)} strokeLinecap="round" />
      </Svg>
    );
  }
  const span = (arc[1] - arc[0] + 360) % 360;
  const lines = [];
  for (let i = 0; i < TICKS; i++) {
    const deg = (i / TICKS) * 360 - 180;
    const rel = (((deg - arc[0]) % 360) + 360) % 360;
    if (rel > span) continue;
    const a = (deg * Math.PI) / 180;
    // Speaking lights the whole ring like a voice; an arc swells to its middle.
    const amp = full ? 0.8 * (0.4 + 0.6 * jitter(i)) : Math.sin((rel / span) * Math.PI) * (0.5 + 0.5 * jitter(i));
    const len = (i % 10 === 0 ? 3.2 : 1.8) + amp * 5.5;
    lines.push(
      <Line
        key={i}
        x1={50 + RING_R * Math.cos(a)}
        y1={50 + RING_R * Math.sin(a)}
        x2={50 + (RING_R + len) * Math.cos(a)}
        y2={50 + (RING_R + len) * Math.sin(a)}
        stroke={light}
        strokeOpacity={0.95}
        strokeWidth={Math.max(hair, 0.5) * 1.25}
        strokeLinecap="round"
      />,
    );
  }
  return (
    <Svg width={size} height={size} viewBox="0 0 100 100">
      <G>{lines}</G>
    </Svg>
  );
});

/** Not listening's stroke, over the disc. */
function Strike({ size }: { size: number }) {
  const hair = 100 / size;
  return (
    <Svg width={size} height={size} viewBox="0 0 100 100" style={{ position: "absolute" }}>
      <Line x1={24} y1={76} x2={76} y2={24} stroke={NIGHT} strokeWidth={Math.max(hair * 3.5, 5)} strokeLinecap="round" />
      <Line x1={24} y1={76} x2={76} y2={24} stroke={OFF_RED} strokeWidth={Math.max(hair * 1.6, 2.2)} strokeLinecap="round" />
    </Svg>
  );
}

/** The moving parts around the static disc: the arc's turn, the ring's pulse, the light's breath. */
function useMotion(phase: GhostPhase, still: boolean, float: boolean) {
  const turn = useLoop(!still && (phase === "thinking" || phase === "starting"), phase === "starting" ? 1600 : 5200, { linear: true });
  const pulse = useLoop(!still && phase === "speaking", 900, { alternate: true });
  const breathe = useLoop(!still && phase === "thinking", 1200, { alternate: true });
  const ripple = useLoop(!still && phase === "listening", 2400);
  const drift = useLoop(!still && float, 2200, { alternate: true });
  return {
    arcStyle: {
      transform: [{ rotate: turn.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] }) }],
      opacity: phase === "speaking" ? pulse.interpolate({ inputRange: [0, 1], outputRange: [0.55, 1] }) : 1,
    },
    lightOpacity: phase === "thinking" ? (still ? 1 : breathe.interpolate({ inputRange: [0, 1], outputRange: [0, 1] })) : phase === "speaking" ? (still ? 1 : pulse.interpolate({ inputRange: [0, 1], outputRange: [0.4, 1] })) : 1,
    ripple,
    drift,
  };
}

/** The hero: the night disc and its ring, Wisp lit inside it, acting out the phase. */
function Hero({ phase, label, float = false, wave = 0, mark = null }: VoicePresenceProps) {
  const size = 320;
  const still = useReducedMotion();
  // Memoized so a parent's re-render (the session's clock, once a second) never redraws the disc and its ticks.
  const look = useMemo(() => lookOf(phase, mark, float), [phase, mark, float]);
  const name = label ?? phaseText(phase);
  const m = useMotion(phase, still, float);
  const breathes = phase === "thinking" || phase === "speaking";
  const quiet = useMemo(() => ({ ...look, glow: look.glow * 0.5 }), [look]);
  return (
    <View
      accessible={!!name}
      accessibilityRole={name ? "image" : undefined}
      accessibilityLabel={name || undefined}
      importantForAccessibility={name ? "yes" : "no-hide-descendants"}
      style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}
    >
      <Disc size={size} look={breathes ? quiet : look} ticks />
      {breathes ? (
        <Animated.View style={{ position: "absolute", width: size, height: size, opacity: m.lightOpacity }}>
          <Glow size={size} light={look.light} />
        </Animated.View>
      ) : null}
      <Animated.View style={[{ position: "absolute", width: size, height: size }, m.arcStyle]}>
        <Arc size={size} look={look} ticks />
      </Animated.View>
      <Animated.View style={{ opacity: look.strike ? 0.7 : 1, transform: float && !still ? [{ translateY: m.drift.interpolate({ inputRange: [0, 1], outputRange: [size * 0.02, -size * 0.02] }) }] : [] }}>
        <Mascot size={Math.round(size * 0.6)} color={look.light} lit={look.light} mood={float ? "passive" : phase} trigger={wave} blinks={float || phase === "passive"} />
      </Animated.View>
      {look.strike ? <Strike size={size} /> : null}
    </View>
  );
}

/** The smaller sizes: the chip, Wisp still in the phase's pose. */
function Orb({ phase, size = 96, label, float = false, mark = null }: VoicePresenceProps) {
  const still = useReducedMotion();
  // Memoized so a parent's re-render (the session's clock, once a second) never redraws the disc and its ticks.
  const look = useMemo(() => lookOf(phase, mark, float), [phase, mark, float]);
  const name = label ?? phaseText(phase);
  const m = useMotion(phase, still, false);
  const breathes = phase === "thinking" || phase === "speaking";
  const quiet = useMemo(() => ({ ...look, glow: look.glow * 0.5 }), [look]);
  const frame = stillPose(WISP, false, phase === "passive" ? null : phase);
  return (
    <View
      accessible={!!name}
      accessibilityRole={name ? "image" : undefined}
      accessibilityLabel={name || undefined}
      importantForAccessibility={name ? "yes" : "no-hide-descendants"}
      style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}
    >
      {phase === "listening" && !still ? (
        <Animated.View
          style={{
            position: "absolute",
            width: size,
            height: size,
            borderRadius: size,
            borderWidth: 1,
            borderColor: look.light,
            opacity: m.ripple.interpolate({ inputRange: [0, 1], outputRange: [0.5, 0] }),
            transform: [{ scale: m.ripple.interpolate({ inputRange: [0, 1], outputRange: [1, 1.18] }) }],
          }}
        />
      ) : null}
      <Disc size={size} look={breathes ? quiet : look} ticks={false} />
      {breathes ? (
        <Animated.View style={{ position: "absolute", width: size, height: size, opacity: m.lightOpacity }}>
          <Glow size={size} light={look.light} />
        </Animated.View>
      ) : null}
      <Animated.View style={[{ position: "absolute", width: size, height: size }, m.arcStyle]}>
        <Arc size={size} look={look} ticks={false} />
      </Animated.View>
      <View style={{ opacity: look.strike ? 0.7 : 1 }}>
        <Mascot size={Math.round(size * 0.62)} color={look.light} lit={look.light} frame={frame} />
      </View>
      {look.strike ? <Strike size={size} /> : null}
    </View>
  );
}
