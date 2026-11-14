# The site: heyghost.app

`site/` is the public site, a static Astro page served by GitHub Pages at
<https://heyghost.app>. One page, leading with the float; the docs stay on GitHub and the
page links to them.

## What it is made of

- **The hero is the app's own floating room.** `site/src/pages/float.astro` mounts
  `FloatingRoom` from `apps/desktop/src/floating` with the desktop's in-memory mock daemon
  (`apps/desktop/src/design/mock`, the `floating` scenario) over a synthetic desktop
  (`site/src/components/Desktop.astro`). The front page loads that page into a frame only once
  it has loaded and the frame is on screen, empties the frame after two seconds off screen,
  and shows a still instead under reduced motion (with a Play button) or with JavaScript off.
  When the floating room changes, the site changes with it: a pull request touching
  `apps/desktop/src/**` builds the site too.
- **The stills** (`site/src/assets/float-*.webp`, `site/public/og.jpg`,
  `site/public/apple-touch-icon.png`) are taken from that page:
  `pnpm --dir site site:build && pnpm --dir site site:stills` (Chrome or Chromium and
  ImageMagick, like `make screenshots`). The desktop app's screenshots come straight from
  `docs/screenshots/`, the README's.
- **The favicon** is Wisp's glyph from `packages/ui` (`wispGlyph`), rendered at build time
  (`site/src/pages/favicon.svg.ts`).
- **Nothing is fetched from anyone else**: fonts are self-hosted (fontsource), no analytics,
  no cookies. Keep it that way.
- **The copy** makes only the promises the README and CLAUDE.md's rules make. When the product
  changes one, change the page in the same pull request.

`site` is a member of the pnpm workspace so it can import the desktop's sources with one
install. It has no `build`, `test` or `typecheck` script, so the root `pnpm build`,
`pnpm test` and `pnpm typecheck` (and the `ci` workflow) don't run it; its own scripts are
`site:build`, `site:check`, `site:stills` and `dev`. It adds about 250 packages to the lock and
about a second to a cold `pnpm install` (measured 2026-11-13: 3.4 s to 4.4 s with an empty
store, 690 to 844 MB of store).

```bash
pnpm --dir site dev          # http://localhost:4321
pnpm --dir site site:check   # tsc over the site's TypeScript and what it imports
pnpm --dir site site:build   # site/dist, about 1.5 s
```

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
