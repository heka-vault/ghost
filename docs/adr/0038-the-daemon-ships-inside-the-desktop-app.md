# 0038: The daemon ships inside the desktop app, and Quit stops everything

- **Status:** accepted (decided 2026-10-28; built in W9 of heka-vault/ghost#39)
- **Date:** 2026-10-28
- **Supersedes:** ADR 0009 for the daemon (the `ghostd` user unit becomes the headless mode); the
  PipeWire echo-cancel unit stays a systemd user service
- **Amends:** ADR 0013 (the desktop app no longer enables a service on first run; it is the
  process that runs Ghost), ADR 0001 (Ghost is up while the operator is logged in and the
  app runs, not while the workstation is up)

The operator's main ask: one app that contains Ghost. Until now the daemon was a systemd
user service and the desktop app a client of its socket, installed as separate packages, so
"is Ghost running?" had two answers and "stop Ghost" meant a terminal.

## Decision

**One bundle.** `tauri build` produces one .deb and one AppImage carrying the desktop app and
`ghostd`, built from the same Cargo workspace (ADR 0039), the daemon as an external binary
beside the shell.

**The shell owns the daemon's life.** At launch the shell looks for a daemon already answering
on the control socket (`make dev`'s, or one started by hand) and adopts it. Otherwise it runs
`ghostd restore-volumes` (music a crash left ducked comes back), spawns `ghostd run` in its own
process group with `settings.env`, pipes the daemon's output through `systemd-cat -t ghostd` so
`journalctl --user -t ghostd` keeps working, and restarts it with backoff if it exits. On Quit
the shell sends TERM to the group, waits up to 20 seconds, then KILL, then `restore-volumes`.
The daemon also exits on its own when its parent is gone, so a crashed shell leaves no orphan
on the microphone.

**Quit stops everything.** The desk's microphone, the phone over WireGuard, the webhook
listener, the polled watches and the routes all run while the app runs and stop when it quits.
The app autostarts at login and lives in the tray, so Ghost is up whenever the operator is
logged in; closing the window hides it; Quit in the tray is the one off switch.

**A headless mode remains.** The same `ghostd` binary runs under the user unit for a desk with no
display or for the operator who wants the observers up with the app closed; nothing in the
daemon knows which way it was started beyond `GHOST_LAUNCHER`, which only changes what the
desktop says about restarting.

## Considered

- **The app as a switch for the service** (`systemctl --user start` on launch, `stop` on Quit).
  Keeps systemd's restart and journal for free, but two packages, a unit to install, and a
  daemon that outlives a crashed app. Rejected for the operator's ask; its one advantage, the
  journal, is kept through `systemd-cat`.
- **The daemon as a crate inside the Tauri process.** One process, no supervision. Rejected: a
  WebKitGTK fault would end a session and leave music ducked, the vault's values would share
  an address space with a web renderer, and the daemon could not run headless.
- **Only the desk microphone following the app**, the observers staying a service. The
  operator chose one thing to start and stop; the headless mode covers the other case.

## Consequences

- `make dev` keeps running the checkout's daemon and the desktop; the shell adopts that daemon
  rather than spawning one. `make deploy` builds with cargo and the app prefers the runtime it
  points at when present.
- The `start_daemon` command in the shell and the `heka-ghostd` package go. The .deb the
  release workflow builds is the desktop bundle.
- Two Ghosts never share the microphone: the shell adopts before it spawns, the daemon refuses
  a socket another answers on (`ghost_api::server::bind`), and `dev.sh preflight` stays.
- Logs reach the journal under the `ghostd` tag, as before, and `GHOST_DEBUG_AUDIO` is still
  set by hand for a session, never in the bundle.
- ADR 0027's listener and ADR 0020's phone path are reachable only while the app runs; the
  operator accepted that on 2026-10-28.
