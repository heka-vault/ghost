/**
 * Phone buttons: md 48 (the minimum target), lg 56, xl 64 (Start), hero 80 (End). Pills,
 * semibold, pressed state darkens and scales a hair. Loading keeps the label and the width.
 */
import type { ReactNode } from "react";
import { ActivityIndicator, Pressable, type StyleProp, View, type ViewStyle } from "react-native";

import { type Colors, radius, space, useColors } from "../theme";
import { Text } from "./Text";

export type TouchVariant = "primary" | "secondary" | "inverse" | "ghost" | "danger" | "danger-ghost";
export type TouchSize = "md" | "lg" | "xl" | "hero";

const SIZES: Record<TouchSize, { height: number; padding: number; text: "lg" | "xl" | "2xl" }> = {
  md: { height: space[12], padding: space[5], text: "lg" },
  lg: { height: space[12] + space[2], padding: space[6], text: "xl" },
  xl: { height: space[16], padding: space[8], text: "xl" },
  hero: { height: space[20], padding: space[8], text: "2xl" },
};

function look(c: Colors, variant: TouchVariant, pressed: boolean): { bg: string; fg: string; border: string; opacity?: number } {
  switch (variant) {
    case "primary":
      return { bg: pressed ? c.accentActive : c.accent, fg: c.onAccent, border: "transparent" };
    case "secondary":
      return { bg: pressed ? c.bgActive : c.bgSurface, fg: c.text1, border: c.borderStrong };
    case "inverse":
      return { bg: c.bgInverse, fg: c.textInverse, border: "transparent", opacity: pressed ? 0.84 : 1 };
    case "ghost":
      return { bg: pressed ? c.bgActive : "transparent", fg: c.text1, border: "transparent" };
    case "danger":
      return { bg: pressed ? c.dangerFillActive : c.dangerFill, fg: c.onAccent, border: "transparent" };
    case "danger-ghost":
      return { bg: pressed ? c.dangerSoft : "transparent", fg: c.danger, border: "transparent" };
  }
}

export interface TouchButtonProps {
  variant?: TouchVariant;
  size?: TouchSize;
  icon?: (color: string) => ReactNode;
  loading?: boolean;
  disabled?: boolean;
  onPress?: () => void;
  fullWidth?: boolean;
  style?: StyleProp<ViewStyle>;
  children: ReactNode;
  accessibilityHint?: string;
}

export function TouchButton({ variant = "secondary", size = "md", icon, loading, disabled, onPress, fullWidth, style, children, accessibilityHint }: TouchButtonProps) {
  const c = useColors();
  const s = SIZES[size];
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityState={{ disabled: !!disabled, busy: !!loading }}
      accessibilityHint={accessibilityHint}
      disabled={disabled}
      onPress={() => {
        if (!loading) onPress?.();
      }}
      style={({ pressed }) => {
        const l = look(c, variant, pressed && !loading);
        const off = disabled && variant !== "ghost";
        return [
          {
            height: s.height,
            paddingHorizontal: s.padding,
            borderRadius: radius.full,
            borderWidth: 1,
            borderColor: off ? "transparent" : l.border,
            backgroundColor: off ? c.bgActive : l.bg,
            opacity: l.opacity,
            flexDirection: "row",
            alignItems: "center",
            justifyContent: "center",
            gap: space[2],
            alignSelf: fullWidth ? "stretch" : "flex-start",
            transform: [{ scale: pressed && !disabled && !loading ? 0.985 : 1 }],
          },
          style,
        ];
      }}
    >
      {({ pressed }) => {
        const l = look(c, variant, pressed);
        const fg = disabled ? c.textDisabled : l.fg;
        return (
          <>
            {(icon || loading) && <View>{loading ? <ActivityIndicator size="small" color={fg} /> : icon?.(fg)}</View>}
            <Text size={s.text} weight="semibold" style={{ color: fg }} numberOfLines={1}>
              {children}
            </Text>
          </>
        );
      }}
    </Pressable>
  );
}

/** A 48dp icon-only button with a required accessible name. */
export function TouchIcon({ label, icon, onPress, pressed: on }: { label: string; icon: (color: string) => ReactNode; onPress: () => void; pressed?: boolean }) {
  const c = useColors();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={on == null ? undefined : { selected: on }}
      onPress={onPress}
      hitSlop={4}
      style={({ pressed }) => ({
        width: space[12],
        height: space[12],
        borderRadius: radius.full,
        alignItems: "center",
        justifyContent: "center",
        backgroundColor: pressed ? c.bgActive : on ? c.bgSelected : "transparent",
      })}
    >
      {icon(c.text1)}
    </Pressable>
  );
}
