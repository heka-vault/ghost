# Wake: does compression or gain before the detector help? 2026-11-12/13

**The question** (the owner): "Is there anything we can or should do with audio compression that helps our wake word process?" That means dynamic-range processing of the microphone before the "hey ghost" detector and before Parakeet's check of a weak hit: a compressor, a limiter, automatic gain or loudness normalisation. It does not mean a codec.

**The answer:**
- **No processing is worth building as a runtime step.** None of the 15 variants hears more of the operator's takes without costing the room or digital sets. The best is pre-emphasis: 18 of 21 on `ghost.mic` against 16, but it lost 2 of the 72 room takes and added 3 false wakes there.
- **The low scores are not a level problem.** On `ghost.mic`, the host canceller mutes the start of the phrase to exact zeros or chops a hole in it while Ghost is talking or has just stopped, and no gain brings back zeros.
- **The array's own stream fixes it.** The detector on the array heard 19 of 21 with no new false wake on any negative set, and 20 of 21 when Parakeet reads the array too. That is the path ADR 0064 has since made the desk's default.
- **For retraining**, the useful augmentation copies the canceller's damage: muted onsets, holes, Ghost's voice under the phrase. Compression is not that augmentation.

The tool is `make wake-dynamics` ([scripts/wake_dynamics.py](scripts/wake_dynamics.py), the streams in [scripts/wake_dynamics_set.txt](scripts/wake_dynamics_set.txt)). It is the [2026-11-12 wake check](2026-11-12-wake-check.md)'s replay with a processing stage in front:
- **The rule is the shipped one.** The detector scores every 80 ms. A score at 0.6 wakes Ghost. A score from the 0.3 floor is a weak hit, which Parakeet (fp32) reads twice and `match_wake` at 0.8 or `match_heys_spelling` confirms.
- **Dependencies:** numpy only. Nothing is added to the oracle's lock.
- **What it writes:** the scores as numbers, and whether each weak hit was confirmed, as true or false (`~/.cache/ghost/wake-dynamics/`), so the run resumes and can go in batches. Parakeet's readings are never written or printed.
- **Run time:** the first run is about 4 CPU-hours of scoring, then about 1.5 hours of Parakeet. A run from the cache takes a minute.

## The sets

The same streams as the wake check, and the baseline reproduces its "after" row exactly:
- **The operator's takes:** 21 "hey ghost"s recorded on 2026-10-30, each on `ghost.mic` and on the array's raw stream side by side.
- **The operator's negatives:** 6.4 min of ordinary speech (the 2026-11-06 Hearing session), eight live-session clips with Ghost talking and the operator barging in, and three clips with no wake.
- **Kokoro, digital:** 48 "hey ghost"s and the named confusables.
- **Kokoro, room:** the same played from the desk speakers and recorded on the array's channel 0: near, with music, and far.

**Placements:**
- `det`: the detector hears the processed stream and Parakeet the unprocessed one.
- `stt`: the reverse.
- `both`: both hear the processed stream.

## Results

Each cell gives takes heard, false wakes (fw) and Parakeet reads (rd). The cost column is the variant in numpy here, per 80 ms frame, from one run on a shared machine (a second run gave up to twice as much).

| variant | where | operator takes, `ghost.mic` | operator takes, raw array | negatives `ghost.mic`: fw | negatives raw: fw | Kokoro digital | Kokoro room | µs/frame |
|---|---|---|---|---|---|---|---|---|
| **unprocessed** | | **16/21**, 1 fw | **20/21**, 0 fw | **0** | **0** | **46/48**, 10 fw | **49/72**, 9 fw | 0 |
| compressor 2:1, −30 dBFS, 5/100 ms | det | 17/21, 1 | 20/21, 0 | 1 | 1 | 46/48, 12 | 41/72, 6 | 50 |
| compressor 4:1, −30 dBFS, 5/100 ms | det | 18/21, 1 | 20/21, 0 | 0 | 0 | 46/48, 13 | 39/72, 5 | 45 |
| same, +12 dB make-up | det | 17/21, 1 | 20/21, 0 | 0 | 0 | 46/48, 15 | 39/72, 5 | 44 |
| compressor 4:1, 50/500 ms, +12 dB | det | 16/21, 1 | 20/21, 0 | 0 | 1 | 46/48, 13 | 43/72, 8 | 42 |
| compressor 8:1, −35 dBFS, 2/80 ms, +18 dB | det | 17/21, 1 | 20/21, 0 | 1 | 1 | 43/48, 13 | 31/72, 4 | 44 |
| +12 dB into a limiter at −6 dBFS | det | 16/21, 1 | 20/21, 0 | 0 | 1 | 46/48, 14 | 42/72, 7 | 36 |
| fast per-window normalisation (2 s, loudest 400 ms to −20 dBFS) | det | 16/21, 1 | 21/21, 1 | 0 | 0 | 46/48, 10 | 47/72, 9 | 155 |
| slow rolling normalisation (speech level, 3 s, held in noise) | det | 16/21, 1 | 19/21, 1 | 1 | 3 | 46/48, 10 | 49/72, 8 | 21 |
| AGC (300 ms, ≤ 20 dB) with a gate back to 0 dB | det | 16/21, 1 | 20/21, 0 | 1 | 1 | 47/48, 11 | 44/72, 9 | 21 |
| pre-emphasis 0.97, +12 dB | det | **18/21**, 1 | 21/21, 0 | 0 | 0 | 47/48, 10 | 47/72, **12** | 6 |
| pre-emphasis 0.97, +12 dB | both | **18/21**, 0 | 21/21, 0 | 0 | 0 | 47/48, 9 | 47/72, **12** | 6 |
| high-pass 200 Hz | det | 16/21, 1 | 19/21, 0 | 0 | 1 | 47/48, 13 | 47/72, 9 | 39 |
| high-pass 400 Hz | det | 16/21, 2 | 21/21, 1 | 3 | 2 | 48/48, 16 | 46/72, 9 | 39 |
| +6 dB over the first 150 ms after an onset | det | 16/21, 1 | 20/21, 0 | 0 | 0 | 46/48, 10 | 49/72, 9 | 21 |
| +12 dB over the first 150 ms after an onset | det | 16/21, 1 | 20/21, 0 | 0 | 0 | 46/48, 12 | 49/72, 9 | 22 |
| **the detector on the array, Parakeet on `ghost.mic`** | det | **19/21**, 1 | (20/21, 0) | **0** | (0) | (46/48, 10) | (49/72, 9) | 0 |
| **the array for both** (ADR 0064's desk) | | | **20/21**, 0 | | **0** | | | 0 |

**Placements:**
- **Before Parakeet only** (`stt`), every variant hears the same or fewer: 15 or 16 of 21 on `ghost.mic`, 20 of 21 raw, and no false wake on the operator's negatives. Parakeet is not short of level. Those rows are in `make wake-dynamics`'s output.
- **Before both**, the result follows `det`, within one take on the operator's sets and three on the room set.

**Reading the table:**
- **Compression helps `ghost.mic` by one or two takes when it feeds the detector.**
  - A 4:1 compressor lifts the take that scored 0.22 to 0.58 and one that scored 0.10 to 0.34, into the checked band, where Parakeet can confirm them.
  - The cost is the room set: 39 of 72 against 49, with 3 to 5 more digital false wakes.
  - The detector was trained on uncompressed audio. Squashing the room set's far and music takes moves them away from what it knows, more than it lifts them over the floor.
- **Normalisation and AGC change nothing on `ghost.mic`.**
  - The slow one adds 3 false wakes on the operator's raw negatives. Held gain lifts the quiet stretches between words.
- **Pre-emphasis is the one interesting result.**
  - It hears 18 of 21 on `ghost.mic` and 21 of 21 raw with no false wake on either negative set, at 6 to 13 µs a frame in numpy (it varied with the machine's load).
  - It loses 2 room takes and adds 3 room false wakes, and 2 takes in 21 is within what one more recording session could move.
  - It is a spectral tilt, not dynamics. If the retraining wants a cheap augmentation, a ±6 dB per octave tilt is a better candidate than compression.
- **The onset lifts do nothing** (next section).
- **The array's stream does what no processing does,** with no false wake on any negative set.
  - The array for both, as ADR 0064 now sets up the desk: 20 of 21, 0 false wakes.
  - The detector on the array with Parakeet on `ghost.mic`: 19 of 21.
    - Its one false wake in the takes set is a weak hit on the array (0.46 or 0.59) that Parakeet confirmed reading `ghost.mic`.
    - Read from the array, the same hit is turned down. That is a reason to give Parakeet the same stream as the detector, which is what ADR 0064 does.

## What is wrong with the low-scoring takes on `ghost.mic`

`make wake-dynamics ARGS=--onsets` compares each take's energy in four bands, in 100 ms steps over the second before the detector's peak. It does this on both streams, which recorded the same air at the same time.

**Best score per take (±1.2 s):**

| take | `ghost.mic` | raw array | what was going on |
|---|---|---|---|
| weak-empty 216.3 | 0.22 | 0.90 | the operator over Ghost's voice |
| respeaker-out 116.7 | 0.10 | 0.98 | Ghost had just spoken through the array's output |
| respeaker-out 125.1 | 0.06 | 0.82 | the same |
| barge 41.5 | 0.10 | 0.87 | a barge-in over Ghost |
| over-ghost 48.9 | 0.41 | 0.40 | a weak take on both streams |
| the other 16 | 0.56–0.98 | 0.83–0.98 | |

(The 21st take, with music, is the one the array loses: below.)

**Four of the five low takes are double-talk:** Ghost talking, or just finished. On those, in the stretch where "hey" sits (0.4 to 0.7 s before the peak), `ghost.mic` is 14 to 22 dB under the array in every band from 60 Hz to 7.6 kHz. "Ghost" itself comes out 10 to 15 dB *over* the array, from the canceller's gain control. The takes that scored well lose only 1 to 6 dB in the same stretch.

| mean, dB `ghost.mic` minus array | −0.7 s | −0.6 s | −0.5 s ("ghost") | −0.4 s |
|---|---|---|---|---|
| low takes, 300–1000 Hz | −13.5 | +5.0 | +15.3 | +15.9 |
| high takes, 300–1000 Hz | −3.9 | +8.5 | +12.7 | +16.9 |
| low takes, 1–3 kHz | −21.1 | −7.3 | +4.9 | +15.1 |
| high takes, 1–3 kHz | −0.8 | +7.8 | +12.1 | +15.3 |

**The level per 100 ms of `ghost.mic` shows what that is:**
- **respeaker-out 116.7:** −94 to −96 dBFS, digital silence, until about 0.4 s before the end of "ghost". The canceller muted "hey" and the start of "ghost" entirely.
- **respeaker-out 125.1:** the phrase begins (−38, −24, −21 dBFS), then 200 ms at −86 and −94 dBFS mid-phrase, then the rest. The canceller cut a hole in the middle.
- **weak-empty 216.3 and barge 41.5:** Ghost's residual sits at −31 to −56 dBFS under the start of the phrase. There is no quiet before the word, so an onset detector never fires. Lifting the start lifts Ghost's residual with it.

**They are not dropouts.** `ghost.mic` has 8 to 44% exact-zero samples against 2 to 8% on the array. They are 2 to 6 ms runs in quiet stretches (the canceller's gate) or the long mutes above, not the short USB-loss pattern ADR 0064's meter looks for. The array has no zero run of 2 ms or more anywhere in the take clips.

**So gain cannot help:**
- A muted onset is zeros, and zeros times any gain are zeros.
- A hole is a hole.
- Under double-talk, the lift that would reach the operator reaches Ghost's residual too.
- The onset lifts left all four double-talk takes where they were (0.22, 0.10, 0.06, 0.10).
- The compressor's gain on 216.3 (0.22 → 0.58) comes from the part of the phrase the canceller let through. It isn't a repair of the onset.

**The one take the array loses is music** (music-80pct 24.9): 0.95 on `ghost.mic`, 0.25 on the array. Only the host canceller removes music played from the computer. The array cancels only what is played through its own output. On the on-chip path, music should play through the array (the desk's setup since 2026-10-30), or the wake under music needs its own measurement (#13).

## Not done live

Live audio was allowed for this work from 2026-11-12 23:55, but a fresh double-talk test can't be made here by playback.
- **One speaker.** The array's output, with the sound bar on its jack, is the machine's only output.
- **Playback is reference.** A "hey ghost" played over Ghost's voice is part of what both cancellers subtract, not the operator talking over Ghost.
- **What it needs:** a second speaker the cancellers don't know about, or the operator's own voice (below).

Every result here is offline, from recordings. None is live.

## Cost if built (ADR 0018)

- **A compressor, limiter, AGC or filter in the daemon:**
  - About 10 to 50 operations a sample: 1280 samples a frame, 5 to 30 µs in Rust.
  - Against the detector's 17.5 ms per frame, that is 0.2% at most. The numpy column above is an upper bound.
  - Cost is not the reason to leave them out. They don't help.
- **The array as the detector's input:**
  - Nothing per frame. It is a setting (`GHOST_WAKE_INPUT_DEVICE`, ADR 0064), and with it the host canceller can be off altogether.
  - Scoring both streams costs a second scorer pass, about 22% of a core more while passive, and isn't needed.

## Conclusion, for the agents on the wake's input and on retraining

**On the wake's input (ADR 0064):**
- **Add no dynamics stage before the detector or the check.**
  - Compression buys 1 or 2 of 21 takes on `ghost.mic`, which the array gives anyway, and costs up to 10 of 72 room takes and 3 to 5 digital false wakes.
  - Normalisation, AGC, onset lifts and high-passes buy nothing.
- **Keep the detector and Parakeet on the same stream, the array's channel 0.** On these recordings that is 20 of 21 and 0 false wakes.
- **Take music played from the computer (not through the array) as the open risk on that path.** It is one take here, measured once.

**On retraining (#8, ADR 0025):**
- **Record the takes through the path the detector will hear.** On the desk that is the array's channel 0, not `ghost.mic`.
- **Record takes over Ghost's voice:** a block the current set lacks, and the one case the canceller ruined. With the array as input, the chip's cancellation is now the thing to test.
- **Augment with the damage the canceller did**, so the model holds up on any machine that still runs it (ADR 0064's host-canceller case):
  - zero or mute 100 to 400 ms at the onset;
  - cut 100 to 200 ms holes inside the phrase;
  - put a Kokoro reply under the phrase at −10 to −25 dB;
  - lift the level by +10 to +15 dB after a quiet stretch, as the canceller's gain control does.
- **Spectral tilt (±6 dB per octave) is worth an augmentation.** It is the one cheap transform that moved scores here.
- **Compression adds little.** If it is used, keep it to 2:1 to 4:1, on positives and negatives alike, since as a runtime step it raised digital false wakes by 3 to 5.

## For the operator, at the desk

Each step is optional and independent. Run them from the main checkout, with Ghost stopped (`make stop`, or Ctrl+C on `make dev`).

**1. Check the array's channel hears your wake (3 min).**
1. Find the array's source with `pactl list short sources` (the XVF3800 `alsa_input`).
2. Run `GHOST_RAW_MIC=<that source> make record NAME=wake-ab-1 SECS=30` and say "Hey Ghost" eight times at your normal voice, three seconds apart.
3. Run `make wake-check-replay ARGS="--stream .scratch/clips/wake-ab-1-raw.wav:count=8 --stream .scratch/clips/wake-ab-1-aec.wav:count=8"`. The first row is the array.

**2. Double-talk takes, the case this note could not record (5 min).**
1. Play spoken audio through the array's output at the level Ghost speaks (a podcast in the browser will do).
2. Run `GHOST_RAW_MIC=<that source> make record NAME=dt-1 SECS=60` and say "Hey Ghost" ten times over it.
3. Stop the audio.
4. Run `make wake-check-replay ARGS="--stream .scratch/clips/dt-1-raw.wav:count=10 --stream .scratch/clips/dt-1-aec.wav:count=10"`.
5. To see whether anything here helps on fresh takes, put the two files in a set file (two lines, `PATH:count=10`, under a `[name]` header) and run `make wake-dynamics ARGS="--set <file>"`.

**3. Takes for retraining (25 min).**
1. `make record-takes` records blocks A to F, 66 takes, each on `ghost.mic` and on the array's two channels ([wake-recording.md](../wake-recording.md)).
2. It needs `ghost-echo-cancel` running for `ghost.mic`. With ADR 0064's on-chip path that service may be off: start it for the session with `systemctl --user start ghost-echo-cancel`.
3. The double-talk block in step 2 is not in it yet. The retraining agent should add it.

Nothing in any step is taken from passive audio. Each take is recorded on purpose with a countdown, stays under `.scratch/` (git-ignored) and never leaves the machine.
