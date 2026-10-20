import { createContext, useContext } from "react";

import { GHOST_PATH, GHOST_PATH_SMALL, lineHue, wavePose } from "@ghost/ui";

import { cx } from "./cx";
import { Mascot } from "./Mascot";

export { GHOST_PATH, GHOST_PATH_SMALL, lineHue };

/**
 * The ghost glyph: Wisp drawn still. At 40px and up it is Wisp saying hello (grinning,
 * mid-wave); below that, where the arms are not drawn, the plain body.
 */
export function GhostGlyph({ size = 16, className, title }: { size?: number; className?: string; title?: string }) {
  return <Mascot size={size} frame={size >= 40 ? wavePose() : {}} title={title} className={cx("g-ghost-glyph", className)} />;
}

/**
 * The app mark: the glyph on an accent tile. Sizes 20, 24, 28, 40, 64. Pass `hovered` (and
 * optionally `trigger`) to make it the live mascot, which plays and settles back to this mark.
 */
export function GhostMark({ size = 24, hovered, trigger, className }: { size?: number; hovered?: boolean; trigger?: number; className?: string }) {
  const glyph = Math.round(size * 0.66);
  return (
    <span className={cx("g-mark", className)} style={{ width: size, height: size, borderRadius: Math.round(size * 0.28) }} aria-hidden>
      {hovered === undefined && trigger === undefined ? <GhostGlyph size={glyph} /> : <Mascot size={glyph} hovered={hovered} trigger={trigger} />}
    </span>
  );
}

export interface LineGlyphProps {
  slug: string;
  name: string;
  size?: "sm" | "md" | "lg" | "xl";
  /** Override the hue (0-7). */
  hue?: number;
  className?: string;
}

/**
 * A conversation's ("line's") tile: the ghost in the line's own colour, on its soft hue.
 * The colour is the identity (the operator's call, 2026-10-20); the name sits beside it.
 */
/**
 * Overrides LineGlyph's size for a subtree. Page's compact bar sets "sm" so a header's
 * leading glyph repeats there at 20px without the screen passing two sizes.
 */
export const GlyphSizeContext = createContext<LineGlyphProps["size"] | null>(null);

export function LineGlyph({ slug, size: own = "md", hue, className }: LineGlyphProps) {
  const size = useContext(GlyphSizeContext) ?? own;
  const h = hue ?? lineHue(slug);
  const px = { sm: 14, md: 16, lg: 20, xl: 28 }[size];
  return (
    <span className={cx("g-line-glyph", `g-line-glyph--${size}`, className)} style={{ color: `var(--hue-${h}-fg)`, background: `var(--hue-${h}-bg)` }} aria-hidden>
      <GhostGlyph size={px} />
    </span>
  );
}
