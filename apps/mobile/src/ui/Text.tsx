/**
 * The one text component. Size and weight come from the type scale, color from the theme,
 * tracking from the size (tight at 16 and up, tighter at 24 and up), as the CSS utilities do.
 */
import { Text as RNText, type TextProps as RNTextProps } from "react-native";

import { type ColorName, fonts, tracking, type TypeSize, type as scale, useColors } from "../theme";

export interface TextProps extends RNTextProps {
  size?: TypeSize;
  weight?: "regular" | "medium" | "semibold";
  color?: ColorName;
  mono?: boolean;
  /** Tabular numerals, for clocks and counters. */
  num?: boolean;
  /** The 11/16 caps overline (12px on the phone). */
  caps?: boolean;
  align?: "left" | "center" | "right";
}

export function Text({ size = "lg", weight = "regular", color = "text1", mono, num, caps, align, style, ...rest }: TextProps) {
  const c = useColors();
  const { fontSize, lineHeight } = scale[size];
  return (
    <RNText
      {...rest}
      style={[
        {
          fontFamily: mono ? fonts.mono : fonts[weight],
          fontSize: mono ? fontSize - 1 : fontSize,
          lineHeight,
          letterSpacing: caps ? fontSize * 0.04 : mono ? 0 : tracking(fontSize),
          color: c[color],
          textTransform: caps ? "uppercase" : undefined,
          fontVariant: num ? ["tabular-nums"] : undefined,
          textAlign: align,
        },
        style,
      ]}
    />
  );
}
