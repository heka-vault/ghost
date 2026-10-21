/**
 * The ghost glyph, the app mark and a line's tile, drawn as on desktop
 * (apps/desktop/src/ui/Brand.tsx). The glyph is Wisp, the mascot, drawn still: at 40px and up
 * it is Wisp saying hello (grinning, mid-wave); below that, where the arms are not drawn, the
 * plain body. Every line gets the ghost in its own colour: the colour is the identity.
 */
import { lineHue, wavePose } from "@ghost/ui";
import { useMemo } from "react";
import { View } from "react-native";

import { useColors } from "../theme";
import { Mascot } from "./Mascot";

const GLYPH_MIN = 40;

/** Wisp saying hello at every size: drawn at 40 (where its arms show) and scaled down below that. */
export function GhostGlyph({ size = 16, color }: { size?: number; color: string }) {
  const frame = useMemo(() => wavePose(), []);
  if (size >= GLYPH_MIN) return <Mascot size={size} color={color} frame={frame} />;
  const k = size / GLYPH_MIN;
  return (
    <View style={{ width: size, height: size, overflow: "visible" }}>
      <View style={{ width: GLYPH_MIN, height: GLYPH_MIN, transform: [{ translateX: -(GLYPH_MIN - size) / 2 }, { translateY: -(GLYPH_MIN - size) / 2 }, { scale: k }] }}>
        <Mascot size={GLYPH_MIN} color={color} frame={frame} />
      </View>
    </View>
  );
}

/** The app mark: the glyph on an accent tile. */
export function GhostMark({ size = 24 }: { size?: number }) {
  const c = useColors();
  return (
    <View
      accessible={false}
      style={{ width: size, height: size, borderRadius: Math.round(size * 0.28), backgroundColor: c.accent, alignItems: "center", justifyContent: "center" }}
    >
      <GhostGlyph size={Math.round(size * 0.66)} color="#ffffff" />
    </View>
  );
}

const GLYPH = { sm: 20, md: 24, lg: 44, xl: 56 } as const;

/**
 * A line's mark: Wisp in the line's own colour, nothing around it. The soft tile with the
 * initial was the old icon (removed 2026-10-20 at the operator's request); the colour is
 * the identity, and at lg and xl Wisp is saying hello.
 */
export function LineGlyph({ slug, size = "md" }: { slug: string; name?: string; size?: keyof typeof GLYPH }) {
  const c = useColors();
  return (
    <View accessible={false} style={{ width: GLYPH[size], height: GLYPH[size], alignItems: "center", justifyContent: "center" }}>
      <GhostGlyph size={GLYPH[size]} color={c.hue[lineHue(slug)].fg} />
    </View>
  );
}
