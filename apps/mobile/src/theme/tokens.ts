/**
 * The design system's tokens for React Native, copied from apps/desktop/src/ui/tokens.css.
 * That file is the source: when a value changes there, change it here. Colors are
 * [light, dark] pairs, resolved once per render by useTheme().
 *
 * Phone ergonomics (mobile.css in the prototype): every target is at least 48dp, Start is
 * 64, End is 80, and type runs one step larger than desktop.
 */
import type { GhostPhase } from "@ghost/ui";

type Pair = readonly [light: string, dark: string];

const palette = {
  bgApp: ["#f1f1f3", "#09090b"],
  bgCanvas: ["#ffffff", "#0f0f11"],
  bgSurface: ["#fbfbfc", "#141416"],
  bgRaised: ["#ffffff", "#19191c"],
  bgSunken: ["#f6f6f7", "#0c0c0e"],
  bgInput: ["#ffffff", "rgba(255, 255, 255, 0.05)"],
  bgHover: ["rgba(24, 24, 27, 0.035)", "rgba(255, 255, 255, 0.04)"],
  bgActive: ["rgba(24, 24, 27, 0.07)", "rgba(255, 255, 255, 0.075)"],
  bgSelected: ["rgba(24, 24, 27, 0.085)", "rgba(255, 255, 255, 0.085)"],
  bgOverlay: ["rgba(24, 24, 27, 0.32)", "rgba(0, 0, 0, 0.6)"],
  bgInverse: ["#18181b", "#ededef"],

  borderSubtle: ["rgba(24, 24, 27, 0.06)", "rgba(255, 255, 255, 0.055)"],
  border: ["rgba(24, 24, 27, 0.09)", "rgba(255, 255, 255, 0.085)"],
  borderStrong: ["rgba(24, 24, 27, 0.15)", "rgba(255, 255, 255, 0.14)"],
  borderInput: ["rgba(24, 24, 27, 0.5)", "rgba(255, 255, 255, 0.38)"],
  controlTrack: ["#8e8e96", "#63636d"],

  text1: ["#18181b", "#ededef"],
  text2: ["#52525b", "#a1a1aa"],
  text3: ["#64646d", "#8b8b94"],
  textDisabled: ["#a1a1aa", "#56565f"],
  textInverse: ["#ffffff", "#09090b"],

  accent: ["#4c57e0", "#5560e8"],
  accentActive: ["#424cc9", "#4a54d6"],
  accentText: ["#4450d8", "#8b9bff"],
  accentSoft: ["rgba(76, 87, 224, 0.08)", "rgba(110, 124, 255, 0.14)"],
  accentBorder: ["rgba(76, 87, 224, 0.32)", "rgba(139, 155, 255, 0.38)"],
  onAccent: ["#ffffff", "#ffffff"],
  focusRing: ["#4c57e0", "#8b9bff"],

  ok: ["#15803d", "#4ade80"],
  okSoft: ["rgba(21, 128, 61, 0.08)", "rgba(74, 222, 128, 0.12)"],
  okBorder: ["rgba(21, 128, 61, 0.25)", "rgba(74, 222, 128, 0.28)"],
  warn: ["#935605", "#fbbf24"],
  warnSoft: ["rgba(217, 119, 6, 0.09)", "rgba(251, 191, 36, 0.12)"],
  warnBorder: ["rgba(217, 119, 6, 0.3)", "rgba(251, 191, 36, 0.3)"],
  danger: ["#c62828", "#f87171"],
  dangerSoft: ["rgba(198, 40, 40, 0.07)", "rgba(248, 113, 113, 0.12)"],
  dangerBorder: ["rgba(198, 40, 40, 0.25)", "rgba(248, 113, 113, 0.32)"],
  info: ["#1d64d8", "#60a5fa"],
  infoSoft: ["rgba(29, 100, 216, 0.07)", "rgba(96, 165, 250, 0.12)"],
  infoBorder: ["rgba(29, 100, 216, 0.25)", "rgba(96, 165, 250, 0.3)"],
  dangerFill: ["#cf3030", "#cf3030"],
  dangerFillActive: ["#bb2727", "#c93030"],

  phaseStarting: ["#1d64d8", "#60a5fa"],
  phasePassive: ["#76767f", "#8b8b94"],
  phaseListening: ["#15803d", "#4ade80"],
  phaseThinking: ["#7c5cf0", "#b69bff"],
  phaseSpeaking: ["#0891d1", "#5cc8ff"],
  phaseOffline: ["#dc2626", "#f87171"],
} as const satisfies Record<string, Pair>;

/** Conversation (line) glyph hues: fg on its own soft tile is at least 4.5:1. */
const hues: readonly { fg: Pair; bg: Pair }[] = [
  { fg: ["#3f4bc9", "#9db0ff"], bg: ["rgba(76, 87, 224, 0.1)", "rgba(110, 130, 255, 0.16)"] },
  { fg: ["#6d3fd0", "#c4a8ff"], bg: ["rgba(124, 77, 240, 0.1)", "rgba(170, 130, 255, 0.16)"] },
  { fg: ["#b4236f", "#ff9ccf"], bg: ["rgba(220, 50, 140, 0.09)", "rgba(255, 110, 180, 0.15)"] },
  { fg: ["#b4460f", "#ffad7a"], bg: ["rgba(234, 100, 30, 0.1)", "rgba(255, 140, 70, 0.15)"] },
  { fg: ["#8a5a00", "#f5cc5c"], bg: ["rgba(217, 160, 6, 0.13)", "rgba(245, 190, 60, 0.14)"] },
  { fg: ["#17763a", "#6ee39a"], bg: ["rgba(22, 163, 74, 0.1)", "rgba(74, 222, 128, 0.13)"] },
  { fg: ["#0b7365", "#5fd4e6"], bg: ["rgba(13, 148, 136, 0.1)", "rgba(70, 200, 230, 0.13)"] },
  { fg: ["#0a6aa8", "#7cc9ff"], bg: ["rgba(8, 145, 209, 0.1)", "rgba(90, 180, 255, 0.15)"] },
];

export type ColorName = keyof typeof palette;
export type Colors = Record<ColorName, string> & { hue: { fg: string; bg: string }[] };

export function colorsFor(scheme: "light" | "dark"): Colors {
  const i = scheme === "dark" ? 1 : 0;
  const out = {} as Record<ColorName, string>;
  for (const k of Object.keys(palette) as ColorName[]) out[k] = palette[k][i];
  return { ...out, hue: hues.map((h) => ({ fg: h.fg[i], bg: h.bg[i] })) };
}

export function phaseColor(c: Colors, phase: GhostPhase): string {
  return {
    starting: c.phaseStarting,
    passive: c.phasePassive,
    listening: c.phaseListening,
    thinking: c.phaseThinking,
    speaking: c.phaseSpeaking,
    offline: c.phaseOffline,
  }[phase];
}

/** The 4px grid. Half steps are for control internals only. */
export const space = { 0.5: 2, 1: 4, 1.5: 6, 2: 8, 2.5: 10, 3: 12, 4: 16, 5: 20, 6: 24, 8: 32, 10: 40, 12: 48, 16: 64, 20: 80 } as const;

export const radius = { xs: 4, sm: 6, md: 8, lg: 12, xl: 16, full: 999 } as const;

/** Size and line height pairs. */
export const type = {
  xs: { fontSize: 11, lineHeight: 16 },
  sm: { fontSize: 12, lineHeight: 16 },
  md: { fontSize: 13, lineHeight: 20 },
  lg: { fontSize: 14, lineHeight: 22 },
  xl: { fontSize: 16, lineHeight: 24 },
  "2xl": { fontSize: 20, lineHeight: 28 },
  "3xl": { fontSize: 24, lineHeight: 32 },
  "4xl": { fontSize: 32, lineHeight: 40 },
} as const;

export type TypeSize = keyof typeof type;

/** Inter at the three weights the system allows, and JetBrains Mono. Loaded in app/_layout. */
export const fonts = {
  regular: "Inter_400Regular",
  medium: "Inter_500Medium",
  semibold: "Inter_600SemiBold",
  mono: "JetBrainsMono_400Regular",
} as const;

/** Tracking in em, as the CSS writes it: tight at 16px and up, tighter at 24px and up. */
export function tracking(fontSize: number): number {
  if (fontSize >= 24) return fontSize * -0.021;
  if (fontSize >= 16) return fontSize * -0.011;
  return 0;
}

export const duration = { instant: 80, fast: 120, base: 180, slow: 280, slower: 420 } as const;

export const opacityDisabled = 0.45;
