# Ghost design system

The contract for the desktop app (`apps/desktop`). Every screen follows it. If something you need is missing, ask the foundation owner for a new token or primitive; don't invent one locally.

Live reference: run the dev server and open `/?mock=passive#/gallery`. It shows every primitive in every state, in both themes side by side, and links every route in every mock scenario.

## Principles

1. **Voice first, app second.** Speaking to Ghost is the product. The app shows state, asks for the operator's okay, and lets them configure things. It should never feel like the main event.
2. **One focal point per screen.** Each page has one thing the eye lands on first: the live transcript, the waiting change, the list being edited. Everything else steps back.
3. **Quiet surfaces, one accent.** Neutral grays do the structure. The iris accent (`--accent`) marks the single primary action and the current selection, and nothing else. Semantic colors (ok, warn, danger, info) only mean what they say.
4. **Hairlines, not boxes.** Group with space first, then with one hairline container (`Card`). Never nest bordered boxes inside bordered boxes.
5. **Show state honestly.** Loading, empty, offline, error and in-progress states are designed, not left over. The status dot moves only when something is actually happening.
6. **Plain words.** Sentence case, the operator's own vocabulary, no jargon in primary labels. Technical names (`GHOST_WAKE_PHRASES`, `github_heka.close`) appear only as secondary monospace detail.
7. **Both themes are first-class.** Dark and light follow `prefers-color-scheme`. Check every screen in both.

## Files

```
apps/desktop/src/
  ui/                  foundation: tokens, base, primitives (import from "../../ui")
    tokens.css         every design token; each color once as light-dark(LIGHT, DARK)
    base.css           reset, focus, scrollbars, type and layout utilities
    components.css     primitive styles (g- prefix)
    index.ts           the single export of every primitive and helper
    Choice.tsx         Checkbox, RadioGroup        Menu.tsx   Menu
    Tabs.tsx           Tabs                        VoicePresence.tsx
    SaveBar.tsx        SaveBar                     PendingChange.tsx
  app/                 foundation: shell, router, route registry, connection states
  design/context.tsx   design-mode context (useDesign, useFixture, scenarioHref)
  design/chrome.tsx    dev-only chrome: Mock badge, scenario switcher, the prototype strip (lazy)
  design/mock/         dev-only mock daemon, scenarios and per-owner fixtures
  screens/<owner>/     index.tsx + <owner>.css: the real screens
  prototypes/<owner>/  index.tsx + <owner>.css: prototypes (dev + mock only)
```

### Ownership

| Folder | Owner | Notes |
|---|---|---|
| `src/ui`, `src/app`, `src/design/context.tsx`, `src/design/mock/{daemon,index,scenarios}.ts`, `src/design/mock/fixtures/index.ts`, `src/main.tsx`, `src/screens/gallery` | foundation | Shared. Page owners never edit these. |
| `src/screens/session` | session | `#/` |
| `src/screens/conversations` | conversations | `#/conversations`, `#/conversations/:slug` |
| `src/screens/policy` | policy | `#/policy` |
| `src/screens/audit` | audit | `#/audit` |
| `src/screens/servers` | servers | `#/servers` |
| `src/screens/skills` | skills | `#/skills` |
| `src/screens/departments` | departments | `#/departments`: the list, and one department’s rules, memory, servers, skills, docs, agents and journal (ADR 0024) |
| `src/screens/settings` | settings | `#/settings` |
| `src/prototypes/onboarding` | onboarding | `#/welcome` |
| `src/prototypes/review` | review | `#/review` |
| `src/prototypes/tray` | tray | `#/tray`, rendered without the shell |
| `src/prototypes/mobile` | mobile | `#/mobile`, `#/mobile/:screen`, rendered without the shell |
| `src/prototypes/mascot` | mascot | `#/mascot` (Wisp at every size, and every frame of its motion), `#/mascot/frames` (the frames alone) |
| `src/design/mock/fixtures/<owner>.ts` | that owner | Extend freely; keep the exported shape. |

Inside your folder you may create any files you like. Prefix every CSS class with your folder's short prefix (`session-`, `conv-`, `policy-`, `audit-`, `srv-`, `skl-`, `tk-`, `set-`, `onb-`, `rev-`, `tray-`, `mob-`, `masc-`) so screens can't collide. Never style a `g-` class from a screen stylesheet; compose primitives instead.

## Tokens

All values live in `src/ui/tokens.css`. Use the variables; never write a raw hex value, pixel size outside the scale, duration or shadow in a screen.

Every color is written once as `light-dark(LIGHT, DARK)` on `:root`, which has `color-scheme: light dark`, so the theme follows `prefers-color-scheme` with no duplicated blocks. `data-theme="light"` or `"dark"` on any element forces that subtree (the gallery shows both side by side this way). To add a color token, add one `light-dark()` line. `light-dark()` needs WebKitGTK 2.44 or later (Ubuntu 26.04 ships newer) and Chrome 123 or later.

### Color

| Role | Tokens | Use |
|---|---|---|
| Ground | `--bg-app` | Window ground behind the sidebar and the page panel. |
| Page | `--bg-canvas` | The inset page panel. |
| Surface | `--bg-surface` | Cards, grouped rows, secondary buttons. |
| Raised | `--bg-raised` | Dialogs, toasts, popovers, the composer. |
| Sunken | `--bg-sunken` | Wells: logs, code, event lists. |
| States | `--bg-hover`, `--bg-active`, `--bg-selected` | Translucent overlays for hover, press, current item. Pressed (`--bg-active`) is momentary and shows only while the pointer is down. Selected is persistent and a different kind of signal: `--bg-selected` plus a 2px `--accent` bar on the row's leading edge (`inset 2px 0 0 var(--accent)`), so selected never reads as hovered or pressed, in either theme. |
| Sidebar current | `--bg-nav-selected`, `--shadow-nav-selected` | The sidebar's current row: a canvas tile with a hairline on the gray ground in light (Plain), a lift in dark. |
| Borders | `--border-subtle` (dividers inside a card), `--border` (card edges), `--border-strong` (controls, emphasis), `--border-input`, `--border-input-hover` |
| Control tracks | `--control-track`, `--control-track-hover` | The off switch, the empty slider, checkbox and radio edges. At least 3:1 on every surface. |
| Tooltip | `--tooltip-bg`, `--tooltip-fg`, `--tooltip-border` | Inverse bubble in light; a raised dark surface in dark. |
| Text | `--text-1` primary, `--text-2` secondary, `--text-3` tertiary/meta/placeholder, `--text-disabled` |
| Accent | `--accent` (fills), `--accent-hover`, `--accent-active`, `--accent-text` (accent as text or icon), `--accent-soft` (tinted background), `--accent-border`, `--on-accent` |
| Semantic | `--ok`, `--warn`, `--danger`, `--info`, each with `-soft` and `-border`; `--danger-fill` for the destructive button |
| Phases | `--phase-starting` info blue (never amber: amber means only “waiting for your okay”), `--phase-passive` gray, `--phase-listening` green, `--phase-thinking` violet, `--phase-speaking` sky, `--phase-offline` red |
| Agent hues | `--hue-0-fg/bg` ... `--hue-7-fg/bg` | Agent glyphs only (via `LineGlyph`). Hue 0 is Ghost. |

Measured contrast (WCAG 2.1), on the page canvas:

| Pair | Dark | Light |
|---|---|---|
| `--text-1` | 16.1:1 | 17.7:1 |
| `--text-2` | 7.4:1 (AAA) | 7.7:1 (AAA) |
| `--text-3` | 5.67:1 (AA) | 5.86:1 (AA) |
| `--accent-text` | 7.4:1 | 6.2:1 |
| White on `--accent` (rest / hover) | 4.97 / 5.38 | 5.6 / 5.9 |
| White on `--danger-fill` (rest / hover) | 5.1 / 5.32 | 5.1 / 6.12 |
| `--ok` / `--warn` / `--danger` / `--info` | 10.8 / 11.5 / 6.8 / 7.4 | 5.0 / 5.9 / 5.6 / 5.4 |
| `--text-3` on `--bg-app` (sidebar) | 5.89:1 | 5.19:1 |
| `--text-3` on a hovered sidebar row | 5.49:1 | 4.83:1 |
| `--text-3` on the current sidebar row (`--bg-nav-selected`) | 4.93:1 | 5.86:1 |
| `--text-2` on the current sidebar row (line status uses it there) | 6.49:1 | 7.73:1 |
| `--warn` on a hovered / current sidebar row (“Waiting on your okay”) | 11.1 / 9.97 | 4.85 / 5.88 |
| `--text-3` on a selected list row (`--bg-selected` on canvas) | 4.65:1 | 4.92:1 |

Every tint a row can take (hover, pressed, selected) keeps `--text-3` at AA; that is why light `--text-3` is `#64646d` and light `--warn` is `#935605`, not the lighter values they started as. State steps (not text, so no ratio applies, but they must be told apart): current vs hovered sidebar row 1.11:1 dark plus text and icon going to `--text-1`, 1.21:1 light plus the tile's hairline and shadow; selected vs hovered list row 1.12 / 1.11.

Non-text contrast (WCAG 1.4.11, needs 3:1):

| Indicator | Dark | Light |
|---|---|---|
| Focus outline / focused field border (`--focus-ring-color`) on canvas | 7.5:1 | 5.6:1 |
| Focus outline on `--bg-app` (sidebar) | 7.8:1 | 4.97:1 |
| `--control-track` (switch off, slider empty, checkbox and radio edges) on canvas | 3.22:1 | 3.25:1 |
| `--control-track` on `--bg-surface` (inside a card) | 3.1:1 | 3.14:1 |
| `--border-input` (text field edge) against the canvas / its own fill / inside a card | 3.54 / 3.18 / 3.15 | 3.36 / 3.36 / 3.33 |
| `--border-input-hover` on canvas / against its fill | 4.99 / 4.48 | 4.94 |
| Status dots (`--phase-*`) on `--bg-app` (sidebar), light | (all over 3:1; starting 7.8) | passive 3.99, starting 4.81, listening 4.45, thinking 4.01, speaking 3.11, offline 4.28 |

Text-field edges meet 3:1 in both themes, against the canvas, against the field’s own fill (dark `--bg-input`) and inside a card. Hover darkens the edge and focus moves it to `--focus-ring-color` plus a halo. Fill buttons darken on hover in both themes, so their white label only gains contrast. **The gallery asserts all of this on every load** (its Checks section, one row per theme): it measures the computed field edges at rest and on hover on the canvas and on a surface, and white on `--accent`, `--accent-hover`, `--accent-active`, `--danger-fill` and `--danger-fill-hover`, and logs a console error if any falls below 3:1 or 4.5:1. An empty text field still shows a placeholder with an example value (“https://api.anthropic.com”); `Field` warns in development when it doesn’t.

Body copy uses `--text-1` or `--text-2` (both AAA). `--text-3` is for meta, help and placeholders only, never for anything the operator must read to act. `--text-disabled` is exempt and only for disabled controls.

### Type

One typeface, **Inter** (variable, optical sizes), and one mono, **JetBrains Mono**, both self-hosted through `@fontsource-variable` so the Tauri app works offline. Inter runs with `cv11`, `ss01`, `ss03` (single-storey a, open digits, rounder quotes). Use `.t-num` for tabular numerals in columns and counters.

| Utility | Size / line | Weight | Use |
|---|---|---|---|
| `.t-display` | 32 / 40 | 600 | Rare: a hero number or the session's focal line. |
| `.t-title-lg` | 24 / 32 | 600 | Page titles (`PageHeader` uses it). |
| `.t-title` | 20 / 28 | 600 | Dialog titles, large empty states. |
| `.t-heading` | 16 / 24 | 600 | Card and pane headings. |
| `.t-subheading` | 14 / 22 | 600 | Section titles (`Section` uses it). |
| `.t-body-lg` | 14 / 22 | 400 | Reading text: transcripts, summaries, descriptions under page titles. |
| `.t-body` | 13 / 20 | 400 | The UI default: labels, rows, controls. |
| `.t-small` | 12 / 16 | 400 | Help text, secondary detail. |
| `.t-caption` | 11 / 16 | 400 | Timestamps and meta. |
| `.t-overline` | 11 / 16 | 500, caps | Column labels inside a card. Use sparingly. |
| `.t-mono` | 12 / 16 | 400 | Identifiers: env names, operation names, paths, issue refs. |

Weights are 400, 500 and 600 only. Tracking tightens automatically at 16px and up via the utilities. Color utilities: `.t-1`, `.t-2`, `.t-3`, `.t-accent`, `.t-ok`, `.t-warn`, `.t-danger`.

### Space

A 4px grid: `--space-1` 4, `-2` 8, `-3` 12, `-4` 16, `-5` 20, `-6` 24, `-8` 32, `-10` 40, `-12` 48, `-16` 64, `-20` 80. Half steps `--space-0_5` (2), `--space-1_5` (6) and `--space-2_5` (10) exist for control internals only.

Rhythm: 8 between a label and its control, 16 between fields, 24 between groups inside a card, 40 between page sections (`Section + Section` does this), 32 below the page header.

### Size, radius, elevation, motion, layers

- Controls: `--control-xs` 24 (inline dismiss buttons), `--control-sm` 28, `--control-md` 32 (default), `--control-lg` 40. Icons: `--icon-xs` 12 (badges), `--icon-sm` 14, `--icon-md` 16 (default), `--icon-lg` 20.
- Shell: `--sidebar-width` 244, `--nav-item-height` 28 (Linear density), `--nav-item-height-2line` 40 (line rows with a status line), `--count-size` 18 (count pills, small badges, key caps), `--panel-inset` 8 (gap around the page panel), `--field-control-width` 320 (the control column of inline fields), `--save-bar-min` 480 (the SaveBar’s narrowest width), `--stop-pitch` 104 (one column per autonomy level in a row of stops), `--touch-target` 56 (the phone’s primary buttons).
- `--page-sticky-top`: set by the shell on `#main`, the bottom of the sticky stack (connection banners plus the compact page bar) from the top of the panel, kept current as banners come and go. Anything a screen pins under the bar (`position: sticky`, `scroll-margin-top`) uses `var(--page-sticky-top, var(--page-bar-height))`; never measure `.g-page-bar` yourself.
- Opacity: `--opacity-disabled` 0.45, the one step every disabled control dims to (filled buttons go neutral instead).
- Radius: `--radius-xs` 4 (kbd, skeleton), `-sm` 6 (small controls, nav items), `-md` 8 (controls, notices), `-lg` 12 (cards, the page panel), `-xl` 16 (dialogs, bubbles, composer), `-full` (pills, dots).
- Elevation: dark mode leans on surface steps and borders; shadows are for things that float. `--shadow-xs` (buttons), `-sm` (the page panel, the selected segment), `-md` (toasts, tooltips, the composer), `-lg` (dialogs).
- Motion: `--duration-instant` 80, `-fast` 120 (hover, press), `-base` 180 (toggles, dialogs), `-slow` 280 (toasts), `-slower` 420. Easings `--ease-out` (default), `--ease-in-out` (loops), `--ease-emphasized` (things arriving). `prefers-reduced-motion` zeroes every duration and stops every loop except spinners.
- Layers: `--z-sticky` 10, `-sidebar` 20, `-banner` 30, `-popover` 40, `-dialog` 50, `-toast` 60, `-tooltip` 70.
- Page widths: `--page-narrow` 680 (forms, settings), `--page-default` 880, `--page-wide` 1160 (tables, two panes).

### Focus

One treatment, applied in `base.css` to every `:focus-visible`: `outline: var(--focus-outline)` (2px solid `--focus-ring-color`) with `outline-offset: var(--focus-offset)` (2px). The offset is transparent, so it reads the same on the sidebar, the canvas and inside cards; the outline follows `border-radius` and survives forced-colors mode. Mouse clicks never show it.

- Text fields, selects and textareas: the border turns `--focus-ring-color` and `--focus-field` adds a 1px ring plus a soft halo (invalid: `--focus-field-danger`). Fields show it on any focus, mouse or keyboard.
- Inside lists, tables, segmented controls and menus, where an outside ring would be clipped: `box-shadow: var(--focus-ring-inset)`.
- A scroller that holds focusable items needs 4px of padding on every side (offset plus ring), or the ring is clipped.
- Buttons in a row sit at least 12px apart (`--space-3`) wherever a focused button can sit next to a filled one: dialog footers, page header actions, card footers. At 8px the 4px ring crowds its neighbour.
- Never remove a focus style without replacing it, and never style focus with a tint alone: the indicator must reach 3:1 (see the table above).

## Primitives

Import from `src/ui` only: `import { Button, Card, Page } from "../../ui";`. Each is keyboard-complete and labeled for assistive tech.

| Primitive | Props that matter | Rules |
|---|---|---|
| `Page` | `title`, `description`, `actions`, `width` (`narrow`/`default`/`wide`/`full`), `leading`, `crumb`, `eyebrow`, `status`, `bare`, `stickyHeader`, `dock`, `dockFirst` | Every screen’s root. `crumb` is the way back (“‹ Conversations”), first in the eyebrow, before `status`; `eyebrow` comes after it. `dockFirst` puts the dock before the content in Tab order (it still sits at the bottom): a save bar on a long form is one Tab from the header, not one past every field. One `h1` (with `bare` it is visually hidden, never missing). `status` is the page’s live state; for Ghost’s state always pass `status={<LiveStatus />}`, never a hand-rolled `phaseText(phase)`: it says exactly what the sidebar’s status card says (“Connecting…”, “Reconnecting”, “Not running”, “Listening”), with the same dot and tone. It leads the eyebrow and repeats right after the title in the compact bar, at 12px in `--text-2`, so a screen never loses its own state when the header scrolls away. The status always starts with its dot, so its leading edge never moves between phases; the dot holds still while a shell banner above it is already moving (models loading, reconnecting). The bar also repeats `leading` at 20px (a `LineGlyph` shrinks itself through `GlyphSizeContext`). A direct child of the content with `className="page-fill"` takes the rest of the page’s height above the dock, for a start state centered in its space (the session before its first turn). Every width shares one column (centered, up to 1160) and one left edge, so the title never moves between routes; `narrow` (680) and `default` (880) cap the header, content and dock inside it, left-aligned, so actions sit over the content they act on. `full` has no column and no padding (bare canvases: tray, mobile). `bare` hides the header but keeps the document title as a visually hidden `h1`. `dock` pins its content to the bottom of the page panel while the page scrolls under it, fading the content out above it: the session composer, a save bar. One dock per page. The compact bar takes the title as soon as the big title has gone under it (not the whole header), so a page that overflows by less than its description never ends its scroll with an empty bar. |
| `PageHeader` | `title`, `description`, `actions`, `eyebrow`, `leading`, `headingLevel` | Used by `Page`. Use directly only inside `bare`/`full` layouts. `headingLevel="p"` for specimens, so a page keeps one h1. |
| `Section` | `title`, `description`, `actions`, `level` | Titled block inside a page. Sections are separated by space, not lines. |
| `Card` | `padding` (`none`/`sm`/`md`/`lg`), `list`, `tone` (`sunken`/`accent`/`warn`/`danger`) | One container per group. `list` gives dividers between children and no padding: put `ListRow`s or padded divs in it. Tinted tones once per page at most. |
| `ListRow` | `title`, `description`, `leading`, `trailing`, `meta`, `onClick`/`href`, `selected`, `chevron`, `density`, `align` (`center`/`start`), roving props (`tabIndex`, `role`, `aria-selected`, `onKeyDown`, `hitRef`, `id`) | Settings rows, lists of lines, servers. A clickable row and its `trailing` control are siblings, so a switch or button in a row never triggers the row. The roving props go on the hit area, for a listbox of rows with one Tab stop (the review queue); `align="start"` top-aligns the leading and trailing parts with a multi-line title. |
| `Table` | `density`, `stickyHeader`, `interactive` | Plain `thead/tbody/tr/th/td`. Cell classes: `is-num` (right-aligned tabular), `is-muted`, `is-shrink` (fit content, no wrap), `is-nowrap` (never break: issue refs, times), `is-mono` (identifiers: operation names, `ghost-mock/app#151`). Identifiers always get `is-nowrap`; a hyphen is not a line break. Expandable rows: `tabIndex={0}`, `aria-expanded`, Enter/Space toggles. |
| `Button` | `variant` (`primary`/`secondary`/`ghost`/`danger`/`danger-ghost`), `size`, `loading`, `leadingIcon`, `trailingIcon` | At most one `primary` per view. `danger` only as the confirm button of a destructive dialog; `danger-ghost` for the entry point to a destructive action. Labels are verbs: "Save", "Delete Legal", never "OK" or "Submit". `loading` keeps the width (asserted in the gallery) and blocks double submits: the spinner takes the leading icon’s box, or sits centered over the hidden label when there is no leading icon. A loading button is `aria-disabled`, not `disabled`, so keyboard focus stays on it (a disabled button drops focus to the page body); clicks are ignored while it loads. Never change the label while loading (“Save” stays “Save”); announce progress with a Toast, a Notice or an `aria-live` line. `href` (+ `target`, `rel`) renders an `<a>` with the same look and states: “Open on GitHub”, the 404 page’s “Go to the session”. Never hand-write `g-btn` classes. |
| `IconButton` | `label` (required), `icon`, `variant`, `size`, `pressed` | The label is the accessible name and the tooltip. Use for toolbar actions only; anything important gets a text button. |
| `Field` | `label`, `help`, `error`, `detail`, `aside`, `optional`, `layout` (`stack`/`inline`), `controlWidth` | Wraps one control and wires `id`, `aria-describedby` and `aria-invalid` automatically. `detail` is where env names go (`detail="GHOST_WAKE_PHRASES"`). `inline` is a two-column grid, label and help left, control right, so controls line up down a `Card list` whatever the label length. `controlWidth` (default 320) sets the control column; use `"auto"` for a Switch or a small button. An inline Field is its own size container: narrower than 560px (a card in a two-pane layout, a half column) it falls back to the stack, except with `controlWidth="auto"`, which stays on the right. An empty text control must have a placeholder showing an example value (see the contrast exception; development builds warn). Put secrets in a `<form>` with one password field each, so Enter saves and Chrome doesn’t warn. Errors say what’s wrong and how to fix it. |
| `Input` | `size`, `leading`, `trailing`, `mono`, `invalid` | Placeholders are examples, never labels. Secrets use `type="password"`, `autoComplete="off"`. |
| `Textarea` | `rows`, `mono` | Resizes vertically. Use `mono` for TOML or code. |
| `Select` | `options` or `<option>` children, `size` | A styled native select: right for WebKitGTK and screen readers. Prefer `SegmentedControl` for 2 to 4 visible options. |
| `Switch` | `checked`, `onChange`, `label` or `aria-label` | Takes effect immediately. If a change needs Save, use `Checkbox` instead. |
| `Checkbox` | `checked`, `onChange`, `indeterminate`, `label`, `description`, `disabled` | Native input. Choices that apply on Save, and multi-select lists (the repositories a line may touch). `indeterminate` for a parent of a partly checked group. |
| `RadioGroup` | `options` (`value`, `label`, `description`, `icon`, `aside`, `disabled`), `value`, `onChange`, `label` or `aria-label`, `variant` (`list`/`card`), `columns` | One choice from 2 to 6 options that need words: the autonomy levels, onboarding steps. Native radios: Tab enters at the selection, arrows move and select. `card` is a bordered option with title and description; selected is `--accent-border` on `--accent-soft`. With an `icon`, the card leads with the icon and the radio moves to the top-right, so each option shows one leading marker (Klaviyo). Prefer `SegmentedControl` when the labels are one or two words. |
| `Menu` | `trigger` (a Button or IconButton), `items` (`{ label, onSelect, icon, shortcut, danger, disabled, reason }`, `"separator"`, `{ heading }`), `align` | Actions on one object: a row’s overflow (Rename, Reset, Delete). The trigger is an `IconButton` with `MoreHorizontal` and a label naming the object (“More for Accounting”). Opens on click, Enter, Space or ArrowDown (ArrowUp opens on the last item), always with focus on an item; the trigger handles Enter and Space itself so the browser never synthesizes a click after focus has moved. Arrow keys, Home/End, typeahead, Enter; Escape returns focus to the trigger. The gallery asserts the Enter path on every load. A `shortcut` shows as plain muted text (“Ctrl P”), not key caps. A disabled item’s `reason` (“Ghost is on this line”) shows as one quiet line under its label and describes it for assistive tech; never a heading over disabled items. A `danger` item opens a ConfirmDialog; it never deletes directly. Not for navigation or picking a value. |
| `Tabs` | `items` (`value`, `label`, `icon`, `count`, `disabled`), `value`, `onChange`, `aria-label`, `size`, `children` (the selected panel) | Views of one object. The line page no longer uses them: the conversation is the page, and its settings (Voice and persona, Memory, History) open from the More menu in its place with a “← Conversation” way back (the operator’s call, 2026-10-20). Roving tabindex, arrows and Home/End. Not for page navigation (routes) or settings values (SegmentedControl). |
| `VoicePresence` | `phase`, `size` (48/96/160, and 320 on the phone), `level` (0 to 1), `label` | The one shared visual for Ghost’s voice state: session, tray and phone all use it, never a homemade orb. Still for passive and offline; one quiet ring for listening (or a ring that swells with `level` when you have a live mic level), a slow breath for thinking, bars for speaking (driven by `level` if given), a turning arc while starting. Frozen at rest under reduced motion. 48 in the tray and compact headers, 96 in the session and phone, 160 only for a hero moment. On the Android app the hero is 320 and has no bubble, rings or bars: it is Wisp alone (`Mascot`, below) in the phase’s color over a soft glow, and Wisp acts out the phase (wide-eyed listening, thought dots, a talking mouth, eyes shut with a rising “z” when checked out, waking while starting, slumped when offline). On Home the 320 hero is the Start control itself, the screen’s main control: `float` drifts Wisp up and down (on the native driver) and it blinks now and then, so it reads as something to tap, and it waves when a tap starts a session, so the wait for the desk has something to watch. The session screen uses the same hero. Under it, Home shows the other lines as a dial pad: three square keys to a row, each the ghost glyph in the line’s hue (no tile box; the colour is the identity) over the name, big enough for a thumb; a press calls that line directly and the session opens in it. Activity rows lead with the same glyph in the colour of the line the change was made in, and name the line in the support line. The ghost glyph everywhere is Wisp, the mascot, drawn still: at 40px and up it is Wisp saying hello (grinning, mid-wave), below that the plain body; every line’s tile is the ghost in the line’s colour, never an initial. The glyph is 30% of the size (14, 29, 48px); 48 uses the simplified small drawing. Pair it with `phaseText(phase)` nearby. |
| `SegmentedControl` | `options`, `value`, `onChange`, `aria-label`, `size`, `fullWidth` | Radiogroup with arrow-key navigation. Two to five options. Without a `size` it follows `ButtonSizeContext`, so in the compact bar it shrinks to `sm` with the buttons beside it. Works as a `Field`’s control: the Field’s label names it and its help describes it. |
| `Slider` | `value`, `onChange`, `min`, `max`, `step`, `format`, `readoutWidth` | Pass `format` to show a value readout (`v => \`${v.toFixed(2)}×\``); `readoutWidth` fixes the readout’s width so the track never moves as the words change (“Strict” to “Forgiving”). |
| `Badge` | `tone`, `variant` (`soft`/`outline`), `size`, `dot`, `icon`, `mono` | Status and short tags. `dot` for states (Set, Waiting, Failed). Always sentence case. A change’s status from the API goes through `statusLabel()` and `statusTone()` (executed is “Done” in `ok`, pending “Waiting” in `warn`, failed “Failed” in `danger`, expired and canceled neutral, stale “Changed first”); never show the raw value in a badge. |
| `StatusDot` | `phase` (`starting`/`passive`/`listening`/`thinking`/`speaking`/`offline`), `size`, `label`, `still` | The voice loop's state. Moves only for starting, listening, thinking, speaking; `still` holds it when something nearby already shows the motion. Pair with words or give it a `label`. For a page’s status use `LiveStatus`, which picks the dot for you. |
| `Notice` | `tone`, `title`, `action`, `onDismiss` | Inline, persistent results and warnings inside the page. Errors that block work go here, not in a toast. |
| `useToast()` | `toast({ title, description, tone, action, duration })` | Transient confirmation of something that just happened, with an optional action (Undo after a save). Never the only place an error appears. |
| `Dialog` | `open`, `onClose`, `title`, `description`, `footer`, `size`, `dismissible`, `initialFocus` (a selector or ref) | Native `<dialog>`: inert background, Escape, focus returned to the opener. The browser lets Tab leave a modal for its own chrome, so `Dialog` wraps Tab and Shift+Tab between its first and last control itself; focus never leaves it. Footer buttons sit 12px apart. Short focused tasks only. |
| `PendingChange` | `summary`, `rationale`, `operation` (mono detail), `expiresAt` (`pending.expires_at`), `onConfirm`, `onCancel`, `busy` (`true`, `"confirm"`, `"cancel"`), `compact`, `touch`, `now` (gallery only) | The inline “Waiting for your okay” card: Ghost’s most consequential control, the one component for session, tray, mobile and review. An opaque warn card: a 12px overline (“Waiting for your okay · 0:42”, the operation right-aligned in mono), the summary at 14/22 in `--text-1`, the rationale in `--text-2`, then Confirm (primary) and Cancel (ghost) 12px apart, Confirm first. The clock re-reads `expiresAt` every second; it is `aria-live="off"`, with one announcement at 10 seconds and one at expiry. Under 10 seconds the clock turns `--danger`. At 0 it reads “Expired · Ghost left it alone” and both buttons are disabled until the daemon clears `pending`. **`onConfirm` must call `client.confirm()` and nothing else** (the click is confirmation evidence, ADR 0013); `onCancel` calls `client.cancel()`. Place it where it can’t scroll away from its own summary: in a `Page` `dock` above a composer, or at the top of a popover. In development it checks on load that if its Confirm is on screen its summary is too (not under the compact bar, the dock or the panel edge) and logs a console error otherwise. `disabled` turns both buttons off with no spinner while Ghost isn’t answering (`!useLive().online`). `compact` (tray): 12px padding, 13px summary, the operation on its own line under the summary (whole, never truncated), two-line rationale, small buttons 12px apart. `touch` (the phone prototype; the Android app answers by voice and shows the question without this card): 16/24 summary clamped to four lines, the operation on its own mono line, two-line rationale, then Confirm (primary) and Cancel (ghost) side by side as two 56px pills 12px apart; Cancel stays unbordered so “leave it” never looks like the bordered “hang up”. The clock, the two announcements and the expiry behave the same in every variant; never reimplement the card for a surface. |
| `SaveBar` | `variant` (`dirty`/`error`/`offline`/`restart`), `title`, `detail`, `description`, `hover`, `command`, `onSave`, `onDiscard`, `onDismiss`, `busy`, `actions`, `notice`, `saveRef`, `copyRef` | The one control that commits a change, in the `Page` dock: a 44px pill, centered, at least 480 wide (`--save-bar-min`) and growing with its words up to the page width. A dot and one line (`title` in `--text-1`, a middle dot, `detail` lowercase), the `Kbd mod+s` cap while Save can run, then ghost Discard and filled Save 12px apart. The dirty dot is neutral (amber means only “waiting for your okay”), `error` turns the edge and dot `--danger` and puts `description` on a second line, `offline` turns Save off and says the changes wait here, `restart` leads with the one thing left to do, shows `command` in the mono slot with a Copy button, and a Dismiss. The screen binds Ctrl S itself and gates Save on `useLive().online`. What Ghost may do, Settings and a line’s page all use it; never build a local one. |
| `ConfirmDialog` / `useConfirm()` | `title`, `description`, `confirmLabel`, `tone` | Replaces `window.confirm`. Title asks the question with the object’s name (“Delete Legal?”); the confirm label repeats the action (“Delete Legal”). With `tone="danger"`, Cancel takes first focus, so Enter never deletes by accident. |
| `EmptyState` | `icon`, `title`, `description`, `actions`, `size` (`compact`/`default`/`page`), `headingLevel` | Says what will appear here and how to make it appear. Never "No data". `size="page"` outside a `Page` replaces the whole screen (Offline, Not found, a crashed route), so its title is the screen’s `h1`; inside a `Page` it is an `h2`; otherwise a `p`. Override with `headingLevel`. |
| `Skeleton`, `SkeletonText` | `width`, `height`, `radius`, `lines` | Shape-matched placeholders for the first load. Never a spinner for a page. |
| `Spinner` | `size`, `label` | Inline, for an action in progress (buttons use it via `loading`). |
| `Progress` | `value` or none for indeterminate, `label`, `tone` (`accent` default, `starting`, `ok`, `warn`, `danger`, `info`, `neutral`) | Model loading, uploads. The shell’s models-loading banner uses `tone="starting"`. |
| `Tooltip` | `content`, `side`, `shortcut` (`"mod+z"`, shown as Ctrl Z), `asChild` | Short labels for icons and truncated values. Never essential information or actions. The description is set on the trigger element. `asChild` attaches to the child itself instead of wrapping it in a span, for a truncating label whose flex sizing a wrapper would break; the child must take a ref. |
| `Kbd` | children (one key) or `shortcut` (`"mod+shift+z"`) | Keyboard keys, in prose and tooltips. `shortcut` renders one cap per key in Linux names: Ctrl, Alt, Shift, Super. Never ⌘. A whole shortcut always goes through `shortcut` (`<Kbd shortcut="super+g" />`), never as children (`<Kbd>Super+G</Kbd>` is one cap with a plus in it). Menus show shortcuts as text, not caps. |
| `Disclosure` | `summary`, `defaultOpen` | Hide advanced or diagnostic detail (device lists, raw JSON). |
| `GhostMark`, `GhostGlyph` | `size`; the mark also takes `hovered`, `trigger` | The brand: Wisp at rest. The mark is the accent tile; the glyph is the bare silhouette. Give the mark `hovered` and it is the live mascot (the sidebar brand does this). |
| `Mascot` | `size`, `hovered`, `trigger`, `mood`, `idle`, `still`, `title` | Wisp, Ghost’s mascot, alive: on hover (or when `trigger` is bumped, as a tap) it bounces up, grins and waves, then follows the pointer with a small smile; on leave it settles back to exactly the brand glyph. Under 24px it draws the simple body, from 40px it adds the arms. The parent owns `hovered`, as with a card. `mood` is a voice phase (`listening`, `thinking`, `speaking`, `hold`, `starting`, `offline`, `passive`): Wisp loops that phase’s pose between gestures and blends from one to the next. `idle` (a slow breath, blinks) is for hero placements only. On the phone a mood alone redraws at 30 frames a second, a gesture at full rate. Reduced motion shows the smiling face on hover without moving to it. The character and its motion are plain TypeScript in `@ghost/ui` (`wisp.ts`, `mascot.ts`), shared with the phone. Every frame: `#/mascot`. |
| `LineGlyph` | `slug`, `name`, `size`, `hue` | A conversation's tile: the ghost for Ghost, the initial for every other line, on a stable hue from the slug. |

Live state, from `src/ui`, provided once by the shell: `useLive()` returns `connection` (`connecting`/`online`/`reconnecting`/`offline`), `phase`, `online` (gate every write on it), `loadingModels`, `setupNeeded` and `setupHref`. `<LiveStatus />` is the page status (dot and words, from `liveLabel(connection, phase)`, the same mapping the sidebar card uses). `useSetupPromptHere(active)` tells the shell a screen is showing the setup prompt as its own focal point, so the banner steps aside. Never derive connection state from `connected` yourself: `phaseOf(status, connected)` says “Not running” during a reconnect.

Helpers, also from `src/ui`: `cx()` (class names), `statusLabel(status)` and `statusTone(status)` (a change’s status in words and its badge tone), `phaseOf(status, connected)` and `phaseText(phase)` (the voice phase alone, for VoicePresence and specimens; pages use `LiveStatus`), `lineName(slug, name?)`, `relativeTime(iso)` (“now”, “4 min ago”, “3 h ago”, “yesterday”, “5 days ago”, “Oct 9”; the only relative-time formatter, never Intl’s “4 min. ago”), `formatTime(iso)`, `formatDateTime(iso)`, `errorText(e)`, `formatShortcut("mod+z")` (“Ctrl Z”), `shortcutKeys()`, `quoted("Hey Ghost")` (“Hey Ghost”). `scrollIntoPanel(el, { block })` scrolls the page panel without moving Chrome’s Tab starting point; `block: "panel-end"` goes to the very end, past a dock (a transcript under its composer). `scrollPanelToTop()` scrolls it to the top (a new wizard step), so no screen needs a marker element.

### Forced states

`.is-hover`, `.is-active` (pressed) and `.is-focus` force a state on `g-btn`, `g-icon-btn`, `g-input`, `g-textarea`, `g-row`, `g-switch`, `g-check`, `g-tabs__tab` and `g-seg__item` (and `.is-hover`/`.is-focus` on a `RadioGroup`). They exist for the gallery and for screenshots of a state; never use them to style real behavior. Shared state comes from `useGhost()` in `@ghost/ui` (status, connected, turns, pending, logs, lastEvent, conversationsVersion, client).

## Page layout

- The sidebar shows exactly one current row (`aria-current="page"` and the `--bg-nav-selected` tile). On `#/agents/:slug` that is the agent’s own row. The status card owns the phase dot; a live line row says “Live · listening” in words and adds a dot only for “Waiting on your okay”, where the amber adds information.
- The sidebar is the status card, then the agents, then Settings (the operator’s call, 2026-10-24). The agents list reads Ghost, then the agent Ghost is on now (the one holding any pending okay), then the rest by last use, then “New agent…” and the retired count. Agents sit under their department’s own agent (ADR 0024), one glyph column in (`sidebar__item--topic`); a group takes the place of its first member, so a live agent still pulls its department up. The pinned Settings row heads a folded group of the other screens (Session, What Ghost may do, Audit log, Servers, Skills, Departments), one glyph column in (`sidebar__item--sub`): the row navigates, the chevron at its end (`sidebar__fold`) opens the group, landing on one of its screens opens it, and while it is folded a waiting okay shows its count on the Settings row instead of the Session row. The nav between the status card and the pinned Settings row scrolls when it must, with 4px of focus room and edge fades; the Prototypes group folds the same way and starts collapsed on a window under 840px tall. Nav icons (16px) and agent glyphs (20px) share one center line, and every label starts on one text column 34px in.
- The shell owns the sidebar and the inset page panel. **The panel is the scroller, not the window**: its inset, border and radius never move (Linear). `position: sticky` inside a screen sticks to the panel. Screens never set `position: fixed`, global backgrounds or `100vh` heights, and never call `window.scrollTo`. To scroll programmatically use `scrollIntoPanel(el, { block })` from `src/ui` or `document.getElementById("main").scrollTo()`, not `element.scrollIntoView()`: Chrome moves its Tab starting point to whatever scrollIntoView reveals, so the next Tab skips “Skip to content” and the sidebar. scrollIntoView is fine only in response to a click that should also move Tab order (a table of contents).
- Anything that must stay in reach while the page scrolls (a composer, a save bar) goes in `Page`’s `dock`, never in a homemade sticky footer.
- **The top edge.** Content never cuts hard at the panel’s rounded top. Every `Page` with a header that isn’t `full` gets a compact sticky bar for free (`stickyHeader`, on by default): as soon as the panel scrolls, a 48px strip of opaque `--bg-canvas` (scrolled content never shows through, not even a large title on a contrasting surface) sits at the top of the panel; once the big header has gone under it, the bar shows the header’s `leading` glyph at 20px, the title (14px semibold), the page’s `status` right after it, and the header’s `actions` at size `sm`, over a `--border-subtle` hairline. It sits under any connection banner. `full` and `bare` pages, and `stickyHeader={false}`, get a 24px fade at the top edge instead. Screens never build their own sticky header; the gallery shows the bar under Surfaces, and `--scroll 400` shows it live. **Header actions exist exactly once.** `Page` renders them into one host element and moves that element between the header and the bar; nothing remounts, so a Menu, a `useConfirm` button or an open dialog survives the move, ids stay unique, and keyboard and screen-reader users meet each action once. The scrolled-away header keeps the space so nothing reflows. Put anything you like in `actions`.
- Every screen's root is `<Page>`. Pick the narrowest width that fits: forms and settings `narrow`, single lists and policy `default`, tables, two-pane layouts and the session transcript `wide`, bare canvases `full`. All of them start at the same left edge (41px into the panel at 1440 and 1280), so the title holds still as you move between routes (Linear, Apple System Settings). Inside `wide`, keep reading measures in check yourself: the session caps bubbles at `min(76%, 720px)`.
- Shell banners (reconnecting, models loading, not set up) share the page column’s edges: their text starts on the title’s left edge at every width.
- Header: title (what this is), one or two plain sentences (what it's for), actions on the right with the primary action last. 32px below the header to the content.
- Content: group with `Section`s 40px apart; one `Card` per group; rows inside cards. Two-pane layouts put the list left (260 to 320px, sticky) and the detail right.
- The window is at least 1280x800; design at 1440x900. Nothing may scroll horizontally at 1280.

## Copy

- Sentence case everywhere: titles, buttons, labels, badges. “What Ghost may do”, not “What Ghost May Do”.
- Use a real ellipsis, … (U+2026), never three periods, in copy, placeholders and labels: “Connecting…”, “Search issues…”, “Replace…”. The gallery’s typography check flags `...` in text and in `placeholder`, `aria-label` and `title`.
- Typographer’s quotes and apostrophes in every UI string: “Hey Ghost”, “never mind”, “can’t”. Never straight `"` or `'` in visible text. Use `quoted("…")` when quoting what the operator says. The gallery asserts this on every load for its own copy (outside code, mono and `[data-user-content]`); mark nodes that render operator or issue text with `data-user-content` so the same check can run on your screen.
- US English, for an operator in Detroit: color, labeled, center, behavior, summarize, canceled in prose (the API’s `cancelled` status stays as the API spells it, shown in mono).
- Shortcuts read Ctrl, Alt, Shift and Super, never ⌘: write them with `Kbd shortcut="mod+z"` or `formatShortcut()`.
- Say “Ghost”, not “ghostd”, in primary copy: “Restart Ghost to use the new microphone.”, with `systemctl --user restart ghostd` as monospace detail underneath.
- The operator's words. "Ask me first", "Do it and tell me", "Do it quietly", "Not allowed" for autonomy (use `autonomyLabel()` from `@ghost/ui`). "Line" for a conversation in speech ("the Accounting line"), "Conversations" for the screen.
- Name things by what they are to the operator, not to the code: "GitHub token", with `GITHUB_TOKEN` as `detail`. Operation names (`github_heka.close`) appear in mono as secondary detail.
- Buttons are verbs. Confirm labels repeat the action and name the object.
- Notice and toast titles never end with a period (“Saved”, “Couldn’t save”); the body is a sentence and does.
- After a save, one toast everywhere: title “Saved”, body says when it takes effect (“Ghost uses the new levels from its next change.”, “Live now: wake phrases.”), `Undo` where possible. “Applied” is implied by any save that needs no restart. The `SaveBar` restart variant is the only exception: it toasts nothing, the bar says what is left to do.
- One fact, one wording: the nav label and the page title match (“What Ghost may do”), and a line never used reads “Not used yet” everywhere.
- Errors: what happened, then what to do. “Ghost didn’t start. Run `systemctl --user status ghostd` to see why.” Never blame the operator, never show a stack trace as the message.
- Empty states: what will be here and how it gets here. “When Ghost changes something, or asks to, it shows up here.” Don’t promise what can’t happen yet: while models load, say so instead of “Say Hey Ghost”.
- After a middle dot, lowercase: “Live · listening”, “Live · reconnecting”, “Kokoro cached · speech-to-text parakeet”. The part before the dot is the label and keeps its capital; what follows reads as the rest of the sentence.
- Contractions are fine (“isn’t”, “can’t”). No exclamation marks. No “please”, no “successfully”.
- Secrets are write-only: the UI says “Set” or “Not set” and offers “Replace…”. Never echo, mask-preview or log a value.

## Iconography

- `lucide-react` only. Default size 16 (`--icon-md`), 14 inside small buttons and badges, 20 for empty-state and large icon buttons. Stroke width is set globally to 1.75; don't override it.
- Icons support a label; they rarely replace one. Icon-only buttons need `IconButton` with a `label`.
- One icon per concept across the app: Session `AudioLines`, Agents `MessagesSquare`, Departments `Building2`, Policy `ShieldCheck`, Audit `ScrollText`, Servers `Server`, Skills `BookOpen`, Settings `Settings2`, Open folder `FolderOpen`, Call an agent `Phone`, Hang up `PhoneOff`, Send `ArrowUp`, Offline `Power`, Hears idle audio `Ear`, More (overflow menu) `MoreHorizontal`, Rename `Pencil`, Pin a note `Pin`, Delete `Trash2`, Not found `SearchX`, Nothing waiting on you (inbox clear) `Inbox`. Search `Search` means only search; never use it for an empty state. Autonomy levels: Ask me first `Hand`, Do it and tell me `Megaphone`, Do it quietly `VolumeX`, Not allowed `ShieldOff`.
- The brand glyph is Wisp at rest (`GHOST_PATH` is built from `wisp.ts`, so the still mark and the live mascot can't drift apart). It has two drawings on the same 24 grid: under 24px `GhostGlyph` switches to larger eyes so it still reads as a ghost at 12 to 16px. The app icons (desktop window and tray, phone launcher, adaptive layers, splash) show Wisp smiling and waving, rendered from the mascot's own drawing; the favicons, at 16 to 48px, are the plain glyph.

## Motion

Motion explains a change; it never decorates. `VoicePresence` and `StatusDot` are the only things that loop for the voice state. The one character motion is the mascot's: it plays when you reach for it (hover on desktop, a tap on the phone) and settles back to rest, and it loops only where it is the voice state's hero (the phone's 320 presence acts out each phase, which is real ongoing work, and blinks on the Start control). Hover and press 120ms, toggles and dialogs 180ms, toasts 280ms, all `--ease-out` or `--ease-emphasized`. Loops are reserved for real ongoing work: the starting, listening, thinking and speaking dots, spinners, skeleton shimmer, indeterminate progress. With reduced motion every loop stops except spinners (slowed) and indeterminate progress, which stops traveling and instead pulses the full-width bar slowly in place (never a static partial bar that reads as “35% done”); transitions are instant.

## State patterns

Every screen designs all of these. Check them with the mock scenarios.

| State | Pattern |
|---|---|
| First load | `Skeleton`s shaped like the content. The shell shows a skeleton page while a lazy screen loads and during the first connection attempt. A `chrome: "bare"` route (tray, phone) gets a centered bare placeholder instead, never the desktop page’s shape, and renders itself during the first attempt with `connection="connecting"` from `useLive()`, so its own Connecting layout is what the phone shows. |
| Empty | `EmptyState` inside the card or page where the content would be (`?mock=empty`). |
| Loading an action | The button's `loading`; keep the rest of the page usable. |
| Result | `Notice` (persistent, inline) or `useToast` (transient, with Undo after a save where possible). |
| Error | `Notice tone="danger"` near what failed, with `errorText(e)`. A screen that throws is caught by its route's error boundary; the rest of the app keeps working. |
| Connecting | The first attempt, before the shell decides (about 900ms). The sidebar’s status card says “Connecting…” / “Looking for Ghost” with a still, neutral dot, and the page shows its skeleton. It never says “Not running” until the Offline page appears, at the same moment. See it with `?mock=passive&connect=600` (shoot with `--wait 300`); `&connect=1500` shows Connecting, then Offline at 900ms, then the screen once it connects. |
| Daemon not running | Handled by the shell for every route with `needsDaemon: true`: the "Ghost isn't running" page with a Start button in Tauri or the systemctl command in a browser (`?mock=disconnected`). Screens don't need their own. |
| Reconnecting | The shell shows a warn banner over the last known screen; the sidebar card says “Reconnecting · Trying again now” and keeps its lines. Screens keep rendering their stale data (don’t clear it), show `<LiveStatus />` (“Reconnecting”, still amber dot, never “Not running”), and disable every write while `useLive().online` is false: Sleep, Confirm and Cancel (`PendingChange disabled`), Save, the composer (`?mock=reconnecting`, or `&drop=<ms>` on any scenario). |
| Models loading | The shell shows a banner with indeterminate progress. Screens disable voice-dependent actions using `status.models_ready` (`?mock=starting`). |
| Not set up | No Anthropic token and no model endpoint (`GET /settings`, which reports secrets only as set or not set). The shell shows a neutral banner in the same slot, after models loading: “Ghost needs a model key before it can answer.” with a small secondary “Finish setup” button to Settings (to `#/welcome` while the mock is active). Not shown on Settings or Welcome themselves. Re-checked on every route change and window focus (`?mock=firstrun`). Read it with `useLive().setupNeeded`. A screen whose focal point would promise something that can’t happen yet says so itself and calls `useSetupPromptHere(true)`, which hides the banner there (one signal per place): the session’s start state reads “Finish setup to talk to Ghost” / “Ghost needs a model key before it can answer.” with a primary “Finish setup” button (`setupHref`), and Wake drops to secondary. |
| Waiting for okay | `pending` from `useGhost()`, always rendered with `PendingChange`; never build your own card or countdown. The session puts it in the `Page` dock directly above the composer, so it can never sit under the compact bar while its Confirm stays visible (Higgsfield’s approval card above its composer). The sidebar shows a count on Session and, on the live line, “Waiting on your okay” in `--warn` with an amber dot. One signal per place; no extra badge on the status card. The Confirm button is real confirmation evidence (ADR 0013): it must call `client.confirm()` and nothing else. `PendingChange` counts down from `pending.expires_at`, never `expires_in_secs` (a snapshot). |
| Okay expired | The change lapses: `PendingChange` shows “Expired · Ghost left it alone” with both buttons off for the moment until `pending` becomes null, the audit row turns `expired`, and Ghost says it left it alone (`?mock=expiring`, 8 seconds after load). |

## Routing

Hash routes, registered in `src/app/routes.tsx`. Each entry has `id`, `patterns`, `path`, `owner`, `title`, `kind` (`screen`/`prototype`/`foundation`), `chrome` (`app` or `bare`), `needsDaemon`, an optional `nav` item and a `React.lazy` component. Screens receive `{ params }` (e.g. `params.slug`). Navigate with `<Link to="/audit">`, `hrefFor(path)` or `navigate(path)` from `src/app/router`. Only hashes that start with `/` are routes; any other hash (`#main`, `#gal-color`) leaves the route alone, so an in-page anchor can never navigate to a 404. Still, never use `href="#id"` for an in-page jump: use a button that focuses or scrolls (the shell’s “Skip to content” is a button for this reason).

Prototype routes and the gallery are compiled only in development (`import.meta.env.DEV`) and appear in the sidebar only when the mock layer is active. The shell puts a strip over every prototype route: with the mock, “Prototype · Mock data only. Not connected to Ghost.”; against the real daemon, in warn tone, “Prototype · Connected to your Ghost. Saves here are real.” A prototype never adds its own badge for this.

### Adding a route

1. Create `src/screens/<owner>/index.tsx` (or `src/prototypes/<owner>/`) with a default export `function X({ params }: RouteProps)`, and `<owner>.css` imported from it.
2. Ask the foundation owner to add one entry to `ROUTES` in `src/app/routes.tsx` (and, for a prototype with its own data, one line in `src/design/mock/fixtures/index.ts`).
3. Add the route to your screenshots in every relevant scenario, both themes.

## Mock scenarios

Development only. Add `?mock=<scenario>` before the hash: `http://localhost:1430/?mock=session#/conversations/accounting`. `main.tsx` imports `src/design/mock` dynamically behind `import.meta.env.DEV`, so production builds contain none of it. The sidebar shows a Mock badge and a scenario switcher (`src/design/chrome.tsx`, lazy and dev-only too). A real screen’s design-only branch (a forced failure behind `?fail=`, a mock-only note) is gated on `import.meta.env.DEV && design`, not on `design` alone, so Vite drops the strings: a grep of the production build for “(mock)” is empty, and the build step checks it.

| Scenario | What it is |
|---|---|
| `passive` | Idle, waiting for the wake word, normal history. Three skills: one of Ghost’s own that needs a server no scenario declares (`month-close` needs `quickbooks`, so it shows as missing), one a department’s own read in place, one ready. Departments (ADR 0024): Ghost at the config root, Accounting (its own `books` server, using Ghost’s GitHub, two docs, the Q3 audit agent and a retired Q3 close under it), Infra and App (App with a `root` in a checkout). |
| `session` | Live session on the Accounting line, multi-turn transcript. |
| `pending` | On the main line; Ghost proposed closing ghost-mock/app#151 and waits for a yes. |
| `speaking` | Mid-session, Ghost is talking. |
| `thinking` | Mid-session, the model is working. |
| `starting` | Daemon up, voice models loading. |
| `disconnected` | ghostd not running: requests fail, the event stream closes. |
| `firstrun` | No secrets or endpoint set, no policy file, so no servers. |
| `empty` | Configured, nothing has happened: Ghost alone, no audit, no policy file, so no servers, no skills and no departments. |
| `expiring` | Like `pending`, but the change expires 8 seconds after load: design the countdown and the lapse. |
| `reconnecting` | A live session on Accounting; the daemon drops 1.5 seconds after load and stays away. |
| `phone` | A live session the paired phone started (`session_origin: "phone"`); the desk shows “On your phone” and can end it. |
| `busy` | Months of use: 13 agents, 200 audit rows, five servers (two GitHubs, a calendar, Mobbin, and one whose command isn’t installed), eight skills, three of them a department’s own, and a fifth department, Legal, with a problem the daemon reports (a literal secret in its `mcp.json`). |

URL options on any scenario: `&latency=0` answers instantly (default 140ms, to show loading states); `&connect=600` holds the event stream’s first open for 600ms (the Connecting state; past 900ms the shell shows Offline until it connects); `&drop=2000` drops the daemon 2 seconds after load (stream closes, requests fail); `&back=6000` brings it back 6 seconds after the drop. `&phone=off|unpaired|paired` sets the phone listener (ADR 0016): off, on with no phone, or a phone paired; `&phone=pairs` has the phone pair 5 seconds after a code is issued, and `&phone=expires` makes codes last 8 seconds. Settings’ `&settings=pairing` opens Pair a phone on load. In `pending` the fixture’s change is held open for design work; changes you make Ghost propose while using the mock expire after 60 seconds, as the real daemon’s do.

The mock is an in-memory ghostd (`src/design/mock/daemon.ts`) that implements every client method and the event stream, so interactions work: typing a message runs listening, thinking, speaking; "close" proposes a confirm-level change; Confirm and Cancel settle it and write the audit log in the gate’s words (the summary turns past tense, “closed the issue”, and the steps read as the gate’s `describe()` does: “comment: …”, then “closed the issue”); "transfer me to legal" switches agents, with the old agent speaking “Transferring you to Legal.” (or “Back to Ghost.”) as `ghost_voice/conversation.py` does; conversation edits, notes, reset, delete and compact all update state and emit `conversation` events; policy and settings saves persist for the page’s life. Skills (ADR 0023): add and remove change the in-memory list and write an audit row at `quiet`; the mock has no file system, so a dropped or typed path becomes a skill named after its last folder, and it refuses with the daemon’s own 409 sentences when the path is a file, a home or downloads folder, already inside `~/.config/ghost/skills`, or a name already installed (where the screen offers Replace). A project’s skill can’t be removed (409), as on the real daemon. Toolkits (ADR 0023): `PUT /toolkits/{name}` writes the toolkit into the in-memory policy file (its `[toolkit.<name>]` table shows in the TOML editor) and answers with the daemon’s own problem sentences (a level looser than the policy’s, a server or write the file doesn’t declare, an item outside the server’s scope, a skill that isn’t installed), 422 for a bad name or level; `DELETE` is 404 for an unknown name. `PUT /conversations/{slug}/toolkits` is 422 for a name the file doesn’t declare and 409 for Ghost or a topic line; creating a line inside a department with toolkits of its own is 422, as on the daemon. Templates: `POST /templates/{name}/apply` adds the template’s toolkits and skills, then the department (409 for a name already taken); `POST /conversations/{slug}/export` adds one of the operator’s own templates, which New line then lists as “yours”. Status, proposals and audit rows carry the department of the live line and the toolkit a write went through.

### Fixtures

Each owner's data lives in `src/design/mock/fixtures/<owner>.ts` as a default export `(scenario) => data`. Read the scenario's flags (`daemon`, `phase`, `session`, `pending`, `configured`, `data`) rather than its name, so new scenarios work everywhere. Use `ago(minutes)` for timestamps so relative times stay fresh, and `seeded(n)` for stable generated data. Paths are written from `~` (`~/.config/ghost/policy.toml`, `~/.local/share/ghost/conversations/<slug>`), never a real home directory. Keep data specific and believable: real-looking issues in `ghost-mock/app` and `ghost-mock/infra`, named teammates, the operator's plain voice.

Prototypes have no API behind them; read their fixture with `useFixture<T>("<owner>")`, importing the type only (`import type { ReviewFixture } from "../../design/mock/fixtures/review"`). `useFixture` returns null outside mock mode, so handle that.

## Screenshots

```
node .scratch/design/tools/shot.mjs "<url>" .scratch/design/shots/<owner>/<name>.png \
  [--theme dark|light] [--size 1440x900] [--full] [--click "Exact text"]... [--click-label "Accessible name"]... \
  [--hover "Exact text"] [--wait ms] [--mobile] [--scroll px] [--tab n]
```

`--click-label` clicks by accessible name (an IconButton’s label: `--click-label "More for Accounting"`); `--hover` rests the pointer on an element before the shot (tooltips, hover-only row actions). For a sequence (type, press Enter, change the hash, scroll to a section), `act.mjs` takes the same first two arguments and an ordered list of `--click`, `--type <selector> <text>`, `--press <key>`, `--hash /path`, `--scroll-to <selector>`, `--wait ms`, `--eval <js>`.

- The page panel scrolls, not the window. `--full` sets `<html data-capture="full">`, which lets the panel grow to its content so the whole page is captured; never set that attribute in the app.
- `--scroll 600` scrolls the panel before the shot (check sticky headers, the dock, the fade). `--tab 3` presses Tab three times (check focus).
- Shoot every screen in both themes at 1440x900 and at 1280x800, in every scenario that changes it. Use `--wait 1300` or more for `disconnected` (the shell waits about 900ms before deciding the daemon is down).
- The script prints page errors and console errors. Any output there is a bug.
- `--mobile` renders 390x844 at 3x for `#/mobile`.

## Review checklist

- One focal point; at most one primary button and one tinted card in view.
- Scrolled at least one screen (`--scroll`): the panel edge stays put, sticky things don’t let content peek, the dock fades.
- Toolkits: the designer is a real GUI, never a TOML field. A level select offers the policy’s level and anything more cautious (deny < confirm < announce < quiet), with the ceiling named under it; a saved level the policy no longer allows stays visible with the daemon’s problem under it. Problems come from the daemon, shown once in a warn Notice and again on the control they are about.
- Only tokens; no raw colors, sizes, shadows or durations.
- Sentence case, plain labels, env names only as `detail`.
- One `h1` per screen. Development builds count them in the page panel once it settles and log a console error otherwise.
- Keyboard: from a fresh load the first Tab reaches “Skip to content” and Enter on it lands in the page; everything reachable with Tab, once (no duplicate tab stops), visible focus, Escape closes overlays, Enter submits forms, Enter and Space open menus.
- Loading, empty, error and offline states designed and screenshotted.
- Dark and light both checked; text contrast at least AA, body text AAA.
- Nothing scrolls horizontally at 1280x800.
- No page errors in the screenshot output.
- Typecheck (`pnpm -s typecheck` at the repo root) and production build pass.

## References

Patterns compared on Mobbin while setting the foundation:

- Linear: inset content panel over a quiet sidebar, 13px nav with 16px icons; the panel stays fixed and scrolls inside ([screen](https://mobbin.com/screens/f00cc4fb-4083-43fc-a0fb-703a6c4ef771), [scrolled](https://mobbin.com/screens/40c3f3c4-f02b-4317-aff3-822bfaf84b3b)).
- Langdock, Lindy, Sana: named agents with small colored tiles in the sidebar; title and description left, actions right ([Langdock](https://mobbin.com/screens/6bf7d0e0-4cde-4e90-beb7-9971ecbc7bbe), [Lindy](https://mobbin.com/screens/c1a6ac79-67b6-454e-9716-48d1c4136182)).
- Base, Grok, Superlist settings: grouped rows in one rounded container, label and description left, control right, centered column ([Base](https://mobbin.com/screens/077991ac-384d-4768-bb43-0a2d4f8f5456), [Grok](https://mobbin.com/screens/58d3be4f-6ffc-4e42-91b6-814bae4dfe6d)).
- Cloudflare, Relume: destructive confirmation naming the object, Cancel plus red action bottom-right ([Cloudflare](https://mobbin.com/screens/aa928ac1-4a1d-4b8c-9ccb-be35211cdca8)).
- Customer.io, Klaviyo: compact activity logs with soft status pills and expandable rows ([Customer.io](https://mobbin.com/screens/d56c279e-cb13-4599-b12f-6e57ecb1ba64)).
- ElevenLabs, Grok voice: one central presence for voice state, a small live pill, transcript beside it, pill composer ([ElevenLabs](https://mobbin.com/screens/cf492872-e47b-453b-b476-0b5495411a83)).
- Delphi, PlayAI, Microsoft Copilot: one quiet ring or level arc around a central presence, a small “Listening” pill under it, nothing else moving ([Delphi](https://mobbin.com/screens/1dc157f3-7617-4ad0-b680-d957327cfc23), [PlayAI](https://mobbin.com/screens/3048235f-760d-4ec2-bf7e-b5c264715402), [Copilot](https://mobbin.com/screens/12555865-9340-476e-a3ef-bc2f8ccdb8a5)). Basis for `VoicePresence`.
- Dropbox Dash: a raised menu with 28px icon-and-label rows, hairline separators and a destructive item last ([screen](https://mobbin.com/screens/c71d9bb7-035a-4f72-b229-5bfc561f2470)). Basis for `Menu`.
- Klaviyo: radio cards with title and description for choices that need words ([screen](https://mobbin.com/screens/98ae6c2c-0f0c-4fc9-9e4e-bedb661b314f)). Basis for `RadioGroup variant="card"`.
- Meetup: a one-line “almost done” strip at the top of the page pointing at the step still missing ([screen](https://mobbin.com/screens/99433cf0-d8c7-49f2-917f-a44f0d395659)). Basis for the shell’s “Not set up” banner, kept neutral rather than tinted because nothing is wrong yet.
- Copilot and Delphi keep the live voice state on the voice screen at all times ([Copilot](https://mobbin.com/screens/12555865-9340-476e-a3ef-bc2f8ccdb8a5), [Delphi](https://mobbin.com/screens/1dc157f3-7617-4ad0-b680-d957327cfc23)). Basis for `Page`’s `status` repeating in the compact bar, and for the session’s centered start state.
