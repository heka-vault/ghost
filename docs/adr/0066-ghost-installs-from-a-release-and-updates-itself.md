# 0066: Ghost installs from a release and updates itself

- **Status:** accepted
- **Date:** 2026-11-13
- **Amends:** ADR 0038 (the bundle is now what a newcomer installs, and an update replaces the
  app and the daemon it carries together)

The operator asked (2026-11-13): "Can we create releases in GitHub? Can we make it so I can
install a release and get the new user experience, and we work on having a system for
updating? I really like how I can just press a button inside T3 and it updates itself."

Until now the release workflow had never run (it called a script that had moved), a person
with the .deb still needed `scripts/models.sh` from a checkout for Parakeet and Kokoro (the
first run said they would download "when it starts", which nothing did), and nothing told an
installed Ghost a newer one existed.

## Decision

**A release is a tag; the workflow does the rest** (`.github/workflows/release.yml`,
[releasing.md](../releasing.md)). It builds the .deb and the AppImage, signs each for the
updater with the repository's key, checks each signature against the public key the app
carries, installs the .deb into clean Ubuntu 24.04 and 26.04, and publishes the files with
`latest.json`, the update manifest (`tools/release_manifest.py`), in one GitHub release. Run by
hand (`workflow_dispatch`) on any branch it does all of that but publish, and keeps the files
as the run's artifact: the pipeline is proven before a tag exists.

**The app downloads what the bundle does not carry** (`ghostd::speech_models`,
`GET /speech-models`, `POST /speech-models/fetch`). Parakeet (2.5 GB) and Kokoro (340 MB) are
fetched by the daemon from their upstream releases at the revisions `scripts/models.sh` pins,
each into a `.part` beside its place, hashed as it arrives, refused when it is not the pin,
moved into place by one rename, and resumed where it stopped after a failure. The first run's
"Get started" starts it so it runs while the model and the microphone are set up, and the
Hear step and Settings › Sound show its progress and say a failure in plain words with Try
again; `ghostd models fetch` does the same on a desk with no display. The daemon's model load,
which already waited for missing files (ADR 0038), is woken as the download ends. The routes
are the operator's: the phone, outside tools and Ghost's own app have none of this.

**The release carries Ghost's echo canceller** (issue #116, ADR 0064). The installer and its
two templates (`infra/pipewire/`) are bundle resources, found beside the daemon's binary
(`/usr/lib/heka-ghost-desktop/pipewire/`, the AppImage's the same under its mount). The
installer copies the user unit to `~/.config/systemd/user/` and renders its config into
`~/.config/ghost/echo-cancel.conf`; the unit runs the system's own `/usr/bin/pipewire` on it,
so nothing it leaves points into the bundle, an AppImage's moving mount included, and no
root is asked for. The same runner (`ghost_api::echo_cancel`) is behind Settings › Sound's
Install and Remove, the first run's Set it up, Repair and `ghostd echo-cancel on|off|status`
(the checkout's `make echo-cancel`). It first asks the sound server what it is (`pactl info`,
through the devices port): a PulseAudio machine, no sound server, or no `pactl` gets one
sentence and the installer never runs. Repair, pressed by the operator, installs it where
nothing cancels Ghost's voice (the unprotected case) and reinstalls it where Ghost should
listen through it and its unit is gone; an automatic repair only restarts it; on a chip that
cancels echo or a headset it is never offered. After an install or a removal the daemon
reopens the microphone, so the change applies at once. The .deb depends on
`pulseaudio-utils` (`pactl`: the tools, not the PulseAudio server) and recommends
`pipewire-bin`; the AppImage can declare nothing, and there the missing `pactl` is the
sentence that names the package.

**Quitting never ends in a crash report** (issue #115). WebKit's web-content process exits
when the app does, and in Mesa's exit handlers (`libgallium`, software GL in the tests) one
quit in five to eight aborted with `free(): corrupted unsorted chunks`, after the daemon had
stopped cleanly, a fresh start's quit as much as an update's. The app, and the floating
room's process, end their own web-content processes with SIGKILL on the way out
(`webkit_exit.rs`): no exit handler runs, no core is left; the network process, which holds
the window's storage, exits on its own.

**A dead speaker never hangs a turn** (issue #117). A playback write gives up when the
stream has taken nothing for three seconds (`ghost_audio::playback::StallWatch`) and at once
after that until it plays again, a session goes on without its chime, the reply shows on
screen, and the waiting list says once that sound is unavailable (`output:unavailable`, its
place Settings › Sound), clearing when sound plays out again.

**The app updates itself, on the operator's button only** (`updater.rs` in the shell,
`tauri-plugin-updater` 2.14.1). The installed app checks
`https://github.com/heka-vault/ghost/releases/latest/download/latest.json` when it starts (after
45 s, in the background) and once a day, while "Check for updates" is on (the default; a word
in `~/.local/state/ghost/update-check`). A new version is said quietly: "Update" on the rail,
"Update available: 2026.11.1" in its tooltip, and Settings › General's Updates with the
version, what's new (the release notes as plain text), Check now, and **Update and restart**.
The button downloads the new version, verifies its minisign signature against the key in
`tauri.conf.json` (and, with `requireSignedVersion`, that the version the signature was made
for is the one `latest.json` announces, so an older signed file can't be passed off as new), waits while a session or a run is going (and says so, with Not now),
installs it, and restarts through the app's ordinary exit, which stops the daemon it runs
(ADR 0038); the new app starts the new daemon. `~/.config/ghost` and `~/.local/share/ghost`
are not touched; the new daemon migrates its store as on any start. A failure says what
happened and leaves the running version as it was. Only the window calls these commands; no
tool, model, app, phone or outside client can start an install, as none can set a permission.

**Both bundle types update in place.** The plugin's Linux installer, read in its source for
2.14.1, takes the bundle type the bundler stamps into the binary: an AppImage is replaced
where it is (the user owns it; no password), a .deb is written to a temporary file and
installed with `pkexec dpkg -i`, which asks for the operator's password once through the
desktop's polkit agent, falling back to zenity or kdialog and then to `sudo`. So the .deb stays
the recommended install, and the button works for it. One limit of `dpkg -i`: it does not
fetch a dependency a new version adds; such a release says so in its notes and asks for `sudo
apt install ./<file>.deb` instead.

**A development build never offers an update.** A build from the checkout (`make dev`, `cargo
build`) has debug assertions or no bundle type, and Settings says it is a development build.

**Privacy.** The check is one GET to GitHub (which redirects to its file host) with the
plugin's user agent (`tauri-plugin-updater/2.14.1`) and nothing that names the computer or the
operator: no version in the address, no identifier, no cookie. The download, when the operator
presses the button, is one more GET of the file. Settings says this beside the switch.

**Performance.** Nothing on a turn: the check runs once after start and once a day off the
main thread, a request of a few kilobytes. The download of the speech models is a first-run
cost, not a turn's.

## The key

The updater's key pair is minisign's, made by `tauri signer generate` with no password. The
public key is in `tauri.conf.json` (`plugins.updater.pubkey`); the private key is the
repository secret `TAURI_SIGNING_PRIVATE_KEY`, read only by the release workflow, never
printed or committed. Rotating it: generate a new pair, ship a release signed with the old key
whose app carries the new public key, then replace the secret; an app that never installed
that bridge release must be updated by hand ([releasing.md](../releasing.md)).

## Considered

- **Only the AppImage updates itself; the .deb points at the download.** The facts said
  otherwise: the plugin installs a .deb in place with one password prompt, and the .deb is
  what Ubuntu users reach for.
- **The shell downloads the speech models itself.** The daemon already waits for them and
  holds the cache paths and the settings that move them (`GHOST_STT_DIR`,
  `GHOST_TTS_MODEL_PATH`); a desk with no display (ADR 0038's headless mode) needs the same
  download, so it is the daemon's, with the shell only showing it.
- **The speech models in the bundle.** A 3 GB .deb for every update, and every update would
  carry them again. Rejected.
- **An apt repository** (`apt upgrade` updates Ghost). More infrastructure to keep (a signed
  repository, its key in every machine's keyring) for one operator's ask of a button in the
  app. Can come later beside this.
