# 0059: The room floats over the desktop

- **Status:** accepted
- **Date:** 2026-11-12
- **Builds on:** ADR 0013 (the control API and its events), ADR 0038 (the daemon ships inside the desktop app), ADR 0055 and ADR 0058 (what the desktop sees of a session), ADR 0057 (not listening)
- **Amends:** ADR 0013 (one more event, `show_app`)

The owner chose two postures for the app on 2026-11-12: the briefing at the desk and the room
during voice (docs/design-system.md, The two postures). The spike of the same day (PR #84,
docs/design/floating-2026-11.md on its branch) showed that the room can leave the window: a
transparent surface over the desktop that is a thin line of light until Ghost's name is
called, comes forward over whatever the operator is doing, and goes back when the session
ends. The owner: "Build it once the new frame has merged." Their words for the feel:
"communicating with and wielding the power of a super computer".

## Decision

**A third posture, `float`, opt-in and off by default.** The one rule in
`apps/desktop/src/app/posture.ts` decides it: a live session at this desk floats when the
operator turned the floating room on, it can show now, and the main window is not the window
the operator is using (focused and on screen). A room held in the window still wins. Off, or
where it cannot show, nothing changes. The main window, the floating surface and the
conversation screen's room view (which no longer keeps its own timer) all read that rule.

**A second process of the desktop app, through X11.** `heka-ghost-desktop --floating`, started
by the shell with `GDK_BACKEND=x11` when the switch is on and stopped when it is off (its stdin
closes). On X11 and XWayland one window can keep itself above the others
(`_NET_WM_STATE_ABOVE`), be on every workspace (`STICKY`, `_NET_WM_DESKTOP` all), stay off the
taskbar, the pager and the switcher, refuse the keyboard (`WM_HINTS` input false, never
mapped with focus), and take clicks only where it draws (an X input shape: empty at rest,
the buttons while forward). GNOME's native Wayland gives an ordinary client none of that, so
the main window stays native and only this one goes through XWayland. Where there is no X11
(a Wayland session without XWayland) the switch is refused with a sentence; a broken native
Wayland mode is not shipped. The surface reads the daemon's events through the same bridge as
the main window; it changes nothing the main window cannot.

**It never takes the keyboard and every answer has two ways.** A change waiting for a yes and
an agent's question show their buttons (a click is the operator's own, `classifier: ui` as in
the window, ADR 0013); the voice path is unchanged. "Open in Ghost" brings the main window to
the operator.

**It is honest about the microphone.** The line at rest is the listening sign; with listening
off (ADR 0057) it is a struck red microphone. While forward, the operator's turn is a sign
from `you_started`, never a level and never words while they speak (#73, ADR 0055).

**It hides from screen capture.** The shell asks mutter every two seconds while the switch is
on whether a screen-cast or remote-desktop session exists (introspection of
`/org/gnome/Mutter/ScreenCast/Session` and `/org/gnome/Mutter/RemoteDesktop/Session`, open to
any session client, no portal prompt). One exists: the surface draws nothing, not even the
line, and a spoken session shows in the window. Where that can't be read (another desktop,
or a client grabbing XWayland's pixels directly) Settings says so and the tray's "Float over
the desktop" turns it off at once.

**A focused full-screen app is left alone.** The monitor it fills is not chosen; with no other
monitor Ghost does not come forward (the voice goes on, the line is under the app).

**Which monitor is a pure function of what is known** (`floating::place::choose`): the
operator's choice, "the other monitor" (default), a named one, or "where I am". Where the
operator works is the focused window's monitor, else the pointer's. Under XWayland the X11
layer cannot tell either while a native Wayland window has the focus, so only a focused X
client counts; the rest comes from an optional GNOME Shell extension shipped with Ghost
(`apps/desktop/gnome-shell/`, positions only, installed by the operator). Without it "the
other monitor" is the one that isn't primary. With one monitor Ghost comes forward on the
half away from the work when it knows where that is.

**`Event::ShowApp { session_id }`**: "Ghost, open the app" (or "show me the window", a quick
intent with no model) asks the desktop to bring its window to the operator. The daemon
publishes it inside a session only; it carries nothing else; the phone does not get it. The
shell raises the window by unmapping and mapping it again, which puts it on the current
workspace and in front on GNOME, where a plain raise would switch workspace or only flash
"Ghost is ready". The tray's Open Ghost and the surface's button do the same.

## What it costs (ADR 0018)

Measured on the workstation (Intel UHD 750, GNOME Shell 50.1, XWayland, two 1920×1080
monitors; `docs/floating-room.md` has the method). At rest: 0.1% of a core for the surface,
no GPU above the desktop's baseline; the loop is stopped and the input shape empty. Forward,
every scene: about 13% of a core for the surface, 9% for Xwayland and 6% for gnome-shell
(the spike measured 32 to 51%, 20 to 29% and 9 to 17%), at 30 frames a second (60 for the
arrival's zoom), the GPU at 58% busy at about 400 MHz. Most of the saving is Wisp drawn at the
scene's frame rate (`Mascot`'s `maxFps`): Wisp redrawn at 60 was half the cost, since every
frame is the monitor's whole surface copied through XWayland. Nothing is added to a turn: the
event is one more broadcast per "open the app", and the quick intent is a phrase match.

## Consequences

- The surface is a second WebKit process while the switch is on: 450 MB resident across its
  three processes in a debug build serving the dev page (shared libraries counted in each);
  a release build was not measured.
- The ADR 0057 rule holds for it: nothing it does opens or closes the microphone.
- A screen share on a desktop that isn't GNOME is not seen; the tray switch is the answer.
- Native Wayland support needs layer-shell (KDE, wlroots) or a GNOME Shell extension that
  stacks the window, both later.
