/**
 * The favicon: the mark from the brand's own drawing (packages/ui, `markSvg`; docs/design-system.md,
 * The mark), so the site's icon and the app's can't drift apart. A tab draws it at 16 CSS
 * pixels, so it is the 16 px drawing: the plain hem lit on the night tile, larger eyes snapped to
 * whole pixels, no mitts. `/favicon.ico` (16, 32, 48) is for browsers that don't take an SVG;
 * `node packages/ui/scripts/marks.mjs site` writes it.
 */
import { markSvg } from "@ghost/ui/src/mark";

export function GET() {
  return new Response(markSvg(16, { id: "fav" }), { headers: { "content-type": "image/svg+xml" } });
}
