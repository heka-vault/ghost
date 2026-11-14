/**
 * The ghost glyph, the app mark and an agent's glyph, drawn as on desktop
 * (apps/desktop/src/ui/Brand.tsx; docs/design-system.md, The mark).
 *
 * - The glyph is Wisp at rest (`WISP.rest`, figure.ts), flat in one colour, the eyes cut out: the
 *   mitts from 32px, the plain hem with larger eyes under 24. No smile, no stick arms, no wave:
 *   the still mark. An agent with no look of its own is this, in its colour.
 * - The mark (`GhostChip`, `GhostMark`) is Wisp lit from within on the night tile, the state's
 *   light rising behind it: a dark chip on every surface, never a bare white ghost on a light
 *   page and never on the indigo slab (indigo is the accent for actions).
 */
import { ARCS, lookOf, MARK_STATES, type MarkState, NIGHT, OFF_RED, SHAPES, WISP } from "@ghost/ui";
import { useId } from "react";
import { View } from "react-native";
import Svg, { Circle, Defs, Ellipse, Line, LinearGradient, Path, RadialGradient, Rect, Stop } from "react-native-svg";

import { useDesk } from "../desk/context";
import { useColors } from "../theme";
import { Mascot } from "./Mascot";
import { OFF_LIGHT } from "./presence";


/** Wisp at rest, flat in `color`: the brand glyph at any size. */
export function GhostGlyph({ size = 16, color }: { size?: number; color: string }) {
  return <Mascot size={size} color={color} frame={WISP.rest} />;
}

/** A point on the 100-unit tile's ring at `deg` (0 is right, -90 up). */
const onRing = (R: number, deg: number) => {
  const a = (deg * Math.PI) / 180;
  return `${(50 + R * Math.cos(a)).toFixed(2)} ${(50 + R * Math.sin(a)).toFixed(2)}`;
};

export interface GhostChipProps {
  /** The tile's size in px. */
  size?: number;
  state?: MarkState;
  /** The hairline ring with the state's lit arc, or none (the app mark). */
  ring?: "arc" | "none";
}

/**
 * The mark in the app: the night tile, the state's light rising behind Wisp and catching the
 * rim, Wisp lit from within; with `ring="arc"` one arc of a hairline ring in the state's colour
 * (the whole ring while speaking; grey and struck through when not listening). Still: the
 * same drawing as the app icon (`markSvg`).
 */
export function GhostChip({ size = 40, state = "idle", ring = "none" }: GhostChipProps) {
  const off = state === "off";
  const tint = MARK_STATES[state].tint;
  const light = off ? OFF_LIGHT : tint;
  const id = `chip-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const hair = 100 / size;
  const R = 39;
  const [a0, a1] = ARCS[state];
  const wisp = Math.round(size * (ring === "arc" ? 0.66 : 0.84));
  return (
    <View accessible={false} style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}>
      <Svg width={size} height={size} viewBox="0 0 100 100" style={{ position: "absolute" }}>
        <Defs>
          <RadialGradient id={`${id}-g`} cx={50} cy={50} r={62} gradientUnits="userSpaceOnUse">
            <Stop offset={0} stopColor={tint} stopOpacity={off ? 0.06 : 0.2} />
            <Stop offset={0.45} stopColor={tint} stopOpacity={off ? 0.017 : 0.056} />
            <Stop offset={1} stopColor={tint} stopOpacity={0} />
          </RadialGradient>
          <LinearGradient id={`${id}-r`} x1={0} y1={0} x2={0} y2={100} gradientUnits="userSpaceOnUse">
            <Stop offset={0} stopColor="#ffffff" stopOpacity={0.16} />
            <Stop offset={0.55} stopColor={tint} stopOpacity={0.18} />
            <Stop offset={1} stopColor={tint} stopOpacity={0.6} />
          </LinearGradient>
        </Defs>
        <Rect width={100} height={100} rx={22.5} fill={NIGHT} />
        <Rect width={100} height={100} rx={22.5} fill={`url(#${id}-g)`} />
        <Rect x={0.6} y={0.6} width={98.8} height={98.8} rx={21.9} fill="none" stroke={`url(#${id}-r)`} strokeWidth={Math.max(hair, 1.2)} />
        {ring === "arc" ? (
          <>
            <Circle cx={50} cy={50} r={R} fill="none" stroke="#eafcff" strokeOpacity={0.3} strokeWidth={hair} />
            {state === "speaking" ? (
              <Circle cx={50} cy={50} r={R} fill="none" stroke={tint} strokeWidth={hair * 2} />
            ) : !off && a0 !== a1 ? (
              <Path d={`M${onRing(R, a0)}A${R} ${R} 0 0 1 ${onRing(R, a1)}`} fill="none" stroke={tint} strokeWidth={hair * 2} strokeLinecap="round" />
            ) : null}
          </>
        ) : null}
      </Svg>
      <View style={{ opacity: off ? 0.75 : 1 }}>
        <Mascot size={wisp} color={light} lit={light} frame={WISP.rest} />
      </View>
      {off ? (
        <Svg width={size} height={size} viewBox="0 0 100 100" style={{ position: "absolute" }}>
          <Line x1={20} y1={80} x2={80} y2={20} stroke={NIGHT} strokeWidth={Math.max(hair * 3.5, 7)} strokeLinecap="round" />
          <Line x1={20} y1={80} x2={80} y2={20} stroke={OFF_RED} strokeWidth={Math.max(hair * 1.6, 3.2)} strokeLinecap="round" />
        </Svg>
      ) : null}
    </View>
  );
}

/** The app mark: the lit tile at rest (the pairing screens, an about line). */
export function GhostMark({ size = 24 }: { size?: number }) {
  return <GhostChip size={size} />;
}

const GLYPH = { sm: 20, md: 24, lg: 44, xl: 56 } as const;

/**
 * A session's or a ring's agent by slug, in the look the desk has for it (ADR 0061). Ghost is
 * the mark itself, the lit tile: the lit look is Ghost's alone.
 */
export function LineGlyph({ slug, size = "md" }: { slug: string; name?: string; size?: keyof typeof GLYPH }) {
  const { conversations } = useDesk();
  const look = conversations.find((c) => c.slug === slug)?.look;
  return <AgentMark slug={slug} look={look} size={GLYPH[size]} />;
}

/**
 * An agent's mark as the desktop draws it (ADR 0061): its shape from the shared family
 * (`@ghost/ui` agentShapes) in its colour, Wisp's two eyes cut in the ground's colour. An agent
 * with no look is Wisp's body at rest, flat in its colour; Ghost is the mark, the lit tile.
 */
export function AgentMark({ slug, look, size = 40 }: { slug: string; look?: string | null; size?: number }) {
  const c = useColors();
  if (slug === "ghost") return <GhostMark size={size} />;
  const l = lookOf(slug, look);
  const fg = c.hue[l.hue].fg;
  if (l.shape === "wisp") return <GhostGlyph size={size} color={fg} />;
  const d = SHAPES[l.shape];
  return (
    <View accessible={false} style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}>
      <Svg width={size} height={size} viewBox="0 0 24 24">
        <Path d={d.body} fill={fg} />
        {d.eyes.map(([x, y, rx, ry], i) => (
          <Ellipse key={i} cx={x} cy={y} rx={rx} ry={ry} fill={c.bgCanvas} />
        ))}
      </Svg>
    </View>
  );
}
