# 0075: The bundle carries what its libraries need; the distributions Ghost supports

- **Status:** accepted
- **Date:** 2026-11-15
- **Amends:** ADR 0038 (the daemon ships inside the app), ADR 0066 (releases and their install
  tests), ADR 0067 (what a speech-model failure reports), ADR 0071 (what `ghostd uninstall`
  lists)

A tester on Arch could not get Ghost to speak (#168). The AppImage carries Ubuntu's
`libespeak-ng.so.1` (1.51), which looks for its data at `/usr/lib/x86_64-linux-gnu/espeak-ng-data`,
where Ubuntu's `espeak-ng-data` package puts it; Arch keeps its own at `/usr/share/espeak-ng-data`
and for 1.52. Ghost initialised espeak-ng through `espeak_Initialize`, which ends the process
with `exit(1)` when its data is missing, so the daemon died with one line from espeak and the
app started it again, forever. The .deb worked only because it depends on `libespeak-ng1`, which
brings the data. Error tracking had nothing: the failure happened only after the 2.9 GB speech
models were downloaded, it killed the process before any report was built, and a report of
`SpeechModelsLoad` would not have said which model failed anyway.

## Decision

**The bundle carries the data its own libraries read, made for them, and Ghost names the folder.**

- `scripts/espeak.sh stage` (run by `scripts/sidecars.sh`) copies the build machine's
  `espeak-ng-data`, checked by `dpkg-query` to be the same package version as the
  `libespeak-ng1` the AppImage carries, into the bundle's resources
  (`<bin>/../lib/heka-ghost-desktop/espeak-ng-data`), with its Debian copyright file, a `SOURCE`
  note naming the source package, and a stamp (`GHOST-ESPEAK-NG-VERSION`) naming the release.
  English only, since Ghost phonemizes `en-us` alone (`ghost_voice::tts::LANG`, held by a test):
  2.6 MB of the folder's 25 MB, about 0.5 MB compressed in each package.
- `ghost_infer::espeak` chooses the folder, in order: `GHOST_ESPEAK_DATA`; the bundle's copy when
  its stamp is the loaded library's release; the system's folders
  (`/usr/lib/<arch>-linux-gnu/espeak-ng-data`, `/usr/share/espeak-ng-data`) when the loaded
  library is the system's own (read from `/proc/self/maps`); the library's default. A bundled
  library is never given the system's data. It initialises through `espeak_ng_Initialize`, which
  returns a status, so data that is missing, unreadable or for another version is a typed error
  (`InferError::EspeakData`) naming the library and the folder, never an exit.
- **The .deb keeps depending on `libespeak-ng1`.** Its `ghostd` has no RUNPATH and loads the
  system's library, so the system's data, which that package brings at the same version, is the
  one that fits. It carries the bundle's copy too (one list of resources for both packages); the
  stamp means it is used only where the versions match (Ubuntu 24.04), where it is the same
  files.
- **Licence.** espeak-ng and its data are GPL-3.0-or-later. The AppImage already carried the
  library with its Debian copyright file (linuxdeploy copies it); the data travels the same way,
  with `COPYRIGHT` and `SOURCE` beside it, and the source is Ubuntu's `espeak-ng` source package
  at the version `SOURCE` names. `deny.toml` governs Rust crates and is not affected: no crate
  links GPL code; the daemon links the system's or the bundle's C library at run time, as before.

**A voice that cannot start is checked first, said plainly and reported by kind.** The daemon
starts espeak-ng (milliseconds) before its speech models load, so the problem is known before
2.9 GB are downloaded for nothing. It is its own waiting-list item (`voice:unavailable`, place
Sound) that starts with what to do, and `ghostd check` says the same; `ghostd models
voice-test` renders one fixed sentence into memory, never played. The `SpeechModelsLoad` report
(ADR 0067) carries closed words only: `part` (`espeak`, `kokoro`, `parakeet`, `onnxruntime`,
`silero`, `smart_turn`, `wake`), `error` (Ghost's `InferError` variant name) and, for espeak,
`espeak` (`data_missing`, `data_version_mismatch`, `data_unreadable`; `library_missing` is the
app's to see, as the daemon's exit 127, since a missing library stops the loader before
`main`). Those facts join the fingerprint, so each is its own issue; a failure without them
keeps the print it had. A fact value that starts like a path (`/`, `~`, `.`) is `other`. The
diagnostic report (ADR 0073) carries the same words under `speech_models.voice`.

**The AppImage has a `ghostd`.** It takes `ghostd` as its first argument and hands the rest to
the bundled binary (`Ghost.AppImage ghostd check`). `ghostd command install` (and Settings ›
General's **Install the ghostd command**, shown only in an AppImage) writes
`~/.local/bin/ghostd`, a script with a marker line that runs the AppImage by its path;
`ghost_core::ghostd_command` is its one writer, never over a file that is not Ghost's. Outside
tools that start a command are given it (`stable_ghostd`). `ghostd uninstall` lists it under
Ghost's entries in other tools, only when the file there is Ghost's.

**Supported distributions.** x86-64 only. By the .deb: Ubuntu 24.04 and 26.04 and Debian 13.
By the AppImage: Arch, Fedora and others with an ordinary desktop (PipeWire's library, ALSA,
X11 or XWayland, Mesa, fontconfig, FreeType, HarfBuzz, FriBidi, Wayland's client library) and
FUSE 2 to mount it. **Debian 12 and older are not supported**: the release is built on Ubuntu
24.04 on purpose (docs/releasing.md), and its binaries need glibc 2.38 and 2.39, which Debian
12 (2.36) lacks. The build base does not move for it.

**Tested in every release.** `release.yml` installs the .deb in clean Ubuntu 24.04 and 26.04 and
Debian 13 and checks espeak-ng starts with data that fits; it runs `scripts/distro-check.sh` in
clean `archlinux:latest` and `fedora:latest` containers on the extracted AppImage: every binary
finds its libraries, `ghostd check` finds the bundle's runtime, model files and espeak-ng data,
Kokoro renders a sentence (its two files fetched by `scripts/models.sh kokoro`, pinned), and a
copy without the espeak-ng data, on its first run, is said and reported with `part: espeak`,
`espeak: data_missing` and `installed_as: appimage` to a stub on the container's loopback.

## Consequences

- A distribution that keeps espeak-ng's data elsewhere, or has none, gets Ghost's voice from the
  AppImage. A person can still point Ghost at another folder with `GHOST_ESPEAK_DATA`.
- The release workflow takes a few minutes more (Debian 13's install, two container runs and
  Kokoro's 340 MB download); the dry run measures it.
- What a container cannot show waits for a real machine: floating through XWayland on Hyprland,
  the tray in Waybar, Start at login, the keyring, screen-share detection, the AppImage's own
  update. `docs/omarchy-checklist.md` is the list.
- Amended 2026-11-16: whether Ghost runs from an AppImage is asked of one function,
  `ghost_core::appimage`, and the answer is yes only when the running binary is inside
  `$APPDIR`. The runtime's `APPIMAGE` and `APPDIR` are inherited by everything an AppImage
  starts, and a person's editor or terminal can be one (Cursor, T3 Code): a .deb's or a
  checkout's `ghostd` run from its shell, as a coding agent following the setup guide runs it,
  took the editor's file for Ghost's. `ghostd uninstall` ended with `rm` of the editor,
  `ghostd echo-cancel on` pointed the canceller's `installed` link at it, `ghostd mcp add
  --command` refused the bridge, and an error report said `appimage`. The copy of `ghostd` the
  app runs from the runtime folder to remove Ghost is outside the mount, so the app names its
  AppImage to it (`ghostd uninstall --after <pid> --appimage <file>`, refused without
  `--after`). `ghost_reports` keeps the same rule in its own words, held to the core's by
  `crates/ghostd/tests/appimage.rs`, and a test lists every source file that names either
  variable.
