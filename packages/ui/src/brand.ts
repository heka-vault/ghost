/** The ghost's drawing and the line hues, shared by the desktop and phone apps. */
import { wispGlyph } from "./wisp";

/**
 * The ghost: Wisp (wisp.ts) at rest, filled with currentColor, the eyes cut out. 24x24 grid.
 * The live mascot settles to exactly this drawing.
 */
export const GHOST_PATH = wispGlyph(false);

/**
 * The ghost drawn for small sizes (under 24px) on the same 24 grid: larger eyes, so it still
 * reads as a ghost at 12 to 16px instead of a blob.
 */
export const GHOST_PATH_SMALL = wispGlyph(true);

/** Stable hue index (0-7) for a conversation slug. Ghost itself is always 0 (the accent). */
export function lineHue(slug: string): number {
  if (slug === "ghost") return 0;
  let h = 0;
  for (let i = 0; i < slug.length; i++) h = (h * 31 + slug.charCodeAt(i)) >>> 0;
  return 1 + (h % 7);
}
