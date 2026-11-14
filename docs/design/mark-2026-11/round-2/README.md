# Round 2: lose the tail, keep the hands

The operator, on round 1: the tail makes Wisp read as an inverted raindrop, or worse; they
liked today's large icon with the hands, and asked for a round that rounds out the bottom
half. The light, the night tile, the ring and the state colours from round 1 stay; this round
is the silhouette. **Nothing shipped changes.**

The sheet, per section in [`sheet/`](sheet/): [the silhouette test](sheet/silhouettes.webp)
(each shape in solid black at 64 and 16 px, read bluntly), the directions in the ring and in
context with the agents beside them ([1a hem](sheet/dir-hem.webp),
[1b one round hem](sheet/dir-hemRound.webp), [1c hem with hands](sheet/dir-hemMitts.webp),
[2 hands on today's tail](sheet/dir-tailMitts.webp), [3a no bottom half](sheet/dir-fade.webp),
[3b rising from the frame](sheet/dir-peek.webp), [4 orb](sheet/dir-orb.webp),
[5 drift](sheet/dir-drift.webp)), [1c's states](sheet/states.webp),
[the two strongest alive](sheet/motion.webp) ([`motion/`](motion/)) and
[the recommendation](sheet/recommendation.webp).

**Recommendation: 1c, the hem with hands**, 8.5/10. It reads as a ghost and a character in
black at every size; under 32 px the hands drop and the hem alone is the mark. It is generic
in black (the most familiar ghost there is); the light, the ring and the hands make it Ghost's.
For the live mascot, `body(s)` becomes a hem whose lobes ripple (`sw` drives the ripple instead
of the tail's swing) and the arm lines become mitts on the same `aL`/`aR`.

The chosen direction (1c) shipped as `packages/ui/src/figure.ts` (the body) and `mark.ts` (the
mark); the round's generators were removed then. They are in git history
(`git show a6200af0:docs/design/mark-2026-11/round-2/src/figures.mjs`).
