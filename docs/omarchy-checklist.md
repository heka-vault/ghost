# Ghost on Omarchy: the checklist for a real laptop

What the release workflow's containers cannot show (ADR 0075): Ghost's AppImage on a real Arch
machine with Hyprland, as Omarchy sets it up. Run it top to bottom, note each line's result
(works, works with a change, fails: what it said), and file what fails with `ghostd diagnose`'s
file attached. Do it on a fresh user account if you can, so nothing of an earlier Ghost is there.

## Before

- [ ] `uname -m` is `x86_64`; `. /etc/os-release; echo $ID` is `arch`; `echo $XDG_SESSION_TYPE`
      is `wayland`; `echo $DISPLAY` is set (XWayland is on: floating needs it).
- [ ] `pacman -Q fuse2 pipewire pipewire-pulse wireplumber libpulse gnome-keyring` — note which
      are missing. `fuse2` is needed to run the AppImage (`sudo pacman -S fuse2`, or run it with
      `--appimage-extract-and-run`); `libpulse` gives `pactl`, which the echo canceller and the
      sound checks use.
- [ ] `pacman -Q espeak-ng` — whatever it says, Ghost must not need it (it carries its own).

## Install and first run

- [ ] Download the AppImage from the latest release, check its SHA-256 against the release
      page, `chmod +x`, move it to `~/Applications/`, run it. The window opens (note how long).
- [ ] `~/Applications/heka-ghost-desktop_*.AppImage ghostd version` prints `Ghost <version> ·
      release <commit>`.
- [ ] Settings › General › **Install the ghostd command**; then in a new terminal `ghostd
      check`. Expect `espeak-ng: 1.51, data at /tmp/.mount_…/usr/lib/heka-ghost-desktop/
      espeak-ng-data (the bundle's)` and no "Ghost can't speak". Is `~/.local/bin` on Omarchy's
      PATH in a new terminal?
- [ ] The first run's Get started downloads Parakeet and Kokoro (2.9 GB) with progress.
- [ ] `ghostd models voice-test` prints `voice: rendered …`.
- [ ] Settings › General › Error reports shows nothing sent (a working run sends none).

## Sound

- [ ] The first run's microphone step finds the laptop's microphone; the Hearing test hears you.
- [ ] Ghost's first line plays through the laptop's speakers (`wpctl status` shows which sink).
- [ ] "Hey Ghost" wakes it; a question gets a spoken answer.
- [ ] On speakers, the first run offers the echo canceller: **Set it up**, then `ghostd
      echo-cancel status` and `systemctl --user status ghost-echo-cancel`. Talk over Ghost:
      does it stop for you and not for itself?
- [ ] Music playing in a browser is ducked while Ghost speaks and comes back after.

## The keyring

- [ ] Settings › Model, add a key (any provider's). Does it save? If Ghost's list says the
      keyring is locked or missing: which Secret Service does Omarchy run
      (`busctl --user list | grep -i secret`)? Note whether `gnome-keyring` was already installed
      and unlocked at login, and what Ghost said.
- [ ] Log out and in: is the key still there, and does Ghost start without asking for it?

## Floating through XWayland on Hyprland

The floating room is a second process of the app on XWayland (`GDK_BACKEND=x11`, window class
`heka-ghost-desktop`, title `Ghost`); the main window is native Wayland. It needs to stay above
everything, be on every workspace, take no keyboard focus and let clicks through except on its
buttons.

- [ ] Turn floating on (Settings › General › Where Ghost appears). Say "Hey Ghost". Note:
      does it appear; is it above a tiled window; does focus stay where you typed; can you click
      the window under it; does it follow you to another workspace?
- [ ] If it tiles, takes focus or stays on one workspace, try these rules in
      `~/.config/hypr/hyprland.conf` (Omarchy: `~/.config/hypr/` user overrides), reload with
      `hyprctl reload`, and note which were needed:

      ```
      windowrulev2 = float, class:^(heka-ghost-desktop)$, xwayland:1
      windowrulev2 = pin, class:^(heka-ghost-desktop)$, xwayland:1
      windowrulev2 = nofocus, class:^(heka-ghost-desktop)$, xwayland:1
      windowrulev2 = noinitialfocus, class:^(heka-ghost-desktop)$, xwayland:1
      windowrulev2 = noborder, class:^(heka-ghost-desktop)$, xwayland:1
      windowrulev2 = noshadow, class:^(heka-ghost-desktop)$, xwayland:1
      windowrulev2 = noblur, class:^(heka-ghost-desktop)$, xwayland:1
      windowrulev2 = noanim, class:^(heka-ghost-desktop)$, xwayland:1
      ```

      `hyprctl clients` shows the window's class, title and `xwayland: 1`.
- [ ] Clicks: with the rules above, does a click on an empty part of the floating room reach
      the window under it (Ghost sets an empty X11 input shape there)? If not, note it: Hyprland
      may not honour XWayland input shapes, and floating would need another way there.
- [ ] Which monitor it picks with two screens, and whether it moves to where you work.

## Screen sharing

- [ ] Share your screen in a browser call (`xdg-desktop-portal-hyprland`). Ghost hides the
      floating room during a share only where it can see one: today it reads GNOME's Mutter
      over D-Bus, which Hyprland does not have. Expect it to stay visible; note it, and whether
      `hyprctl` or the portal tell us a share is on.

## The tray and Start at login

- [ ] Waybar shows Ghost's tray icon (Waybar's `tray` module, a StatusNotifier host); its menu
      opens and Quit stops Ghost (`pgrep -fa ghostd` shows nothing after).
- [ ] Closing the window keeps Ghost in the tray; the icon reopens it.
- [ ] Tray › Start at login on; log out and in. Does Ghost start (Hyprland does not read
      `~/.config/autostart` by itself; Omarchy may run a helper such as `dex` or `uwsm`'s
      autostart, or it may not)? If not, note it, and that `exec-once =
      ~/Applications/heka-ghost-desktop_*.AppImage --hidden` in `hyprland.conf` is the manual way.

## The AppImage's own update

- [ ] With an older release installed and a newer one published: Settings › About shows Update
      available; **Update and restart** replaces the file in `~/Applications/` and comes back on
      the new version (`ghostd version`). Does `~/.local/bin/ghostd` still run it (same path)?

## Uninstall

- [ ] `ghostd uninstall --dry-run` lists every kind, including the `ghostd` command in
      `~/.local/bin`.
- [ ] Settings › General › Remove Ghost from this computer, everything chosen: the app quits, a
      notification says what was removed, `~/.local/bin/ghostd` is gone, and the last line names
      the AppImage file to delete.
- [ ] `ls ~/.config/ghost ~/.local/share/ghost ~/.cache/ghost ~/.config/autostart` shows nothing
      of Ghost's; `systemctl --user list-unit-files | grep ghost` is empty.
