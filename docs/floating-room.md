# The floating room

ADR 0059. The room posture without a window: Ghost is a thin line of light at the bottom edge
of a monitor until its name is called, then comes forward over the desktop with no
application chrome, and goes back to the line when the session ends. The windowed app (Today,
the briefing) stays the place you open on purpose.

## Turning it on

Settings › General › The room › **Float over the desktop**, or the tray's **Float over the
desktop**. Off by default. Under it, **Ghost appears on**: the other monitor (default), a
named monitor, or where I am. The choice is kept beside the window
(`~/.config/dev.heka.ghost/floating.json`), not in Ghost's folder: it is how this desk shows
Ghost, like the room's own delays.

When it is on and you call Ghost while the main window isn't the one you are using, the
session comes forward over the desktop. If the main window is focused, the session stays in
it (and goes to its room after 20 s, as before). A room you hold in the window (Ctrl+.) wins.

## Where it works

| Desktop | Floating room |
|---|---|
| X11 session | Yes |
| GNOME on Wayland with XWayland (the default) | Yes: only this window goes through XWayland, the main window stays native |
| A Wayland session without XWayland | Refused with a sentence in Settings; the switch is off |
| GNOME native Wayland for this window | Not shipped: no keep-above and every map takes the keyboard (the spike, PR #84) |
| KDE, wlroots (layer-shell), macOS, Windows | Later |

## What it does

- **At rest**: one 56×2 px line of light at the bottom centre of the monitor; a struck red
  microphone when Ghost isn't listening (ADR 0057). Nothing runs: no frame loop, Wisp is not
  mounted, the input shape is empty, so every click goes through.
- **Forward**: the desktop dims, a scan line sweeps once, Wisp comes in from depth. The phase's
  word over the type (Listening to you, Using GitHub, Waiting for your yes, Accounting asks);
  your words, once your turn closes, light and grey; Ghost's reply in heavy type (800, up to
  96 px), the clause being said underlined from the daemon's `saying` stamps; the apps the
  turn reached on an arc while Ghost works; in the corner a margin of the session's real
  work: its clock, calls, open calls, apps reached, the waiting count, the last eight calls
  with their times (`github.issue_read 1.2 s`), and **Open in Ghost**.
- **A yes or a question**: the question in heavy type and its answers as large one-word
  buttons (Yes, No; an agent's choices), "or say it". Clicking is the same as the window's
  Confirm. Nothing else on the surface takes a click.
- **Hold**: the line, with "On hold · say “Ghost” to pick up" over it.
- **Done**: Wisp falls back into depth and the light drops to the line.

It never takes the keyboard (the window refuses input focus and is never mapped with it) and
everything it shows is what the window would show, under the same rules (#73: never your
words while you speak).

Reduced motion: no zoom, no scan, one frame per change. Forced colours: the glows go. Screen
readers: one polite status line per change (the scene, and the reply whole when it starts);
a yes or a question is an `alertdialog` named by its question.

## Screen sharing

While the floating room is on, the shell asks mutter every two seconds whether a screen-cast
or remote-desktop session exists (D-Bus introspection of `/org/gnome/Mutter/ScreenCast/Session`
and `/org/gnome/Mutter/RemoteDesktop/Session`: a child node is a session). Every capture on
GNOME goes through one: a browser's meeting through the portal, OBS, GNOME's own recorder,
remote desktop. While one exists the surface draws nothing at all and a spoken session shows
in the window. Not seen: a program that grabs XWayland's pixels itself (`ffmpeg -f x11grab`)
and any desktop other than GNOME; Settings says so when it can't tell, and the tray switch
turns the floating room off at once.

## A full-screen app

A focused full-screen window (a game, a video, a presentation) is left alone: Ghost comes
forward on another monitor instead, and with only one monitor it does not come forward (the
voice goes on). mutter keeps a focused full-screen window above windows that keep themselves
above, so the line is under it too. This is known for an X11 app from the X11 layer; for a
Wayland app only with the helper below.

## Which monitor, and the helper

`floating::place::choose` is a pure function of the choice and what is known. "Where I am" is
the focused window's monitor, else the pointer's. Measured on GNOME Shell 50.1 under XWayland
(2026-11-12, `heka-ghost-desktop --floating-check`):

- `XQueryPointer` answered `942,1079` at every read across 40 minutes. XWayland only learns
  the pointer while it is over an X window, so this is where it last crossed one; whether the
  pointer moved in those minutes is item 1 of the owner's check below. Not believed under
  XWayland.
- `_NET_ACTIVE_WINDOW` was `0x400003`, a window not in `_NET_CLIENT_LIST` (mutter's own),
  while native Wayland windows had the focus. Only a focused X client counts.
- `_NET_CURRENT_DESKTOP` follows the workspace.
- `org.gnome.Shell.Introspect` and `Eval` are closed to ordinary clients.

So on GNOME Wayland the pointer and the focused window come from **Ghost's desktop helper**,
an optional GNOME Shell extension in `apps/desktop/gnome-shell/ghost-desktop@heka.dev`. It
exports one D-Bus method, `dev.heka.Ghost.Desktop.Where()`, returning the pointer, the focused
window's frame, whether it is full screen and the workspace: positions only, never a title or
an app, answered once per wake. Without it, "the other monitor" is the one that isn't primary
and "where I am" is the same.

Installing it is the operator's choice, since it changes GNOME's list of extensions:
`apps/desktop/gnome-shell/install.sh` (packs and installs it for this user), log out and back
in once on Wayland, then `gnome-extensions enable ghost-desktop@heka.dev`. Remove it with
`gnome-extensions uninstall ghost-desktop@heka.dev`. GNOME turns off an extension whose
`shell-version` doesn't list the running release, so a GNOME upgrade disables it until the
list is bumped; it uses only `global.get_pointer`, the focus window's frame and the workspace
manager, which have been stable across releases. The floating room works the same without it,
less the "where I am" part.

## Workspaces

The surface is on every workspace (`_NET_WM_STATE_STICKY`, `_NET_WM_DESKTOP` = all), so the
wake finds you wherever you are. Bringing the main window to you (Open in Ghost, the tray's
Open Ghost, "Ghost, open the app"): a window that is showing but not focused is unmapped and
mapped again, which GNOME treats as a new window: it lands on the current workspace and gets
the focus. A plain raise would switch your workspace to it, or, with no activation token
(nothing outside a click in a Wayland window has one), only show "Ghost is ready".

## Cost

Method: the spike's `measure.sh` (on PR #84's branch: CPU from /proc for the surface's three
processes, gnome-shell and Xwayland; the GPU's busy share from its RC6 counter), each scene
held still by a mock scenario for 8 s. Intel UHD 750, two 1920×1080 monitors, debug build.

| State | Surface | gnome-shell | Xwayland | GPU busy @ clock |
|---|---|---|---|---|
| No surface (baseline) | — | 0.3–0.5% | 0 | 0.2% |
| At rest, the line | 0.1% | 0.4% | 0 | 0.2% |
| Forward, any scene (listening, you, thinking, speaking, a yes) | 13% | 6% | 9% | 58% @ ~400 MHz |
| The same before Wisp was capped (this branch, first build) | 25–28% | 7% | 19–20% | 74–81% @ 670–920 MHz |
| The spike, XWayland (PR #84) | 32–51% | 9–17% | 20–29% | 81–84% @ 840–1000 MHz |

Frames: 30 a second in every scene, 60 for the arrival's zoom and the leaving; 0.1–0.3 ms of
script per frame. Where the time went: Wisp redrawn at 60 frames a second was half of it
(without Wisp: 9.5% and 8.6%; without the field: 20.8% and 17.5%), since each frame is the
whole monitor's surface copied through XWayland. Drawing the field at 0.25 instead of 0.4
resolution changed nothing measurable. WebKit throttles the page's timers in a window that
never takes focus (a 750 ms timeout ran 10 s late), so the arrival and the leaving are timed
from the frame loop.

## Developing it

- `http://localhost:1420/floating.html?mock=floating` plays the whole sequence over and over
  in a browser (`&main=active`, `&capture=yes`, `&covered=1`, `&side=left`, `&still=1`,
  `&listening=off`). The main window's mock takes `&floating=on|off|refused`.
- The real surface on this desktop, with the mock page (debug builds only):
  `GDK_BACKEND=x11 GHOST_FLOATING_URL='http://localhost:1420/floating.html?mock=floating' target/debug/heka-ghost-desktop --floating`
  (the page must come from the app's dev address, 1420; close it with Ctrl+C or by closing
  its stdin).
- `heka-ghost-desktop --floating-check` prints what the desktop tells it now, as JSON: the
  monitors, the capture, the X11 reads, the helper's answer and each choice's monitor.

## The owner's check (two minutes)

Things this session could not do without your hand. With the dev page served
(`pnpm --filter @ghost/desktop exec vite --port 1420`) and the binary built
(`cargo build -p heka-ghost-desktop`):

1. **The pointer from X11.** Put the pointer on the right monitor over a native window (Files,
   Settings), run `target/debug/heka-ghost-desktop --floating-check | grep -A3 '"pointer"'`;
   move it to the left monitor and run it again. Expected: the same numbers both times (stale).
   Then the same over an X11 window (Spotify, Discord here): the numbers follow the pointer.
2. **The focused window.** Click into a native window, run the check: `"focus": null`. Click
   into Spotify: `focus` is its frame and `where_i_am` its monitor.
3. **Workspaces.** Start the surface with the mock (above), wait for Ghost to come forward,
   switch workspace (Super+Page Down): Ghost is there on the new one, and the line stays after
   the session ends. Switch back: the same.
4. **Click-through and the keyboard.** While Ghost is forward, click a window behind the dim:
   it takes the click. Type in a text field: the letters go there. The Yes and No buttons take
   a click (the mock answers).
5. **Screen sharing.** Start a share (a browser meeting's "present", or Ctrl+Alt+Shift+R), run
   the check: `"capture": "yes"`. With the floating room on in the real app, Ghost disappears
   within two seconds and comes back when the share ends.
6. **Bringing the app.** With the real app on (the tray's Float over the desktop), put the main
   window on workspace 1, go to workspace 2, use the tray's Open Ghost: the window appears on
   workspace 2, focused. Say "Ghost, open the app" in a session: the same.
7. **The helper** (optional): `apps/desktop/gnome-shell/install.sh`, log out and in,
   `gnome-extensions enable ghost-desktop@heka.dev`, run the check: `helper` has your pointer;
   move to the other monitor and run it again: it follows. `gnome-extensions uninstall
   ghost-desktop@heka.dev` to remove it.
