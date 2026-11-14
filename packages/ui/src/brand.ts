/** The ghost's drawing and the line hues, shared by the desktop and phone apps. */
import { wispPath } from "./figure";

/**
 * The ghost: Wisp (figure.ts) at rest with its mitts, one colour, the eyes cut out, on the 24
 * grid. Fill it with the default (nonzero) rule, never evenodd. The live mascot settles to
 * exactly this drawing. For the brand's mark (the lit tile, the ring, the icons) use mark.ts.
 */
export const GHOST_PATH = wispPath({ mitts: true });

/**
 * The ghost drawn under 32 px on the same grid: the plain hem with no mitts and larger eyes,
 * so it still reads as a ghost at 12 to 16 px instead of a blob.
 */
export const GHOST_PATH_SMALL = wispPath({ mitts: false, small: true });

/** Stable hue index (0-7) for a conversation slug. Ghost itself is always 0 (the accent). */
export function lineHue(slug: string): number {
  if (slug === "ghost") return 0;
  let h = 0;
  for (let i = 0; i < slug.length; i++) h = (h * 31 + slug.charCodeAt(i)) >>> 0;
  return 1 + (h % 7);
}
