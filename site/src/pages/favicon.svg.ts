/**
 * The favicon: Wisp at rest from the brand's own drawing (packages/ui, `wispGlyph`), so the
 * site's mark and the app's can't drift apart. The small drawing (larger eyes) since a favicon
 * is shown at 16 to 32px; glow-cyan on the stage's black, the floating room's palette.
 */
import { wispGlyph } from "@ghost/ui/src/wisp";

export function GET() {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="#05060a"/><path fill="#5fe3ff" fill-rule="evenodd" transform="translate(3.03 1.88) scale(1.081)" d="${wispGlyph(true)}"/></svg>`;
  return new Response(svg, { headers: { "content-type": "image/svg+xml" } });
}
