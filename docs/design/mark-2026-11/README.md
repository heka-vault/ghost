# Ghost's mark, a concept round (November 2026)

The operator: the floating room's Wisp is "levels ahead" of the logo on the desktop app, the
phone and the site. This folder is the concept round that answers it. **Nothing shipped
changes here**: no icon, no brand glyph, no asset outside this folder.

## The sheet

Rendered per section in [`sheet/`](sheet/) (2x, small marks pixelated so every pixel shows):

| | |
|---|---|
| [intro](sheet/intro.webp) | today, the floating room and the recommendation side by side |
| [character](sheet/character.webp) | Wisp's still pose against today's drawing; the agents' family beside it |
| [I · Lit](sheet/dir-lit.webp), [II · Instrument](sheet/dir-instrument.webp), [III · Eyes in the dark](sheet/dir-eyes.webp) | each direction at 1024 to 16 px, every pixel at 32/24/16, on dark, light and busy, its states and its one-colour drawing |
| [contexts](sheet/contexts.webp) | dock, tray, the app's rail, browser tab, Android home (four masks and themed), notification, GitHub, README, the site's nav: today and all three |
| [big surfaces](sheet/big.webp) | the reach on the social card and the splash |
| [wordmark](sheet/wordmark.webp) | the lockup |
| [motion](sheet/motion.webp) | [arrival](motion/arrival.webp), [idle breath](motion/breath.webp), [a state change](motion/states.webp) |
| [recommendation](sheet/recommendation.webp) | the call, the scores, what stays flat, what I would not do, the rollout by file |

## Recommendation

**II, the instrument**: lit Wisp inside the floating room's ring of ticks from 48 px up, an arc
of the ring lit in the state's colour; under 32 px the ring goes and Lit's small drawing (I)
is the mark. The night tile is part of the mark, so on a light surface it is a dark chip.
The reach (beams to named places) is for big surfaces only. Wisp's still pose drops the smile
and the arms (gestures, kept for the live mascot), leans −7°, swings the tail and makes the
eyes taller. The wordmark is Inter 800 at −0.035 em; the room's mono capitals are the tagline.

Scores against the floating room (9): today 4, I 7, II 8.5 (7.5 at 16 to 32), III 6.5.

## Build

Plain Node (22+), Chrome and ImageMagick; no dependencies.

```sh
node src/export.mjs   # every mark: svg/ (sources) and png/ (each at its own pixel size, not committed)
node src/sheet.mjs    # sheet/index.html
node src/render.mjs   # sheet/*.webp
node src/motion.mjs   # motion/*.webp
```

`src/marks.mjs` holds every drawing. Wisp's body is copied from `packages/ui/src/wisp.ts`
(`body(s)`) and the agents' shapes are read from `packages/ui/src/agentShapes.ts`. The fonts
are the design system's own (Inter and JetBrains Mono, OFL). Today's marks on the sheet are
resized from the shipped files, read only.
