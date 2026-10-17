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
