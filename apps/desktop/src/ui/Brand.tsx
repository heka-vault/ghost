import { createContext, useContext } from "react";

import { GHOST_PATH, GHOST_PATH_SMALL, lineHue } from "@ghost/ui";

import { cx } from "./cx";
import { Mascot } from "./Mascot";

export { GHOST_PATH, GHOST_PATH_SMALL, lineHue };

export function GhostGlyph({ size = 16, className, title }: { size?: number; className?: string; title?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" className={cx("g-ghost-glyph", className)} role={title ? "img" : undefined} aria-label={title} aria-hidden={title ? undefined : true}>
      <path fill="currentColor" fillRule="evenodd" d={size < 24 ? GHOST_PATH_SMALL : GHOST_PATH} />
    </svg>
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
 * A conversation's ("line's") tile: Ghost gets the ghost, every other line its initial,
 * each on its own soft hue. Color is never the only cue: the letter carries identity.
 */
/**
 * Overrides LineGlyph's size for a subtree. Page's compact bar sets "sm" so a header's
 * leading glyph repeats there at 20px without the screen passing two sizes.
 */
export const GlyphSizeContext = createContext<LineGlyphProps["size"] | null>(null);

export function LineGlyph({ slug, name, size: own = "md", hue, className }: LineGlyphProps) {
  const size = useContext(GlyphSizeContext) ?? own;
  const h = hue ?? lineHue(slug);
  const isGhost = slug === "ghost";
  const px = { sm: 14, md: 16, lg: 20, xl: 28 }[size];
  return (
    <span className={cx("g-line-glyph", `g-line-glyph--${size}`, className)} style={{ color: `var(--hue-${h}-fg)`, background: `var(--hue-${h}-bg)` }} aria-hidden>
      {isGhost ? <GhostGlyph size={px} /> : (name.trim()[0] ?? "?").toUpperCase()}
    </span>
  );
}
