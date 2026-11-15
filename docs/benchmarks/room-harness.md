# A session in a noisy room: the room harness

`make perf-room` plays whole sessions with a person at the desk while the room goes on
around them, and counts what Ghost took that nobody said to it. `make perf` measures how
long a turn takes and `make perf-hear` what Ghost heard of words meant for it; this one
measures what Ghost should not have heard at all. The code is
`crates/ghost-voice/src/perf/room/`; the first report is
[2026-11-14-noisy-room.md](2026-11-14-noisy-room.md).

```sh
make perf-room                                        # every room, both paths, ~1 h
make perf-room ARGS="--conditions babble --snr 6 --scripts follow --jobs 2 --show-text"
make perf-room ARGS="--set room_gate=on"              # as a laptop's session has it (ADR 0071)
```

It runs the `ghost-perf` binary directly: no vault, no settings file, nothing of this
computer's sound. A run's settings are Ghost's defaults and its own `--set NAME=VALUE`s
(the `GHOST_` variable without the prefix). Reports land in
`.scratch/perf/<stamp>-room/` (`results.json`, numbers only, and `report.md`).
`--show-text` prints what was heard to the terminal; nothing writes it to disk.
`--keep-audio` writes each scene's microphone mix as a WAV beside the report, for
listening to a scene that went wrong (synthetic voices only).

## What a scene is

The whole session pipeline, `build_session`, with the real Silero, Parakeet, Smart Turn,
echo filter, turn rule, idle clock and Kokoro for Ghost's voice, fed in real time (the
stages keep wall-clock time), several scenes at once (`--jobs`, 6 by default). The model
is scripted: it answers the person's lines with the scene's replies and anything else with
"Sorry, I didn't catch that." (0.6 s to the first word, 150 tokens a second), so every
turn Ghost takes is counted and nothing is spent.

**The person** is Kokoro's `am_michael`, close to the microphone with a trace of the room
(a reverb 14 dB under the direct sound). Three scripts:

| Script | What the person does |
|---|---|
| `follow` | After the greeting: a question; a follow-up a second after the answer; then, 9 s after the next answer and 5 dB quieter, a third line; then nothing. |
| `barge` | A question; 1.8 s into the answer "Wait, make it the app repo instead." (an interruption with words); 1.8 s into that answer "Stop." (one of `STOP_WORDS`); then a question. |
| `first` | Woken with the request in the same breath (`first_utterance`), answered, and nothing more: the session never hears the person's level. |

**The room** ([scene.rs](../../crates/ghost-voice/src/perf/room/scene.rs)), every sound
synthetic and seeded, so a run is the same on every machine. Voices are Kokoro's, in
voices that are neither Ghost's nor the person's; the rest is made from seeded noise and
sines. A talker across the room is attenuated (to the scene's ratio), low-passed at
3.5 kHz and given a short synthetic reverb (RT60 0.55 s, as much reverberant energy as
direct). The beds are 240 s, rendered once and cached under `.scratch/perf/room/`; each
scene starts at its own seeded point in them.

| Room | What it is |
|---|---|
| `quiet` | Nothing. |
| `noise` | A fan (filtered noise), a street (brown noise, a car passing every 8–15 s), a keyboard in bursts of typing. |
| `talker` | One person across the room (`bf_emma`), office small talk with pauses. Some lines are the kind a model would act on ("Can you send me that spreadsheet…"). |
| `babble` | Four people at once (`af_sarah`, `am_adam`, `bm_lewis`, `af_nicole`), each a little further off than the last. |
| `name` | One person across the room (`am_eric`) who mentions Ghost, never at the start of what they say ("You should ask Ghost about the release later"). |
| `music` | A band (chords, kick, hat) 6 dB under a singer (`af_sky`), from a speaker across the room. |

**The ratio** (`--snr`, 20, 12, 6 and 3 dB) is the person's active speech level over the
room's (ITU-T P.56 in spirit: the mean power of the frames within 25 dB of the loud end).

**Ghost's own voice** (`--paths`): `headphones`, nothing comes back; `speakers`, it comes
back through a canceller's residual, duller, 20 ms late and 28 dB down, except in the
first half second after it starts from silence, when it comes back 14 dB down (the
canceller converging; measured on the desk's array, 2026-10-30).

## What it counts

| Column | What it is |
|---|---|
| false turns | Turns Ghost closed with words that held no line the person had said in the last 20 s (a line counts when 60% of its words are in the turn). Each is a model call. |
| of them act | ... with a word a model would act on (send, close, label, assign, read, check, ...): a proxy for a tool call, since the scripted model calls none. |
| well below / as loud | ... split by the loudest segment read with words for that turn, against the person's segments: 10 dB or more under them, or closer than that. A turn heard over Ghost's voice has no read of its own and counts as "as loud". This split is what a level can and can't separate: the first kind a level tells from the person, the second only who is talking could. |
| opened for nothing | The screen showed the person talking (`Activity::YouStarted`, ADR 0055) for a turn that then closed with no words. |
| false cuts | Ghost cut off (`Activity::Cut`) with no line of the person's under way. |
| lines lost | The person's lines (the stop aside) with no turn holding most of them. |
| WER | Word errors on the person's turns, lost lines counted whole. |
| stops and interruptions cut | `barge`'s two lines meant to cut Ghost off, and how many did within 1.5 s of their end. |
| session ended after | From the end of Ghost's answer to the person's last line to the session's end; `never` when it had not ended 75 s later (the idle timeout is 45 s). |

## What it does not do

- **The wake.** A scene starts at the wake; the passive listener is not run. Nothing in
  the noisy-room fix touches the wake path, and its own checks are
  [wake-takes.md](../wake-takes.md) and `make wake-eval`.
- **Real rooms, real microphones.** The room is a model: no microphone's own gain control,
  noise suppression or beam, no talker moving, no Lombard effect. It ranks changes; a
  desk with real noise confirms them ([voice-rubric.md](../voice-rubric.md), Part I).
- **Time under load.** The stages keep wall-clock time, so a busy machine slows a scene:
  the microphone's frames arrive late (the log says `scripted mic 0.8s behind`) and turns
  read whole take longer. Compare runs made on the same machine at similar load.
