# ghost-audio

Ghost's audio engine (ADR 0039; W4 of heka-vault/ghost#39). It replaced the Python daemon's
`ghost_voice.transport` (a sounddevice port of Pipecat's PyAudio transport), `ghostd.audio`
(the `Ducker` and the chime), `ghost_voice.sounds`, and the mixer under
`ghost_voice.thinking`, which are in history now (W10), on PipeWire through the project's own
Rust binding.

The rules hold here as everywhere (CLAUDE.md): idle audio stays on the machine, and nothing
from the microphone is logged or stored. The crate writes samples only into a ring the daemon
reads; its counters are numbers, never audio; `GHOST_DEBUG_AUDIO` is the daemon's one
exception and nothing here reads it.

## What is here

| Module | What it does |
| --- | --- |
| `capture` | `Capture`: a PipeWire stream on the node named `ghost.mic` (the echo canceller's output, `infra/pipewire`), 16 kHz mono s16, asking for a 20 ms quantum, on PipeWire's own thread. The process callback copies into a lock-free ring and allocates nothing; `next_frame().await` / `frames()` give 20 ms `Frame`s to tokio. A missing node is an error that names it; `fallback_to_default` turns that into a warning and the default source. |
| `playback` | `Playback`: a stream to the default sink or the one named, 24 kHz mono s16, fed from a ring the daemon writes. `clear()` is barge-in (what is queued is dropped within one quantum); `drain()` resolves when what was queued has played; a `MixerHandle` switches the thinking sound on under speech at `GHOST_THINKING_VOLUME`. |
| `ring` | The `rtrb` ring helpers the callbacks run: chunked moves, no allocation; what does not fit is dropped and counted. |
| `wake` | The wakeup from PipeWire's thread to tokio: an `eventfd`, written from the callback (no user-space lock), read through `AsyncFd`. |
| `mixer` | Pipecat's `SoundfileMixer` arithmetic: `clip(audio + sound * volume)` in float64, truncated to s16, the sound restarting from its beginning when a chunk would run past its end. |
| `duck` | `Ducker` and `restore_from_state`: the matching music streams lowered for a session and put back to their exact previous volumes, with `duck.json` under the runtime dir so a crash leaves nothing ducked. |
| `sounds` | The chime (two sine notes, 24 kHz) and the thinking loop, rendered into `~/.cache/ghost/thinking-v2-<rate>.wav` once (the file the Python daemon made) and loaded by `hound`. |
| `pw` | The PipeWire plumbing: a `Session` (main loop, core, round trips), the registry's nodes, `Props` and `Format` pods. |
| `config` | `AudioSettings::from_env()`: the `GHOST_*` names the Python daemon read. |

Settings, with the Python defaults: `GHOST_INPUT_DEVICE` (`ghost.mic`; a `node.name`, not a
PortAudio index), `GHOST_OUTPUT_DEVICE` (unset: the default sink), `GHOST_DUCK_APPS`
(`["spotify"]`), `GHOST_DUCK_LEVEL` (0.25), `GHOST_CHIME` (true), `GHOST_THINKING_SOUND`
(true), `GHOST_THINKING_VOLUME` (0.12), `GHOST_THINKING_DELAY_SECS` (0.5), `GHOST_CACHE_DIR`.

### Ducking: native PipeWire, the Python's file

The ducker goes through PipeWire's registry and the node `Props` param (`channelVolumes`,
what `wpctl set-volume` sets), not the pulse protocol. The state file keeps the Python's shape
and meaning so a file either daemon wrote restores under the other: keyed by the stream's
`object.serial` (the number `pactl` shows as the sink-input index; the node id is a different
number), values are pulse-scale fractions, the cube root of PipeWire's linear volume (what
`pactl` shows as a percentage). The cube root and the cube in f64 recover every f32 bit for
bit (a unit test checks two million), so the restore is exact. The file is written before any
volume changes, so a crash in between still restores.

PipeWire does not expose pulse's cork. A paused player's node is `idle`, the same as a silent
uncorked one (checked with mpv on 2026-10-28), so where the Python skipped `si.corked`, this
ducks idle streams along with running ones; they come back exactly, so it is harmless.

## The probe

`audio-probe` (`cargo build --release -p ghost-audio --example audio-probe`) works the engine
against the live graph. Play only into a null sink, never the operator's speakers:

```bash
pactl load-module module-null-sink sink_name=ghost-test   # prints the module id
P=target/release/examples/audio-probe
$P list                                   # nodes, serials, states, formats, volumes
$P capture --seconds 10                   # frames from ghost.mic: levels and counts, nothing stored
$P loopback --seconds 10 --sink ghost-test
$P duck --pattern spotify --seconds 2     # ducks, restores, and checks the restore is bit-exact
$P chime --sink ghost-test --thinking 2   # the chime, then 2 s of the thinking loop
pactl unload-module <module id>
```

## Measurements (2026-10-28, this workstation)

PipeWire 1.6.2, the graph at 256/48000 (5.3 ms cycles, so a 16 kHz stream gets 85 samples a
callback and the ring assembles the 20 ms frames), `ghost.mic` from `ghost-echo-cancel`,
release build, the data thread at SCHED_RR 20 through RTKit (`libpipewire-module-rt`).

**No xruns in 10 minutes of capture.** Quiet run (nothing else started on the box):
600 s, 29,975 frames, 112,407 callbacks, 9,592,037 samples, 0 overruns, 0 empty callbacks, `pw-top` ERR 0 on the `ghost-capture` node, 0 xrun lines in the journal (the box was not idle: load average 4.5 from the operator's own work). A first 10-minute run, deliberately under load (clippy, the test builds and
`cargo deny` ran during it, load average 4), ended with 0 overruns and 0 empty callbacks and
the ring's worst lag at 5 ms, but `pw-top` showed ERR 2 on the `ghost-capture` node, both in the
first build burst. The one lock on the callback path was `tokio::sync::Notify::notify_one`,
which takes a mutex while a waiter is registered (nearly always), so a preempted tokio thread
could stall the realtime thread; it was replaced by the `eventfd` in `wake`.

**Loopback under 40 ms.** `loopback --seconds 10 --sink ghost-test`, 500 frames, 0 skipped:
engine latency from the capture callback that completed a frame to the playback callback that
took its first sample, mean 6.7 ms, p50 5.6 ms, p95 9.8 ms, p99 10.5 ms, max 10.75 ms. The
rest of the frame follows at the device's rate, so to its last sample add the frame itself,
19.9 ms: 30.7 ms at the worst. PipeWire's own reported delays were 352 samples (22 ms) on the
capture side, the echo canceller's path and the same for any client of `ghost.mic`, and 0 on
the null sink. The loopback holds its queue to one frame: both streams run off one clock, so a
backlog from any hiccup (the mic's resync, a late wake) would otherwise stay forever, and an
earlier unbounded run crept from 26 ms to 40 ms when it did.

**A ducked stream restored to its exact previous volume.** A `pw-play` tone named
`ghost-duck-test` on the null sink, set to 34 % with `pactl`: `channelVolumes` 0.03930273
before, 0.015625 while ducked to 0.25 (`pactl` 25 %), 0.03930273 after, bit for bit; `pactl`
showed `22282 / 34% / -28.11 dB` before and after; `duck.json` held `{"<serial>":[0.3399963…]}`
in between and was gone after.

The chime (4,320 samples, 180 ms) wrote and drained in 197 ms; the thinking loop mixed at 0.12
for 2 s with 0 underruns.

## Tests

```bash
cargo nextest run -p ghost-audio                                  # 35 unit tests, no PipeWire
cargo nextest run -p ghost-audio --run-ignored ignored-only       # 5 against the live graph
```

The ignored tests need PipeWire, `ghost.mic` (`ghost-echo-cancel` running), `pactl` and
`pw-play`; each loads its own null sink and unloads it. They count frames and read volumes
back; nothing from the microphone is stored or printed.

## Crates

`pipewire` 0.10 and `libspa` 0.10 (the PipeWire project's own bindings, feature `v1_2_0`),
`rtrb` 0.4 (the ring), `hound` 3.5 (WAV), `rustix` (the eventfd), `tokio`, `futures`,
`serde_json` (the state file), `thiserror`, `tracing`.

## Not here

The pipeline's thinking-sound state machine (`ThinkingSoundProcessor`: when the loop starts
and stops) belongs with the pipeline; this crate gives it the switch. The daemon's
`restore-volumes` command and the `ExecStopPost` hook call `restore_from_state`. The chime goes
through `Playback` rather than its own stream, so the daemon writes it and `drain`s before the
session's first turn. `GHOST_INPUT_DEVICE` as a PortAudio index is gone; it is a `node.name`.
