import { createContext, useContext } from "react";

import { cx } from "./cx";

/** The ghost silhouette, filled with currentColor; the eyes are cut out. 24x24 grid. */
export const GHOST_PATH =
  "M12 2.75c-3.87 0-7 3.13-7 7v10.1c0 .78.9 1.2 1.5.72l.95-.76a1 1 0 0 1 1.25 0l1.05.84a1 1 0 0 0 1.25 0l1.05-.84a1 1 0 0 1 1.25 0l1.05.84a1 1 0 0 0 1.25 0l1.05-.84a1 1 0 0 1 1.25 0l.95.76c.6.48 1.5.06 1.5-.72V9.75c0-3.87-3.13-7-7-7ZM9.6 8.6c-.66 0-1.2.67-1.2 1.5s.54 1.5 1.2 1.5 1.2-.67 1.2-1.5-.54-1.5-1.2-1.5Zm4.8 0c-.66 0-1.2.67-1.2 1.5s.54 1.5 1.2 1.5 1.2-.67 1.2-1.5-.54-1.5-1.2-1.5Z";

/**
 * The ghost drawn for small sizes (under 24px) on the same 24 grid: two scallops, a 2px
 * bottom gap and larger eyes, so it still reads as a ghost at 12 to 16px instead of a blob.
 */
export const GHOST_PATH_SMALL =
  "M12 3a7 7 0 0 0-7 7v10.3c0 .75.86 1.15 1.43.67L8.4 19a1 1 0 0 1 1.3 0l1.65 1.7a1 1 0 0 0 1.3 0L14.3 19a1 1 0 0 1 1.3 0l1.97 1.97c.57.48 1.43.08 1.43-.67V10a7 7 0 0 0-7-7ZM8.3 10.7a1.3 1.6 0 1 0 2.6 0a1.3 1.6 0 1 0-2.6 0ZM13.1 10.7a1.3 1.6 0 1 0 2.6 0a1.3 1.6 0 1 0-2.6 0Z";

export function GhostGlyph({ size = 16, className, title }: { size?: number; className?: string; title?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" className={cx("g-ghost-glyph", className)} role={title ? "img" : undefined} aria-label={title} aria-hidden={title ? undefined : true}>
      <path fill="currentColor" fillRule="evenodd" d={size < 24 ? GHOST_PATH_SMALL : GHOST_PATH} />
    </svg>
  );
}

/** The app mark: the glyph on an accent tile. Sizes 20, 24, 28, 40, 64. */
export function GhostMark({ size = 24, className }: { size?: number; className?: string }) {
  return (
    <span className={cx("g-mark", className)} style={{ width: size, height: size, borderRadius: Math.round(size * 0.28) }} aria-hidden>
      <GhostGlyph size={Math.round(size * 0.66)} />
    </span>
  );
}

/** Stable hue index (0-7) for a conversation slug. Ghost itself is always 0 (the accent). */
export function lineHue(slug: string): number {
  if (slug === "ghost") return 0;
  let h = 0;
  for (let i = 0; i < slug.length; i++) h = (h * 31 + slug.charCodeAt(i)) >>> 0;
  return 1 + (h % 7);
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
