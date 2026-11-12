# Floating Ghost: a spike, November 2026

The owner asked on 2026-11-12: could Ghost drop all of the desktop app's chrome and float
over the desktop, coming into focus when called, with the mood of neon-noir science fiction
(glow on black, haze, scanned light, heavy type, the feeling of wielding a supercomputer)?
And which motion graphics stack would get us there?

This is a feasibility spike with a working prototype. Nothing in it is meant to merge into the
product. The normal app is unchanged: the spike is a second binary behind a cargo feature,
with its own config, and a page of its own.

**Short answer:** yes on X11 (and on GNOME through XWayland), with real limits. On GNOME's
native Wayland a normal app cannot do it properly: no keep-above, and each summon takes the
keyboard. The look holds up: one WebGL2 pass plus Wisp plus DOM type runs at 60 fps on this
machine's integrated GPU, and costs nothing measurable when idle. **Recommendation:** build
it as a third posture that the operator turns on, X11/XWayland first on Linux, then macOS and
Windows, where the platform supports every part of it. Add no motion libraries.

## Run it

```
apps/desktop/src/design/floating/run.sh demo           # native Wayland
GDK_BACKEND=x11 apps/desktop/src/design/floating/run.sh demo   # XWayland (recommended)
apps/desktop/src/design/floating/run.sh send demo|summon|dismiss|state <phase>|mic off|yes|no|quit
```

`run.sh` builds `cargo run -p heka-ghost-desktop --example floating --features floating`,
starts Vite on :1431 for the page, and closes both on Ctrl+C or `send quit`. Switches
(`apps/desktop/src-tauri/examples/floating.rs`): `FLOATING_MONITOR`, `FLOATING_PLACE`
(`fullscreen`, `maximize`, `size`), `FLOATING_TYPE` (`normal` by default: see below), `FLOATING_IDLE`
(`mapped`, `hidden`), `FLOATING_FOCUSABLE`. When the window has focus, Space plays the demo
and Escape dismisses. The page also opens in a plain browser at
`http://localhost:1431/src/design/floating/index.html?auto=demo`. It starts no daemon, uses
mock data only, and never opens the microphone or the speaker.

Files: `apps/desktop/src-tauri/examples/floating.rs` and `floating/tauri.conf.json` (the
window), the `floating` feature in that crate's `Cargo.toml` (it only adds `gtk`, already in the
graph through Tauri), `apps/desktop/src/design/floating/` (the page, `measure.sh`, `bench/`).

Captures (outside the repo, `~/projects/active/ghost/.scratch/design/floating-2026-11/`):
`summon-sequence.mp4` (33 s, the full demo), `contact-sheet.png`, one PNG for each state,
`idle-ember-zoom.png` / `idle-mic-off-zoom.png`, `reduced-motion-speaking.png`, and
`bench-*.png`. GNOME refuses screenshots and screen recording without a portal prompt this
session cannot answer. So the captures come from the XWayland run: `import -window` for the
stills and `ffmpeg -f x11grab -window_id` for the video. Both show only the window, so the
desktop behind it is black. A capture of XWayland's root window is all black, since in rootless
mode it holds no other client's pixels. While ffmpeg was capturing, the page dropped to 40–50
fps; without the capture it holds 60.

## 1. The window

Measured on this machine: Ubuntu, GNOME Shell 50.1, Intel UHD 750, two 1920×1080 monitors
at scale 1, WebKitGTK 2.52.6, Tauri 2.12.5 (GTK3/tao). XWayland: `xprop`, `xwininfo`, an XShape
query (`/tmp/flt/tools/inputshape.py`, not committed) and `_NET_CLIENT_LIST_STACKING`. Native
Wayland: what GDK reports after mapping (logged by the example), since GNOME denies
`org.gnome.Shell.Introspect` and screenshots.

| Property | Native Wayland (GNOME) | XWayland (`GDK_BACKEND=x11`) |
|---|---|---|
| Transparent window, compositor blends it | Yes (RGBA surface; WebKit's page background transparent) | Yes (32-bit ARGB visual) |
| Undecorated, no shadow | Yes | Yes (`_MOTIF_WM_HINTS` no decorations) |
| Keep above other windows | **No.** GTK's `set_keep_above` does nothing; GNOME has no protocol for it. Once the operator clicks another window, it goes above Ghost | **Yes** (`_NET_WM_STATE_ABOVE`; Ghost stayed top of the stack) |
| No taskbar or alt-tab entry | No protocol for it, so the dock and alt-tab are expected to list it (not observable from this session) | Yes (`SKIP_TASKBAR`, `SKIP_PAGER`, `STICKY`) |
| Place and size itself | Not by position. **Fullscreen on a chosen monitor works** (`gtk_window_fullscreen_on_monitor`, which xdg-shell carries): got 1920×1080 on monitor 0 or 1 as asked. `maximize` gives the work area (1920×1005). `move` is ignored | Yes. `size` puts it exactly at the monitor's origin. Fullscreen works only with type `normal`: mutter refuses fullscreen to a `utility` window and to a window that can't be resized |
| Span the monitor | Yes, as fullscreen | Yes (fullscreen or explicit geometry) |
| Click-through except where Ghost is drawn | Set the same way (GDK input shape, which becomes `wl_surface.set_input_region`). Could not be observed here: this session cannot inject pointer input | **Verified**: input shape is 0 rectangles when idle, 1 (Wisp) while listening, 3 (Wisp and both buttons) while waiting for a yes |
| Don't take the keyboard | **Fails.** Every map activates the window (`FOCUSED` from the compositor), even with accept-focus and focus-on-map off. Hiding and re-showing on each summon takes focus every time | **Yes.** `_NET_ACTIVE_WINDOW` never moved to Ghost (`DEMANDS_ATTENTION` is set instead) |
| Several monitors | One surface per output; pick it with fullscreen-on-monitor. Spanning both means two windows | Same, or one window sized across both |
| Fractional scaling | Not tested: changing the scale on the operator's desktop was out of bounds. GTK3 has no fractional-scale protocol, so it renders at the next integer scale and mutter scales down (sharp, about 1.8× the pixels at 125%) | mutter's `xwayland-native-scaling` is on here. X clients render at native resolution and must scale themselves (`GDK_SCALE`) |
| A full-screen app in front | Unknown, and likely bad: with no keep-above, any focused window is above Ghost | An *unfocused* full-screen X window stayed below Ghost. mutter puts the *focused* full-screen window above "always on top" windows, so a game or a video in full screen will cover Ghost (not reproduced: launching a focused window is blocked) |

WebGL2 works in the transparent window on both backends (`webgl2=true`; WebKit masks the
renderer string as "Apple GPU"). Premultiplied alpha composites correctly over the desktop.
**Keep the DMABUF renderer on**: with `WEBKIT_DISABLE_DMABUF_RENDERER=1` the same scene took
91% of a core at 53 fps, against 39% at 60 fps.

### What it costs

`measure.sh` samples /proc for the window's three processes (the shell, WebKit's web and
network processes), gnome-shell and Xwayland, and reads the GPU's busy share from its RC6
counter. All runs were fullscreen 1920×1080, the field at half resolution, a debug build of the
shell (WebKit is the system's). Figures are % of one core; the baseline desktop moves between
0.4 and 3% for gnome-shell.

| State | Window | gnome-shell | Xwayland | GPU busy @ clock |
|---|---|---|---|---|
| No floating window (baseline) | — | 0.4–3.0 | 0.0 | 0.3–0.9% |
| Wayland, mapped, idle (ember only) | 0.1–0.9 | 2.8–4.0 | 0 | 1.8–2.2% @ ≤35 MHz |
| Wayland, unmapped, idle | 0.0 | 0.6 | 0 | 0.5% |
| Wayland, listening / thinking / speaking | 37 / 40 / 32 | 12 / 15 / 7 | 0 | 52 / 64 / 55% @ 185–280 MHz |
| XWayland, mapped, idle | 0.1–0.3 | 1.3 | 0.0 | 2.1–3.2% |
| XWayland, listening / thinking / speaking | 32 / 34 / 51 | 9 / 10 / 17 | 20 / 22 / 29 | 81–84% @ 840–1000 MHz |

The page's own frame stats (rAF in WebKitGTK): 59–60 fps, p95 frame interval 17–22 ms, and
0.1–0.5 ms of JavaScript per frame in every phase on both backends.

Where the time goes (Wayland, thinking, % of a core in the window): everything 39, without the
field 21, without Wisp 24, without either 10 (React, the label nodes, compositing). With the
field at quarter resolution 30. The reply's blurred text glow made no difference that
measurement could separate.

**Idle costs nothing measurable.** At idle the frame loop is stopped, the canvas is hidden,
Wisp is unmounted, and the input region is empty. The window still being mapped adds less than
1% of a core to the shell and to gnome-shell, inside the baseline's own noise. Unmapping it
adds nothing at all, but on Wayland it then takes the keyboard on every summon. On screen, the
moment costs a third to a half of a core and some GPU for a few seconds. That is fine for a summon
and wrong for a decoration. **XWayland roughly doubles the on-screen cost** (Xwayland's copy,
the GPU at a 3–4× higher clock). That is the price of keep-above and not taking the keyboard.

### Other platforms (from documentation; not tested)

- **KDE Plasma (Wayland):** KWin implements `wlr-layer-shell`. A layer-shell surface on the
  overlay layer, `keyboard_interactivity: none`, sits above everything without focus and
  without a taskbar entry. GTK3 needs `gtk-layer-shell` (wrapped by the `gtk-layer-shell`
  crate). KWin also has a blur protocol (`org_kde_kwin_blur`) for a real blur behind Ghost.
  The best Linux target.
- **wlroots compositors (Sway, Hyprland), COSMIC:** layer-shell, same as KDE.
- **X11 sessions:** as the XWayland column, with no Xwayland copy. Focus and stacking depend
  on the window manager, but EWMH `ABOVE`, `SKIP_TASKBAR` and input shapes are universal.
- **GNOME Wayland, the only proper route:** a GNOME Shell extension that keeps Ghost's window
  above everything, out of alt-tab and unfocused (extensions can set a window's layer and
  focus). That means maintaining an extension for each GNOME release, with GNOME's review.
- **macOS:** an `NSPanel` with `.nonactivatingPanel` at `.screenSaver` or `.statusBar`
  window level, `ignoresMouseEvents` and `collectionBehavior` for every Space and full-screen
  apps. Tauri exposes `transparent` with `macOSPrivateApi`, plus `always_on_top`,
  `visible_on_all_workspaces` and `set_ignore_cursor_events`. Hit-testing only where drawn
  needs a small native view (`hitTest:`), or toggling ignore-mouse as the pointer moves.
  Vibrancy (`window-vibrancy` crate) gives a real blur behind. Screen recordings and sharing
  capture it unless `sharingType = .none`.
- **Windows:** a `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`
  topmost window is the classic overlay. Tauri's `set_ignore_cursor_events` covers the whole
  window; per-region hit testing needs `WM_NCHITTEST`. Acrylic/Mica blur is available.
  `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` keeps it out of screen capture.
  Exclusive-full-screen games will cover it.

**No blur on GNOME.** A client cannot blur what is under it on GNOME (no protocol), so the
prototype dims instead: black at 46–58% with a vignette and coloured haze, which reads as
the room's lights going down. Where a platform offers blur (KDE, macOS, Windows), use it under
the dim.

## 2. The motion stack

The same scene, the field's shader with the thinking state's inputs at half resolution, was
built in raw WebGL2, OGL, three.js (plain, and with EffectComposer + UnrealBloomPass) and
PixiJS 8, run in the floating window on native Wayland and measured the same way
(`bench/`, `bench/build.sh`). Sizes are esbuild-minified bundles of what this kind of stack
imports (`bench/sizes.mjs`).

| GPU layer | min / gzip | Licence | Window CPU | GPU busy | Live data | Ours to own |
|---|---|---|---|---|---|---|
| **Raw WebGL2 (`field.ts`)** | **7 KB** bundle incl. shader | ours (MIT) | **14.6%** | 56% | uniforms set each frame | all of it: ~230 lines, one shader |
| OGL 1.0.11 | 43 / 13 KB | Unlicense | 15.7% | 63% | uniforms | the shader; OGL owns 40 lines of setup |
| three.js 0.187 | 520 / 130 KB | MIT | 18.6% | 66% | uniforms | the shader |
| three.js + UnrealBloomPass | 534 / 134 KB | MIT | 22.8% | 51% | uniforms | the shader; bloom is theirs, but it washed the frame out and it is known to drop alpha (needs a patched pass over a transparent window) |
| PixiJS 8.23 | 532 / 155 KB | MIT | 20.3% | 49% | filter uniforms | the shader, rewritten into Pixi's filter conventions |

Same pixels everywhere, so the differences are each library's per-frame overhead and its
size. GPU figures move ±10 points between runs (the clock governor). None of the libraries
adds anything the look needs: the glow, bloom, streak, ring, beams, haze and dust are one
analytic shader. A real multi-pass bloom costs more and, over a transparent window, fights the
alpha. **Recommendation: raw WebGL2.** Move to OGL if we ever need several meshes or render
targets; skip three.js and Pixi.

Mascot runtime:

| | Size | Licence | Live inputs | Reduced motion | Notes |
|---|---|---|---|---|---|
| **Wisp's own engine** (`@ghost/ui` `mascot.ts` + `wisp.ts`, `Mascot.tsx`) | already in the app | ours | `mood` per phase, `trigger`; a frame is plain numbers, so a `level` input (mouth open with the voice) is about 20 lines | `still`, follows the media query | One engine for desktop and phone. Measured ~15% of a core at 224 px while it loops |
| Rive (`@rive-app/canvas` 2.50) | 352 KB JS + 2.2 MB wasm (931 KB gz) | runtime MIT; the editor is Rive's proprietary tool (free tier) | state-machine inputs (number, bool, trigger), the best model for "driven by level and state" | we'd build it | We would redraw Wisp in Rive's editor, keep a second source of truth beside `wisp.ts`, and ship 2.5 MB more. Not built here: a `.riv` needs Rive's editor |
| Lottie (`lottie-web` 5.13) | 302 / 78 KB (light 168 / 48) | MIT | none to speak of: segments and frames only (dotLottie's state machines are newer, a separate runtime) | pause | Exported from After Effects, so no live voice. Wrong fit |

**Recommendation: keep Wisp's engine** and give it a `level` input. Rive is the right tool only
if a motion designer joins and Wisp gets redrawn for it.

UI motion:

| | Size | Licence | Notes |
|---|---|---|---|
| **WAAPI + CSS** (what the prototype uses) | 0 | — | The zoom from depth, the dismissal and the yes card's arrival are `element.animate` and one keyframe; the compositor runs them; reduced motion sets the duration to 0 |
| Motion 14 | 71 / 26 KB (`animate`), 142 / 49 KB (`motion/react` with layout) | MIT | Worth it only for shared-layout transitions (the windowed app's record turning into the floating reply), not for this |
| GSAP 3.15 | 69 / 27 KB | GSAP "Standard no charge" licence: free, but not an open-source licence, and it bars use in tools that compete with Webflow | Keep it out: it would be the only non-OSI licence in the app |

**The stack: raw WebGL2 + Wisp's engine + WAAPI/CSS, no new dependency.** cargo-deny is
unaffected (the `gtk` crate the spike adds is already in Tauri's graph). The JS lockfile is
untouched; the candidates were installed outside the repo.

## 3. The prototype

`apps/desktop/src/design/floating/`: `Floating.tsx` (the sequence and the layers),
`field.ts` (the shader), `script.ts` (mock data and timing), `shell.ts` (the window's three
commands), `floating.css` (prefix `flt-`). Three layers back to front: the field (WebGL2, half
resolution, stretched by the compositor), Wisp (SVG from the brand's own engine, 224 px), and
the words (DOM).

- **Idle: a single ember.** A 44×2 px line of light at the bottom centre of the screen, still,
  drawn once. Nothing is a lie on an always-on-top surface: the ember says Ghost is there and
  hearing for its name. With the microphone off (ADR 0057) it becomes a red struck microphone
  in the same place. Nothing at all would be calmer, but on a surface that is always there,
  the operator should never have to wonder whether it is listening. It costs nothing: no loop
  runs. (Captures: `idle-ember-zoom.png`, `idle-mic-off-zoom.png`.)
- **The wake (0.95 s).** The desktop dims with a vignette, a scan line sweeps once down the
  screen, Wisp flies in from depth (perspective `translateZ(-2600px)` with blur and over-
  bright, settling at 78%), and a shockwave rings out of the core as it lands. An anamorphic
  streak crosses the core; an instrument ring of 120 ticks turns slowly around Ghost.
- **Listening.** Green. The operator's voice is energy only: the inner ring ripples and
  thickens with the level, the ticks grow with it, a second ring trails out on loud moments.
  No words, ever (the rule of #73 and the conversation screen).
- **Thinking, its reach visible.** Violet. Each app or agent Ghost touches appears on an arc
  over its head ("GitHub · issue_read", "Ledger · agent", …), a beam lights from the core
  with packets travelling out while the call runs and dims once it answered. In the product
  these are ADR 0055's activity events by app and tool name.
- **Speaking.** Cyan. The ticks follow Ghost's own voice level; the reply sits in heavy type
  (Inter 800, 68 px at 1080p) under Wisp, said words bright with a glow and a hair of
  chromatic split, the unsaid dim, clocked by `@ghost/ui`'s `splitSaid` and
  `CHARS_PER_VOICED_SEC` as on the conversation screen.
- **A yes.** Amber, as the design system reserves amber for "waiting for your okay": an amber
  horizon low on the screen, "Ledger asks · github_issue_write", the question in heavy type,
  "Yes, close it" and "Leave it" as the only clickable things besides Wisp, "or say “yes”".
- **Dismissal (0.75 s).** Wisp falls back into depth, the light drops to the bottom edge and
  goes out where the ember is: back to nothing, and you can see where it went.

Departures from docs/design-system.md, on purpose and only here: weight 800 (the system allows
400–600), colours outside the tokens (neon on black, a dark-only surface), uppercase mono
labels. If this ships, they become tokens of a "stage" theme rather than local values.

Reduced motion (`?still=1` or the OS setting): no zoom, no scan, no loop. One frame is drawn per
change of state (`reduced-motion-speaking.png`). Forced colours: the glows go.

## 4. How it fits the product

**It is the room, unbound from the window**, and a third posture beside the briefing (at the
desk) and the room (a session in the window). The overhaul's recommendation stands: the
windowed app is where the operator works with Ghost, and the briefing is its home. The
floating surface is what a *spoken* session looks like when the window is closed or behind
other work: the wake brings Ghost to the front over whatever the operator is doing, and
dismissal leaves them where they were. It should not become the whole app. Settings, the
waiting list, agents and watches are things you sit down to, and an overlay is the wrong
place for them. Make it an operator's choice (Settings › Sound or General: "When I call Ghost:
open the window / float over my screen"), off until it is good on the platform in use.

What the daemon would need to provide; almost all of it exists:

- The wake and the session's end (status phase changes), Ghost's voice level (`voice`), the
  operator's turn opening and closing and tool calls by app and tool name (`activity`, ADR
  0055), the reply text, `pending` and questions (ADR 0051), and the listening switch (ADR 0057).
  All of it is on the existing event stream. The shell's bridge would feed the floating window
  the way it feeds the main one.
- New: an event when a reply *starts being said*, with its text, so the big type can appear
  with the voice instead of after the turn. Also, if the estimate is ever visibly off, the
  word clock from the TTS stage (already "still to do" in the overhaul doc).
- Nothing on the passive path: the floating window must never get passive audio or
  transcripts (ADR 0013). The ember's "hearing for its name" is the listening switch's
  state, not audio.

Privacy and trust:

- **Always say whether it hears.** The ember (listening for its name) or the red struck
  microphone (not listening) is on screen at all times, idle or summoned. While summoned, the
  label says "Listening" and adds "mic off" whenever the switch is off. The surface must never
  draw a listening state while the microphone is closed, and should be the clearest place to
  see that it is closed.
- **Screen sharing and recordings will show Ghost**, including replies and questions with issue
  text in large type. On GNOME there is no capture exclusion. Ghost should hide the surface
  (or fall back to the window) while the screen is being shared or recorded, using the
  portal's screencast session state where it can be read. On macOS and Windows, set capture
  exclusion. Until then, a "don't float while sharing" setting.
- The surface only ever shows what the windowed app would show, under the same rules: no
  words while the operator speaks, app text as data, the gate's yes unchanged. "Yes, close
  it" is a `client.confirm()` like the Confirm button (ADR 0013).

Accessibility: Wisp and the buttons are real elements; the label is `aria-live`, the yes is an
`alertdialog` named with its question, and the reply carries its whole text as the label while
the said/unsaid split is `aria-hidden`. Keyboard: the floating window should not take the
keyboard (that's the point), so every action must also exist in the windowed app and by
voice. A global shortcut to answer a yes from the keyboard would need the desktop's
global-shortcuts portal on Wayland. Reduced motion is honoured; contrast is high by
construction (near-white on a darkened desktop).

What could go wrong on a real desk:

- **Covering work.** A dimmed full screen for every summon is heavy for a 3-second "what's
  next on my calendar". Mitigation: dim only for the wake and for a yes; for a short answer,
  keep the dim near zero and the type smaller. The prototype already lightens while speaking.
- **Distraction.** Motion in peripheral vision is the most attention-grabbing thing a screen
  can do. Nothing moves when idle; the summon is the operator's own doing.
- **Clicks in the wrong place.** Only Wisp and the buttons take the pointer; everything else
  passes through. But on GNOME Wayland a click on the window underneath raises it above
  Ghost, which disappears mid-sentence.
- **Games and video in full screen** cover it (and should). Ghost should then fall back to
  voice only, or to a notification.
- **Two monitors.** Choose the one with the pointer, or the one the operator is looking at
  (unknowable); per-monitor windows double the cost.
- **Cost if it stays up.** A third to a half of a core while on screen. A session left open
  for minutes should settle into a still frame (the loop runs only while something changes:
  voice, calls, the said clock).

## Recommendation

**Build it, as an opt-in third posture, on the platforms that support it, in this order:**

1. **Linux on X11 and XWayland** (GNOME included, via `GDK_BACKEND=x11` for this window
   only): keep-above, no focus taken, input shapes and placement all work today, at about
   twice the on-screen cost of native Wayland, and nothing when idle. Wire it to the daemon's
   existing events, add the "reply starts" event, add the screen-share hide, and give Wisp's
   engine a `level` input.
2. **KDE and wlroots** via layer-shell (`gtk-layer-shell`): the proper Wayland overlay, with
   blur on KDE.
3. **macOS and Windows** when Ghost goes there: both have first-class overlay windows,
   capture exclusion and blur.
4. **GNOME native Wayland** only through a Shell extension, if at all. Until then, GNOME
   users get it through XWayland or not at all, and the windowed room stays the default.

Don't build it as the whole app, and don't bring in three.js, Pixi, Rive, Lottie, Motion or
GSAP for it: one shader, Wisp's engine and WAAPI gave the look at 60 fps with nothing to
license.

### Not done in the spike

Fractional scaling (would have changed the operator's display settings); click-through and
stacking under native Wayland (no input injection, and GNOME refuses introspection); a focused
full-screen app in front (couldn't launch one with focus); a Rive version of Wisp (needs
Rive's editor). The benches' fps are not in the table: their pages come from another origin,
which the window's capability doesn't let report back; the CPU and GPU numbers are from
/proc and RC6 and the screenshots show each rendered.
