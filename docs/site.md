# The site: heyghost.app

`site/` is the public site, a static Astro page served by GitHub Pages at
<https://heyghost.app>. One page, leading with the float; the docs stay on GitHub and the
page links to them.

## What it is made of

- **The hero is the app's own floating view over the README's desktop.** The first screen is
  that desktop edge to edge, with the words over its quiet left side; on a phone (or any screen
  taller than wide) the float takes the top of the screen, centred, with the words under it.
  `site/src/pages/float.astro` mounts `FloatingRoom` from `apps/desktop/src/floating` on the
  site's own small mock of the `floating-site` scenario (`apps/desktop/src/design/mock/site.ts`:
  the one turn and the few routes the floating view reads, a few kilobytes where the desktop's
  whole mock daemon and its fixtures are about 250 KB; `apps/desktop/test/floating-site-mock.test.ts`
  holds it to the desktop's `?mock=floating-site` event for event on the same clock), one turn on
  the clock of the hero's sound, below, repeating every 24.6 s: the wake at 1.2 s, six places while Ghost thinks,
  the reply from 6.1 s, a yes waiting from 11.7 s, the goodbye at 21 s) over
  `apps/desktop/shots/world.{ts,css}`, the same made-up desktop
  `shots/desktop.html` puts under the README's pictures, its windows moved right of the middle
  (`?arrange=site`). The front page shows a still first, loads that page behind it once the
  page has loaded and the hero is on screen, and shows it the moment Ghost first comes forward
  (the page posts `ghost:forward`). After two turns (about 50 s) with nobody touching the hero
  the front page counts the float's `ghost:loop` and asks it to rest (`ghost:rest`): the scene
  and every timer stop, and Wisp waits where it comes forward, lit and drawn once, with Play
  beside it; pointing at the hero, a tap, Play, Hear it or scrolling back starts the turn again
  from the wake. The frame stays at rest rather than going: it costs what an emptied one does
  (below) and starts again at once. It empties the frame after two seconds off screen, and stays
  a still with JavaScript off, under reduced motion, or with Save-Data (or `prefers-reduced-data`)
  on, with a Play button. The site's build carries no commit (`__GHOST_WINDOW_BUILD__` is null),
  so the float's files keep their names from one deploy to the next unless their code changed. When the floating
  room, the mock or the world changes, the site changes with it: a pull request touching
  `apps/desktop/src/**` or `apps/desktop/shots/**` builds the site too.
- **The hero's sound, on request.** The page loads silent and stays so until the visitor presses
  **Hear it** (a small cyan wave that moves with Ghost's voice, at the hero's foot; the same
  button turns it off; the choice is not remembered). The press plays the clip inside the
  gesture, as Chrome's, Firefox's and Safari's autoplay rules ask, and the float starts its turn
  again from the wake; from then on the clip goes back to its start each time the float says its
  turn starts (`ghost:loop`, 60 ms in, the time a clip takes to sound again after the jump), so
  the beams, the light and the words stay within about 35 ms of each other. The words show as
  captions at the hero's foot, with the chimes and the thinking sound named. With reduced motion
  the sound plays over the still; off screen it pauses. Nothing of it loads before the press:
  the clip (`site/public/audio/`, WebM/Opus where the browser takes it, else AAC in MP4) is one
  request on the first press. The timeline is a 2 KB script the float loads anyway, since it is
  the float's clock.
- **The sound is rendered, not recorded.** `site/audio/turn.json` holds the lines and their
  gaps; `pnpm --dir site site:audio` (tools/oracle's environment through `uv`, Kokoro from
  `scripts/models.sh kokoro`, Rust for the cues, ffmpeg; nothing fetched) renders each line in
  Kokoro, the person in `am_michael` and Ghost in the daemon's own default (`af_heart`, speed
  1.0), and lays the lines out from their real lengths. It mixes in Ghost's own cue sounds as
  the daemon plays them: the chimes and the thinking loop come from `ghost_audio::sounds`
  (`cargo run -p ghost-audio --example site-sounds`, byte for byte the daemon's), the loop at
  the mixer's 0.12 from half a second after the request. The person's voice is matched to
  Ghost's loudness, and the whole is set to -16 LUFS under a -2.5 dBFS limiter. It writes the
  two files and `apps/desktop/src/design/mock/fixtures/floating-site-turn.json`: each line's
  start and length, and Ghost's level per 50 ms window, computed as `ghost_voice::level` does.
  The mock plays the turn on that clock, with that level as its `voice` event, so the light
  moves with the words with the sound off too. The render is byte-for-byte repeatable. `--check`
  reads every line back from both encoded files with Parakeet (word error rate at most 0.2 per
  line) and measures the loudness, the true peak, clipping, the two voices' balance and the
  size. Change a line, render, commit the four files.
- **Every voice is synthetic** (Kokoro-82M, Apache-2.0, credited in the footer); no recording of
  anyone is used. `pnpm --dir site site:sound-check` (after `site:build`) checks it in browsers,
  with nothing played aloud. Stock Chrome and Firefox (fresh profiles, headless) must refuse to
  play with no gesture; Playwright-driven browsers always allow it, so that part is checked
  apart. Then, in Chrome and Playwright's Firefox: no clip before the press, one clip after it,
  the float's scenes within 150 ms of the clip's clock, the captions, the button turning it off,
  and reduced motion playing over the still. `--video out.mp4` writes Chrome's capture of one
  turn with the clip muxed in, for review.
- **The pictures below the hero** are the README's stills of Wisp's states
  (`docs/screenshots/floating-{hero,speaking,yes}.webp`) and one of the site's own
  (`site/src/assets/float-listening.webp`, the README's desk and crop), and the desktop app's
  screenshots, all straight from `docs/screenshots/`.
- **The site's stills** (`site/src/assets/float-{wide,tall,listening}.webp`) are taken from the
  float page: `pnpm --dir site site:build && pnpm --dir site site:stills` (Chrome or Chromium
  and ImageMagick, like `make screenshots`). The still is the live float caught at one moment,
  so when the live one takes its place Wisp's body doesn't change: take the stills again
  whenever the floating view's drawing does.
- **The mark** (docs/design-system.md, The mark): the nav and the footer carry the lockup
  (`Wordmark.astro`: the 32 px chip from `markSvg`, inlined, and "Ghost" in Inter 800 at
  −0.035 em). The icons and the social card are the mark's own files, not stills:
  `node packages/ui/scripts/marks.mjs site` (part of `make marks`) writes `site/public/favicon.ico`,
  `apple-touch-icon.png` and `og.jpg` (the reach: the lit Wisp reaching, a beam to each named
  place, the name and the line; `reachCardSvg`), with the README's header and the
  organisation's avatar (`docs/brand/`).
- **The version** on the page (the Download button, the file names) is the repository's own,
  read from the root `package.json` at build time, which `scripts/version.py` keeps with every
  other copy; the button goes to the latest release. The .deb and AppImage links name the files
  in `releases/latest/download/` by that version, so between a version bump's merge and its tag
  being pushed (docs/releasing.md) they point at a file not there yet: push the tag soon after.
- **Give this to your agent** sits straight under the hero (`AgentStart.astro`): the prompt a
  person pastes into the coding agent they already have (`site/src/agent-prompt.txt`, the same
  words as the README's Install, held together by `tools/tests/test_agent_setup.py`), a Copy
  button with no library (it says Copied, and selects the words where the clipboard is
  refused), and a line for people without an agent that goes to the .deb and the AppImage. The
  prompt names `/agents.md`, which is `docs/agent-setup.md` served whole as Markdown
  (`src/pages/agents.md.ts`, read at build), and `/llms.txt` says in a few lines what Ghost is
  and points at it. `pnpm --dir site site:verify`, run by the site workflow after the build,
  fails when the built `/agents.md` is not the repository's file or the page lost the prompt.
  The guide is data an agent obeys on a stranger's machine: every command in it must exist in
  the published release and have been run in a clean container (its PR has the transcript).
- **The favicon** is the mark's 16 px drawing from `packages/ui` (`markSvg`), rendered at build
  time (`site/src/pages/favicon.svg.ts`), with `/favicon.ico` beside it for browsers that don't
  take an SVG.
- **Nothing is fetched from anyone else**: fonts are self-hosted (fontsource), no analytics,
  no cookies. Keep it that way.
- **The copy** makes only the promises the README and CLAUDE.md's rules make. When the product
  changes one, change the page in the same pull request.

`site` is a member of the pnpm workspace so it can import the desktop's sources with one
install. It has no `build`, `test` or `typecheck` script, so the root `pnpm build`, `pnpm test`
and `pnpm typecheck` (and the `ci` workflow) don't run it; its own scripts are `site:build`,
`site:check`, `site:verify`, `site:budget`, `site:stills`, `site:audio`, `site:sound-check` and `dev`. It adds about 250
packages to the lock and about a second to a cold `pnpm install` (measured 2026-11-13: 3.4 s to
4.4 s with an empty store, 690 to 844 MB of store).

```bash
pnpm --dir site dev          # http://localhost:4321
pnpm --dir site site:check   # tsc over the site's TypeScript and what it imports
pnpm --dir site site:build   # site/dist, about 1.5 s
pnpm --dir site site:audio --check   # render the hero's sound, read it back, measure it
pnpm --dir site site:sound-check     # the sound in Chrome and Firefox, headless (after site:build)
pnpm --dir site site:budget          # the hero's bytes and behaviour (after site:build; --bytes-only)
```

### What the hero costs

`site:budget`, run by the site workflow after the build, holds the hero to a budget. Failing:
the front page's first paint at most 200 KB (gzip; the HTML, its CSS and scripts, the preloaded
font and the larger still as served; 160.8 KB on 2026-11-15), the float frame's module graph at
most 115 KB (gzip, every module it reaches; 112.2 KB, of it 102.2 KB of script), and in Chrome
against `astro preview`: the field drawn at most 31 times a second, Wisp's SVG under 400
attribute writes a second, no audio before the press, the scene stopped by itself after two
turns with nothing drawn after, the frame gone three seconds after the hero scrolls away, and
no frame under reduced motion or Save-Data. Warned about only, since a shared runner is noisy
and draws WebGL in software: with the CPU 4x slower on slow 4G, when Ghost comes to life
(ceiling 4.5 s) and the main thread's share while it plays (25%). A Lighthouse score is not
used: it came out bimodal on this page. When a ceiling is passed, say what bought the bytes and
raise it in the same pull request.

Measured 2026-11-15 with the performance review's scripts against a local production build
served as GitHub Pages serves it (gzip, `max-age=600`), Chrome with the GPU, medians of three:

| | desktop | mid phone (4x CPU, slow 4G) | low phone (6x) |
|---|---|---|---|
| Ghost comes to life | 1.57 s (was 1.57) | 3.89 s (was 4.41) | 3.83 s (was 4.43) |
| The float frame on the wire | 125 KB (was 187–202) | 125 KB (was 202) | 125 KB (was 202) |
| Main thread while it plays | 3.1% (was 3.3) | 12.7% (was 13.1) | 17.9% (was 18.1) |
| Main thread after two turns | 0.6% (was 3.4, it never stopped) | 1.4% (was 12.8) | 2.0% (was 17.6) |
| Frame emptied, off screen | 0.5% | 1.1% | 1.6% |

What is left, for a later pull request: the frame's longest task (about 260 ms at 4x, a second
after the frame is asked for) is mostly the field's shader link read back at once
(`getProgramParameter(LINK_STATUS)` straight after `linkProgram` in `floating/field.ts`, 205 ms
waiting on the GPU process); `KHR_parallel_shader_compile` and a check a frame later would take
it off the main thread. While Ghost plays, the frame still recalculates style about 35 times
and lays out about 32 times a second: Wisp's per-frame SVG attribute writes (about 345 a
second) are geometry, so each frame is a layout; moving Wisp's motion to transforms on groups
would end it. The arrival's blur (`filter: blur()` in the zoom from depth) repaints Wisp for
950 ms each time Ghost comes forward.

### What the sound measured (2026-11-13)

| | WebM/Opus 32 kb/s | AAC 48 kb/s |
|---|---|---|
| Size | 93 KiB | 139 KiB (a visitor loads one) |
| Loudness, true peak | -16.8 LUFS, -1.3 dBTP, nothing clipped | -16.6 LUFS, -1.2 dBTP |
| The person / Ghost | -17.7 / -16.6 LUFS | -17.3 / -16.4 LUFS |
| Parakeet, every line | as written (it writes "4" for "four") | the same |

Played through the desk's speakers from Chrome and recorded across the room on a webcam's
microphone: every line read back as written except once "Hey Ghost" as "Hey goals" (at about
7 dB over the room's floor; alone it read right). The voices are within 0.9 LU of each other
in the room. The chimes stand 17 to 25 dB over the speech band at their own notes. The
thinking loop is faint, as the daemon's mix makes it: its notes are 3 to 5 dB over the room's
floor. The person's voice sits at a median pitch of 121 Hz and Ghost's at 200 Hz. The float's
scenes landed within 26 ms of the sound heard in the room. Lighthouse (mobile / desktop):
performance 95 / 100 against main's 96 / 100 (the same LCP, 2.70 s / 0.56 s), CLS 0.000 / 0
(main 0.007 / 0), nothing of the sound requested. Not tested: Safari and iOS (no Apple device
here). Both carry Opus in WebM on current versions as far as we know, and the AAC file is
there for any that don't.

## Deploying

`.github/workflows/site.yml`: on a push to main that touches the site or what it imports, the
official Astro action builds `site/dist` and uploads it as the Pages artifact, then
`actions/deploy-pages` publishes it. A pull request touching the same paths builds without
deploying; `workflow_dispatch` runs it by hand (it deploys only from main).
`site/public/CNAME` holds `heyghost.app`; Astro's `site` is `https://heyghost.app`, no `base`.

## Turning it on (the owner, once)

`.app` is on the HSTS preload list: browsers only ever ask for `https://heyghost.app`. Don't
announce the address until step 6 shows a certificate.

1. **Verify the domain for the organisation**, so no other account can put a Pages site on it:
   github.com › heka-vault › Settings › Pages › **Add a domain** › `heyghost.app`. GitHub shows a
   TXT record, `_github-pages-challenge-heka-vault.heyghost.app` with a value; add it at the
   registrar, wait for `dig _github-pages-challenge-heka-vault.heyghost.app +short -t TXT` to
   show it, then press **Verify**.
2. **DNS at the registrar** (remove any parking A/AAAA/CNAME records on `@` and `www` first):

   | Type | Name | Value |
   |---|---|---|
   | A | `@` | `185.199.108.153` |
   | A | `@` | `185.199.109.153` |
   | A | `@` | `185.199.110.153` |
   | A | `@` | `185.199.111.153` |
   | AAAA | `@` | `2606:50c0:8000::153` |
   | AAAA | `@` | `2606:50c0:8001::153` |
   | AAAA | `@` | `2606:50c0:8002::153` |
   | AAAA | `@` | `2606:50c0:8003::153` |
   | CNAME | `www` | `heka-vault.github.io` |

   No wildcard records. Check: `dig heyghost.app +noall +answer -t A` (the four addresses),
   `dig heyghost.app +noall +answer -t AAAA` (the four), `dig www.heyghost.app +noall +answer`
   (a CNAME to `heka-vault.github.io.`). Propagation can take up to a day.
3. **Pages from Actions**:
   `gh api -X POST repos/heka-vault/ghost/pages -f build_type=workflow`
   (if Pages already exists: `gh api -X PUT repos/heka-vault/ghost/pages -f build_type=workflow`).
   Check: `gh api repos/heka-vault/ghost/pages --jq '{build_type, status, html_url}'`.
4. **The custom domain**: `gh api -X PUT repos/heka-vault/ghost/pages -f cname=heyghost.app`.
   GitHub then checks DNS and asks Let's Encrypt for a certificate for the apex and `www`.
5. **Deploy**: merge the pull request (or run the workflow from main:
   `gh workflow run site.yml --ref main`), then `gh run watch` on it. The `github-pages`
   environment the first deploy creates accepts main only.
6. **The certificate**: `gh api repos/heka-vault/ghost/pages --jq .https_certificate` until
   `state` is `approved` and `domains` lists `heyghost.app` and `www.heyghost.app` (minutes to
   an hour after DNS resolves). Then
   `gh api -X PUT repos/heka-vault/ghost/pages -F https_enforced=true`. Check:
   `curl -sI https://heyghost.app | head -1` (200) and `curl -sI https://www.heyghost.app | grep -i location`
   (a redirect to the apex).
