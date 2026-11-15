# 0071: The person removes what Ghost left; the package deletes nothing

- **Status:** accepted
- **Date:** 2026-11-14
- **Amends:** ADR 0066 (the .deb gains a `prerm` that says what stays; the echo canceller's
  unit is conditioned on Ghost being installed), ADR 0038 (the app's Settings can quit it to
  remove it)

People who installed 2026.11.1 said (issue #147): "The deb ships no maintainer scripts, so the
user-level leftovers (echo-cancel service, autostart entry, config, 2.8G model cache, database)
will need manual removal", and the agent guide's Undo missed
`~/.config/autostart/heka-ghost-desktop.desktop` and the window's storage,
`~/.local/share/dev.heka.ghost`. The owner chose all four of: a command and the same in
Settings, a note when the package is removed, the echo canceller stopping by itself once Ghost
is gone, and the documents.

## Decision

**One list, in code, from a real install** (`ghost_core::leftovers`). Every file, folder, unit
and keyring item Ghost or the libraries it runs write under a person's own folders, typed and
grouped into six kinds the person chooses among, each with its size and a plain sentence: what
keeps running or starts by itself (the echo canceller's service, Start at login, the GNOME
Shell helper, a checkout's `ghostd.service`); Ghost's entries in other tools (through
`other_tools`' unlink, with Claude Code's own command shown when `claude` is absent, the
backups Ghost made beside their files, and the client keys); the speech models' cache; the
app's own storage (the webview's folders under `dev.heka.ghost`, `~/.local/state/ghost`, the
runtime folder); the person's data; and the vault with its key in the login keyring. It was
taken from installing the published 2026.11.1 .deb in clean Ubuntu 24.04 and 26.04 and using
it, then diffing the home folder, and from reading the code for what a container cannot
exercise. Tests hold it there: a real `ghostd run` in a temp home writes nothing the list does
not name; every well-known folder named in the source is in it or named as read and never
written; the README's by-hand list names every path in it.

**`ghostd uninstall`, and the same in Settings › General.** With no option it prints the list
and asks per kind, No by default, the person's data and passwords last, each after its
warning; `--dry-run` only lists; `--all` asks once (`--yes` not at all); `--keep-data` takes
all but the data and the vault. It stops this Ghost first (its service, the app, which stops
the daemon it carries and the floating room, then any `ghostd` left), only processes whose own
environment gives them the data folder being removed (another Ghost on the same account, with
folders of its own, is never signalled); a Ghost process whose environment it can't read or
that names no folder makes it refuse before anything is signalled, since it never guesses; and
it refuses with a sentence if one will not stop. It needs no root and never runs `apt`: it ends with the one line the person runs
(`sudo apt remove heka-ghost-desktop`, or the AppImage to delete). It removes only paths the
list names, under the person's home or runtime folder, never the folder itself, never through
a link out of them (a link is removed as a link), refuses an empty or `/` home, and is
idempotent. Settings › General's **Remove Ghost from this computer** shows the same list from
`GET /uninstall` (the operator's only), the same choices and warnings, a confirmation naming
what goes and the finishing command; confirming hands the choice to the shell, which copies
`ghostd` out of the bundle, starts `ghostd uninstall --only … --yes --after <pid>` on its own
and quits, since a window cannot remove its own storage. No tool of Ghost's own app, outside
client, phone route, model or run can list, start or choose any of it.

**The package deletes nothing in a home folder.** A package's scripts run as root for every
account on the machine; deleting a person's transcripts and passwords on `apt remove`, or
worse on an upgrade, would be the wrong default, and Debian's convention agrees. The .deb's
`prerm` (Tauri's `preRemoveScript`) deletes and stops nothing; on `remove` only it prints two
lines: what stays, and that `ghostd uninstall` removes it (or the README's list once the
package is gone). On an upgrade, which the app's own Update and restart is, it says nothing.
`DEBIAN_FRONTEND` governs debconf's questions, not a script's notes, so it prints under
noninteractive too.

**The echo canceller stops by itself once Ghost is gone.** Its unit gains
`ConditionPathExists=%S/ghost/installed`, a link Ghost keeps pointing at its installed `ghostd`
(`/usr/bin/ghostd` from the .deb) or, from an AppImage, at the AppImage file itself (its mount
moves at every start; the file does not). The installer writes the link
(`GHOST_INSTALLED`, else itself), and the daemon refreshes it at every start, so a moved
AppImage only skips the canceller until the next launch, and a removed package or deleted
AppImage leaves the link dangling: systemd skips the unit and the default microphone is the
system's again. The unit is rewritten at the daemon's start when it is Ghost's own (its
marker line, or byte for byte the form 2026.11.0 and 2026.11.1 wrote), never when the person
edited it and took the marker out.

## Considered

- **A `postrm purge` that deletes every user's Ghost folders.** Rejected: root deleting in home
  folders it does not own, and a purge typed by habit would take passwords that cannot be had
  back.
- **The echo canceller's unit conditioned on `/usr/bin/ghostd`.** Simple for the .deb and wrong
  for an AppImage, whose binary is only there while it runs; the link serves both.
- **The removal done by the daemon over the control API.** The daemon holds the database open
  and the window holds its own storage; removing them from inside either leaves a half-removed
  Ghost writing its last lines. The removal runs after both are gone.

## Consequences

- `ghostd uninstall`, `GET /uninstall`, Settings › General's last section, the `prerm`, the
  unit's condition and its refresh. The release workflow's install test removes the package
  and checks the note is printed on remove and not on an upgrade, and that no file under a
  person's home changed.
- A new path Ghost writes under a home folder goes into `leftovers` in the same change; the
  tests above fail otherwise.
- The journal keeps Ghost's log lines until the system rotates them; Mesa's shader cache is
  every app's. Neither is Ghost's to remove, and the README says so.
