# PipeWire echo cancellation

Ghost's microphone hears the speakers. Without echo cancellation the wake word
and VAD trigger on music and on Ghost's own voice. `module-echo-cancel` (WebRTC
AEC, shipped with PipeWire) creates a cleaned source and a virtual sink. Only
the capture side needs to go through it: set the cleaned source as the default
source, keep the real speakers as the default sink, and point the module's
reference at the speakers' monitor.

Install (user scope; no root):

```bash
mkdir -p ~/.config/pipewire/pipewire.conf.d
cp infra/pipewire/90-ghost-echo-cancel.conf ~/.config/pipewire/pipewire.conf.d/
systemctl --user restart pipewire pipewire-pulse wireplumber
pactl list sources short | grep ghost   # ghost.mic should appear
pactl set-default-source ghost.mic
```

Edit `capture.props node.target` and `source.props` if the microphone or
speakers change (`pactl list sources short` / `sinks short`). The reference
sink is left as the real output so F8/F9, Spotify, and `ghostd`'s per-stream
ducking keep working unchanged.
