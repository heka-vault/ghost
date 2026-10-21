import { createContext, useContext } from "react";

import { GHOST_PATH, GHOST_PATH_SMALL, lineHue, wavePose } from "@ghost/ui";

import { cx } from "./cx";
import { Mascot } from "./Mascot";

export { GHOST_PATH, GHOST_PATH_SMALL, lineHue };

/**
 * The ghost glyph: Wisp saying hello (grinning, mid-wave), drawn still, at every size. The
 * mascot only draws its arms at 40px and up, so smaller glyphs are the 40px drawing scaled
 * down: the plain body at small sizes read as the old icon (the operator, 2026-10-20).
 */
const GLYPH_MIN = 40;
export function GhostGlyph({ size = 16, className, title }: { size?: number; className?: string; title?: string }) {
  if (size >= GLYPH_MIN) return <Mascot size={size} frame={wavePose()} title={title} className={cx("g-ghost-glyph", className)} />;
  const k = size / GLYPH_MIN;
  return (
    <span className={cx("g-ghost-glyph", className)} style={{ display: "inline-block", width: size, height: size, overflow: "visible", lineHeight: 0 }} aria-hidden={title ? undefined : true} role={title ? "img" : undefined} aria-label={title}>
      <span style={{ display: "inline-block", transform: `scale(${k})`, transformOrigin: "top left", width: GLYPH_MIN, height: GLYPH_MIN, lineHeight: 0 }}>
        <Mascot size={GLYPH_MIN} frame={wavePose()} />
      </span>
    </span>
  );
}

/**
 * The app mark: the glyph on an accent tile. Sizes 20, 24, 28, 40, 64. Pass `hovered` (and
 * optionally `trigger`) to make it the live mascot, which plays and settles back to this mark.
 */
export function GhostMark({ size = 24, hovered, trigger, className }: { size?: number; hovered?: boolean; trigger?: number; className?: string }) {
  const glyph = Math.round(size * 0.66);
  return (
    <span className={cx("g-mark", className)} style={{ width: size, height: size, borderRadius: Math.round(size * 0.28) }} aria-hidden>
      {/* Still: Wisp saying hello at any size. While hovered (or playing), the live mascot. */}
      {hovered || (trigger !== undefined && trigger > 0) ? <Mascot size={glyph} hovered={hovered} trigger={trigger} /> : <GhostGlyph size={glyph} />}
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
