/** The ghost's drawing and the line hues, shared by the desktop and phone apps. */

/** The ghost silhouette, filled with currentColor; the eyes are cut out. 24x24 grid. */
export const GHOST_PATH =
  "M12 2.75c-3.87 0-7 3.13-7 7v10.1c0 .78.9 1.2 1.5.72l.95-.76a1 1 0 0 1 1.25 0l1.05.84a1 1 0 0 0 1.25 0l1.05-.84a1 1 0 0 1 1.25 0l1.05.84a1 1 0 0 0 1.25 0l1.05-.84a1 1 0 0 1 1.25 0l.95.76c.6.48 1.5.06 1.5-.72V9.75c0-3.87-3.13-7-7-7ZM9.6 8.6c-.66 0-1.2.67-1.2 1.5s.54 1.5 1.2 1.5 1.2-.67 1.2-1.5-.54-1.5-1.2-1.5Zm4.8 0c-.66 0-1.2.67-1.2 1.5s.54 1.5 1.2 1.5 1.2-.67 1.2-1.5-.54-1.5-1.2-1.5Z";

/**
 * The ghost drawn for small sizes (under 24px) on the same 24 grid: two scallops, a 2px
 * bottom gap and larger eyes, so it still reads as a ghost at 12 to 16px instead of a blob.
 */
export const GHOST_PATH_SMALL =
  "M12 3a7 7 0 0 0-7 7v10.3c0 .75.86 1.15 1.43.67L8.4 19a1 1 0 0 1 1.3 0l1.65 1.7a1 1 0 0 0 1.3 0L14.3 19a1 1 0 0 1 1.3 0l1.97 1.97c.57.48 1.43.08 1.43-.67V10a7 7 0 0 0-7-7ZM8.3 10.7a1.3 1.6 0 1 0 2.6 0a1.3 1.6 0 1 0-2.6 0ZM13.1 10.7a1.3 1.6 0 1 0 2.6 0a1.3 1.6 0 1 0-2.6 0Z";

/** Stable hue index (0-7) for a conversation slug. Ghost itself is always 0 (the accent). */
export function lineHue(slug: string): number {
  if (slug === "ghost") return 0;
  let h = 0;
  for (let i = 0; i < slug.length; i++) h = (h * 31 + slug.charCodeAt(i)) >>> 0;
  return 1 + (h % 7);
}
