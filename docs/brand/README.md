# The brand's files

Drawn from the one drawing in `packages/ui/src/mark.ts` (docs/design-system.md, The mark) by
`node packages/ui/scripts/marks.mjs site` (part of `make marks`). Don't edit them by hand: change
the drawing and run the script.

| File | What it is for |
|---|---|
| `lockup-dark.png`, `lockup-light.png` | The README's header: the chip and "Ghost" in Inter 800, the word in the cool ink for GitHub's dark theme and in the night for its light one (a `<picture>` picks one). 112 px tall, shown at 56. |
| `avatar-512.png` | The organisation's avatar on GitHub: the mark at 512 with the ring of ticks, full bleed on the night (GitHub rounds the corners). Uploaded by hand: github.com › the organisation › Settings › Profile › Profile picture. |

The site's (`site/public/`): `favicon.ico` (16, 32, 48), `apple-touch-icon.png` (180, full bleed)
and `og.jpg`, the social card (1200×630: the reach, `reachCardSvg`). The SVG favicon is drawn at
build time (`site/src/pages/favicon.svg.ts`).
