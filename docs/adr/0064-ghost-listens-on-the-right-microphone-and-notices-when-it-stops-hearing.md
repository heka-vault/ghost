# 0064: Ghost listens on the right microphone for the machine, and notices when it stops hearing

- **Status:** accepted
- **Date:** 2026-11-13
- **Issues:** #100 (the wake on the array's own stream), #58 (`ghost.mic` goes deaf over the day), #26 (noticing a microphone that drops audio), #52, #54

Until now Ghost listened on one input, `ghost.mic` by default: PipeWire's WebRTC echo canceller, installed by `infra/pipewire/install.sh`, over whatever microphone the machine had. On the operator's desk that is a reSpeaker XVF3800, which cancels echo on its own chip against what it plays, and Ghost plays through it. The host canceller ran on a signal that was already clean:

- Over a day it ran up tens of thousands of xruns and drifted until it chopped the operator's words; the wake's scores fell and Parakeet read nothing from loud windows. A restart brought it back, and the restart could move the graph to 48 kHz and silence the output (#58).
- Under it, the wake detector heard 10 to 16 of the operator's 21 recorded takes; on the array's own stream, 19 to 20 ([2026-11-12 wake check](../benchmarks/2026-11-12-wake-check.md)).

## Decision

**Ghost listens on what the machine needs, read from PipeWire.** `ghostd::microphone` reads the nodes and the default devices (`pw-dump`) and puts the machine in one of four cases:

| Case | What it is | What Ghost listens on, unset |
|---|---|---|
| on-chip | The microphone cancels echo on its own chip, and Ghost plays through the same device: a mic array, a conference puck, a speakerphone | The device directly. An array's processed channel is taken alone (`node#0`): mixed down to mono, its raw microphone beside the beam would bring the echo back |
| headset | Ghost's voice is in the operator's ears | The headset |
| host-canceller | Ghost's canceller (`ghost.mic`) runs over a microphone that doesn't cancel | `ghost.mic` |
| unprotected | Nothing cancels Ghost's voice: a laptop's microphone and speakers, a webcam with desk speakers | The default microphone, with a sentence that Ghost may hear itself and that headphones (or the canceller) help |

- **On-chip detection** is a short list of device names (`ghost_audio::input::ON_CHIP_CANCELLERS`) and the same `device.id` for the microphone and Ghost's output. It is a recommendation and a default, never a switch over the operator's setting.
- **Two inputs.** `GHOST_INPUT_DEVICE` is what a session hears; `GHOST_WAKE_INPUT_DEVICE` is what the wake detector and Parakeet's check of a weak hit hear. Unset, the wake hears the session's input. Both take `node` or `node#N` (one channel taken alone, `ghost_audio::InputSpec`).
- **The settings win.** Ghost suggests a better choice (`ghostd check`, Settings › Sound, the first run's microphone step) and never makes it itself.
- **Toward working, never toward silence.** A setting that names a device that is not there falls back to the case's own choice, and says so. Without PipeWire's tools the settings are taken as given, and nothing named means `ghost.mic` when it is there, else the default source (`CaptureConfig::for_input`).
- **The streams.** The wake's capture closes, and is awaited, before a session opens its own: two inputs are never open at once. Not listening (ADR 0057) builds no passive pipeline, so neither opens.
- **The hand-over at the wake (#54).** The words after "hey Ghost" are read from the wake's own stream up to the operator's pause, as before. They become the first turn whatever the session hears. Nothing more is lost at the switch than at today's close-and-reopen of one device.
- **The installer.** `install.sh` recognises the on-chip case and installs nothing unless `--force` is given.

**Ghost notices when the microphone stops hearing.** The passive stream passes a meter: per 3 s window, the share of exact zeros, the median run between them, and the level. Numbers only; no sample is kept. Each check of a weak wake hit reports its level, how many words Parakeet read, and whether it was the final read. From these, `microphone::Health` decides:

- **Dropping**: windows chopped into pieces of 32 samples or less with 30% zeros or more, for 30 s. That is the signature of a USB link losing packets (#26). A canceller's gate leaves long runs and is not taken for it.
  - The note says to plug the device into the computer, not a hub.
  - Repair can't fix it and isn't offered as the fix.
- **Deaf**: three final checks at −35 dBFS or louder that read no words, within 20 minutes; a loud check that reads words clears it (#58).
  - A desk session's reads count the same way (2026-11-13): each segment's level and word count, never its words or audio (`stt::OnRead`); not a segment heard while Ghost spoke, and not the phone's.
  - The note says Repair restarts the echo canceller (when it is in use) and reopens the microphone.
  - When the hardware cancels on chip, the note adds that Ghost can listen to it directly.

A problem is a note of Ghost's own on the waiting list (`input:degraded`), shown and never said, and it leads to Settings › Sound (`WaitingPlace::Sound`).

**Repair** (`POST /microphone/repair`, the note's button and Settings › Sound) restarts `ghost-echo-cancel` when an input is the canceller. It pins `clock.force-rate` to the microphone's rate while the canceller is gone, so the graph can't move to 48 kHz under the devices. Then it unpins and reopens the passive microphone. It is refused while a session is open (its stream would go with it) and while not listening. With `GHOST_INPUT_REPAIR=auto`, the operator's choice, a deaf input is repaired by itself while no session is open, at most once an hour. A dropping link is never repaired by itself, since a restart can't help it.

`ghostd check` prints the case, both inputs, the advice and the canceller's xruns. It never fails on sound, since `make deploy` runs it on machines without devices.

## Consequences

- **The operator's desk** listens to the array directly for the wake and for sessions, with the host canceller off. The measurements are in the [2026-11-13 microphone path note](../benchmarks/2026-11-13-microphone-path.md):
  - **Sessions** read the same on both inputs: 4.6% word error on the array's channel 0 against 4.2% on `ghost.mic` for the turn the model gets, and better under music (4.2% against 8.3%).
  - **The 15.4% of 2026-11-06** was the int8 model's.
  - **Ghost's own voice** through the sound bar (later on 2026-11-13, §7 of the note): 0 passive VAD segments and 0 wakes from 90 s of Ghost's sentences and 80 clips of Ghost saying the wake phrases; no long reply in a desk session cut off. At Ghost's onset the array's channel 0 held −54 dBFS in the first half second against a −55 dBFS room (122 onsets, played at −19 dBFS): nothing leaks there.
- **The echo filter's warm-up rule follows the path** (`Plan::echo_warmup`, `EchoFilter::with_warmup`). A short segment begun within 0.8 s of Ghost's voice (re)starting is taken for its leak only where a canceller leaks at the onset: Ghost's own canceller (also over an on-chip device), an unprotected microphone, a path Ghost could not read, and the phone. It is off where a session hears an on-chip device directly, and for a headset, since nothing of Ghost's plays into the room there. Off, the operator's "okay" or "yes" right as Ghost starts, and the words after the wake (#54), are kept. The test by words runs on every path. `GHOST_ECHO_WARMUP` overrides it for debugging only.
- **Idle audio still stays on the machine.** The meter counts numbers on the passive path and keeps none of the audio. The check's report is a level and a word count, never words. No amendment of the rule was needed; the passive audio's numbers were already local.
- **Cost (ADR 0018):**
  - The meter is one compare and one multiply per sample, and a sort of at most a few thousand run lengths every 3 s.
  - The plan reads `pw-dump` at most every ten seconds, when a pipeline is built or the desktop asks.
  - Nothing is added to a turn.
- **Not tested here:** a laptop, a USB headset, a webcam with desk speakers and a conference puck.
  - Their cases are tested on fixtures shaped like their PipeWire nodes. When a detection is wrong it fails toward working: no canceller assumed, the default microphone, the sentence that Ghost may hear itself.
  - A device that cancels on chip but isn't on the list reads as unprotected until it is added.
- **Still open:**
  - The warm-up rule off on the on-chip path is measured on Ghost's voice alone. A person speaking at Ghost's onset, and barge-in over Ghost's voice by a real voice, are still to be run live (#54). The host canceller's onset leak (#42) is unchanged where it is used.
