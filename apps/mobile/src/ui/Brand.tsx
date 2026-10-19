/** The ghost glyph, the app mark and a line's tile, drawn as on desktop (apps/desktop/src/ui/Brand.tsx). */
import { GHOST_PATH, GHOST_PATH_SMALL, lineHue } from "@ghost/ui";
import { View } from "react-native";
import Svg, { Path } from "react-native-svg";

import { useColors } from "../theme";
import { Text } from "./Text";

export function GhostGlyph({ size = 16, color }: { size?: number; color: string }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24">
      <Path fill={color} fillRule="evenodd" d={size < 24 ? GHOST_PATH_SMALL : GHOST_PATH} />
    </Svg>
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

const GLYPH = {
  sm: { box: 20, r: 6, text: "xs", glyph: 14 },
  md: { box: 24, r: 6, text: "sm", glyph: 16 },
  lg: { box: 32, r: 8, text: "lg", glyph: 20 },
  xl: { box: 48, r: 12, text: "2xl", glyph: 28 },
} as const;

/** A line's tile: the ghost for Ghost, the initial for every other line, on its own soft hue. */
export function LineGlyph({ slug, name, size = "md" }: { slug: string; name: string; size?: keyof typeof GLYPH }) {
  const c = useColors();
  const g = GLYPH[size];
  const hue = c.hue[lineHue(slug)];
  return (
    <View accessible={false} style={{ width: g.box, height: g.box, borderRadius: g.r, backgroundColor: hue.bg, alignItems: "center", justifyContent: "center" }}>
      {slug === "ghost" ? (
        <GhostGlyph size={g.glyph} color={hue.fg} />
      ) : (
        <Text size={g.text} weight="semibold" style={{ color: hue.fg, lineHeight: undefined }}>
          {(name.trim()[0] ?? "?").toUpperCase()}
        </Text>
      )}
    </View>
  );
}
