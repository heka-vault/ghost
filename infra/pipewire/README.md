# PipeWire echo cancellation

Ghost listens on `ghost.mic`, a source produced by PipeWire's WebRTC echo
canceller. It subtracts what the speakers are playing from what the microphone
hears, so neither Ghost's own voice nor music comes back as the operator's turn
or as a wake word. This is the primary defence against self-interruption; the
pipeline's word-count barge-in rule is only the backstop.

The module runs in its own small PipeWire client process,
`ghost-echo-cancel.service` (a `systemd --user` unit running `pipewire -c`),
not inside the main daemon. Installing, updating, or removing it therefore never
restarts PipeWire, and nothing else playing is interrupted.

```bash
infra/pipewire/install.sh --dry-run   # show the rendered config
infra/pipewire/install.sh             # render, install the unit, start it, verify ghost.mic
infra/pipewire/install.sh --sink-drives   # also make the speakers keep time (see Two clocks)
infra/pipewire/install.sh --uninstall
systemctl --user status ghost-echo-cancel
```

Device names never go in this repository. The installer takes them from
`GHOST_MIC_NODE` and `GHOST_SINK_NODE`, falling back to the current PipeWire
defaults, and renders them into `~/.config/ghost/echo-cancel.conf`. Then set
`GHOST_INPUT_DEVICE=ghost.mic` for `ghostd`. Re-run the installer after
changing microphone or speakers.

Requires `libspa-aec-webrtc` (package `pipewire-audio` or `libspa-0.2-modules`
depending on the distribution).

## Two clocks

The echo canceller reads the microphone and the sink's monitor in one PipeWire
graph, and a graph runs off one device's clock, the *driver*. When the mic and
the speakers are different devices (a USB mic array and the onboard output,
say), their two hardware clocks drift apart, and PipeWire resyncs whichever
device is following every few seconds. Without Ghost the two would be separate
graphs and nothing would be resynced.

PipeWire prefers a capture device as the driver, so by default the mic keeps time
and the speakers follow. Each resync of the speakers is an audible click, and
under music it sounds like crackle. The installer notices when the mic and the
sink are different devices and says so.

`install.sh --sink-drives` turns it round. It writes
`~/.config/wireplumber/wireplumber.conf.d/51-ghost-sink-driver.conf`, which raises
the sink's `priority.driver`, and restarts WirePlumber, so playback stops for a
moment. A re-run keeps the rule and re-renders it for the current sink, and
`--uninstall` removes it. The follower is now the mic: `ghost.mic` loses a few
milliseconds at each resync. The echo canceller sees a jump either way, since
before it was its reference that skipped. Whether the gaps cost wake words or
echo leakage is a question for the real-mic bench, not a guess.

To see which device keeps time and how often the follower is resynced:

```bash
pw-top -b -n 2                     # the driver is the unindented row; its followers are the "+" rows under it
journalctl --user --since -1min | grep -c 'follower.*resync'
```

The other way out is one clock for both: play through an output on the mic's
own device. That only suits music if the device plays at a full rate; a USB array
whose firmware offers 16 kHz alone would sound muffled.

What was tried on the reSpeaker XVF3800 (16 kHz only, USB synchronous endpoints;
2026-10-27), with the speakers driving. It is resynced every ~2 s whatever the
setting:

- `api.alsa.period-size` and `api.alsa.headroom` 256: still every ~2 s.
- `api.alsa.disable-batch`: still every ~2 s.
- `api.alsa.disable-tsched`: no resyncs, but capture failed with I/O errors.
- Graph quantum 480 instead of 256, so a cycle is a whole number of 16 kHz
  frames: still every ~2 s (16 resyncs in 30 s, against 15 at 256).
