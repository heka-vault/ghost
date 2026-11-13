# The microphone path: the wake on the array's own stream, sessions, and the canceller, 2026-11-13

Issues #100, #58 and #26; ADR 0064. The operator's desk: a reSpeaker XVF3800 is the microphone and, through its headphone jack and a sound bar, the only speaker (the onboard card has nothing plugged in). Ghost's echo canceller (`ghost.mic`, PipeWire's WebRTC module) ran over the array's channel 0.

Nothing in this note quotes the operator's speech. The recordings made for it are under `~/.local/share/ghost/mic-path/` (0600) and stay on the machine.

## 1. The wake on each input

`make wake-check-replay` (`docs/benchmarks/scripts/wake_check_replay.py`), the shipped rule (detector at 0.6, Parakeet's check of a weak hit from 0.3 with `match_heys_spelling`).

The same recordings were scored on both inputs:
- **The 2026-10-30 clips** were recorded on `ghost.mic` and on the array at once. Their "raw" side is `parecord --channels=1` of the array, which is the mono **mixdown** of channel 0 (the processed beam) and channel 1 (a microphone the chip doesn't clean). It is a harder input than channel 0 alone, which is what Ghost now opens.
- **The 2026-11-06 Hearing session** keeps both of the array's channels, so channel 0 alone is scored there. Its blocks were joined into three streams: room speech (blocks A–F, 258 s), music through the array (G, 52 s) and silence (H, 75 s).

| set | `ghost.mic` | array |
|---|---|---|
| operator's "hey ghost" takes (21) | 16 heard | **20 heard** (mixdown) |
| false wakes in the take clips and the eleven barge-in and room clips | 1 (a music clip at 0.6 or more) | 0 (mixdown) |
| false wakes, room speech, 4.3 min | 0 (1 weak hit, turned down) | 0 (3 weak hits, turned down) (channel 0) |
| false wakes, music through the array, 0.9 min | 0 | 0 (channel 0) |
| false wakes, silence and room noise, 1.3 min | 0 | 0 (channel 0) |
| Ghost's own voice through the array (`respeaker-out` clip, 10 min, Ghost reading aloud): false wakes | 0 | 0 (mixdown) |

**Why the takes are lost on `ghost.mic`** (the dynamics measurement on the same takes, `2026-11-12-wake-dynamics.md`):
- Four of the five takes it scores under 0.6 came while Ghost was speaking or had just finished; the array scores those same takes 0.84 to 0.98.
- In those takes the 0.3 s where "hey" sits is 14 to 22 dB below the array in every band, while "ghost" is 10 to 15 dB above it.
- So the host canceller suppresses the start of the operator's speech during and after Ghost's voice. That is #42's double-talk suppression, now measured on the wake.

**Music.** The one clip where `ghost.mic` beats the array (0.97 against 0.17, `music-80pct`) is from 2026-10-17, when music played through the onboard output and the canceller's reference was that output. Since 2026-10-30 everything plays through the array. In the Hearing session's music block, channel 0 holds the music 15 dB under the uncleaned channel 1 (median −63.6 against −48.8 dBFS), and sessions read better on it (§3).
- The rule, which `ghostd::microphone` now says when it applies: a chip cancels only what plays through its own device.
- On a machine where other sound plays from another output, the on-chip case names that output and says Ghost may hear it.

## 2. The level

The 2026-11-12 note found the array "about 13 dB quieter". That was the mixdown against the canceller's gain control. Channel 0 alone, on the 2026-11-06 session:

| | `ghost.mic` | array channel 0 |
|---|---|---|
| speech, median of half seconds | −35.2 dBFS | −38.4 dBFS |
| speech, 95th percentile | −13.1 dBFS | −13.1 dBFS |
| room with nobody speaking (block H), median | −68.8 dBFS | −54.7 dBFS |

- **Speech** is within 3 dB. The detector doesn't mind 30 dB either way (2026-11-12 note, §1). The VAD's volume floor sees the same speech level.
- **The room floor** is 14 dB higher on channel 0, because the host canceller's noise suppression is gone. It added no false wakes and no made-up words (§1, §3).
- **No gain stage was built.**

## 3. Sessions on each input

`ghostd perf hear --takes ~/.local/share/ghost/hear/sessions/2026-11-06` on each channel (fp32 Parakeet, the operator's settings, release build). `turn` is what the model is given (ADR 0044).

| | `ghost.mic` | array channel 0 |
|---|---|---|
| word error rate, turn | 4.2% | 4.6% |
| word error rate, turn, music (block G) | 8.3% | **4.2%** |
| word error rate, stage | 8.4% | 7.7% |
| empty turns | 0 of 49 | 1 of 49 |
| short replies dropped | 0 of 14 | 1 of 14 |
| silent takes with words, turn | 0 of 13 | 0 of 13 |
| read at the close, turn p95 | 1.84 s | 1.96 s |

- Every difference is within the harness's noise (its 95% intervals cross zero).
- The dropped reply (D04, two words at −7 dBFS) reads whole from the file, so it is one empty segment read, not the level.
- The 15.4% of 2026-11-06 for the raw array was the int8 model's (#52, #58), and is not what the array scores.

**Decision:** sessions move to the array as well on this desk. The words are the same; music reads better; and the canceller's drift over the day (§4) and its suppression of the operator's onset under Ghost's voice (§1) go with it.

## 4. The canceller (#58)

The live state on 2026-11-12, read without changing anything:
- The canceller had run for 8 days.
- `pw-top` ERR: `ghost.mic` 30,203, `echo-cancel-sink` 13,022, `ghost.mic.capture` 8,455, the array's capture 14,206; the daemon's own capture 0.
- The canceller's three nodes ask for a 10 ms quantum (480/48000), the block WebRTC works in. The graph runs the array at 128 samples per cycle at 16 kHz (8 ms): PipeWire rounds to a power of two, so the canceller never gets whole blocks and buffers across cycles.
- Restarted at 00:09 (the graph's rate held at 16 kHz, as Repair does), `ghost.mic` ran up 10 xruns in its first nine minutes of use.

So the xruns are steady, not a sign of age. They don't by themselves say when the canceller has gone deaf, and Ghost doesn't use them as the trigger. The trigger is what deafness looks like to Ghost: loud stretches that Parakeet reads as nothing, again and again.

The likely mechanism, not proven:
1. Every xrun on the capture or reference stream drops or repeats a block, and moves the microphone against its reference.
2. The canceller's state doesn't recover from that over hours, and a restart does.
3. The two adaptive stages make it worse: the gain control raises the residual above the room (#42), and the suppressor clamps the operator's onset under double talk (§1).

**On this hardware the host canceller has nothing to do.**
- With Ghost reading through the array, channel 0 held Ghost's voice at −82 dBFS (#42, 2026-10-30 21:24).
- Tonight a full-scale tone through the array lowered channel 0 by 5 dB (the chip's own suppressor working against its reference), while the uncleaned channel 1 didn't move.

**What was changed:**
1. **No host canceller on such hardware.** It is the default when the machine is that case. `install.sh` declines it unless `--force`. Ghost says so in `ghostd check`, Settings › Sound and the first run.
2. **Where the canceller stays** (a laptop, a webcam with desk speakers), its configuration is unchanged; the rates were already matched in PR #53.
   - Repair now holds the graph's rate across the restart, which removes #58's second failure (the restart moving the graph to 48 kHz and silencing the output).
   - Turning its gain control off made the operator's onset under Ghost's voice continuous in #42's take three. It cost a mid-word clamp under ducked music. Sessions now pause the players (`GHOST_PAUSE_PLAYERS`), so the far end in a session is Ghost alone, which is the case it helped. Changing it needs the live A/B on such a machine, not done here.
   - Matching the quantum to the canceller's block means turning off PipeWire's power-of-two quantum for the whole graph. That is a global change, not made without measuring it.
3. **Ghost notices.** The meter and the check counts below, a note on the waiting list (`input:degraded`), and Repair.

## 5. Health signals, and what they cost

- **The meter** per 3 s window counts exact zeros, the median run between them and the level: one compare and one multiply per sample. On fixtures:
  - #26's chopped stream (25 zeros in every 30 samples) trips it.
  - A canceller's gate (a second of zeros, a second of signal) doesn't.
  - A quiet room doesn't.
- **The check report** is the level and the word count of each Parakeet check of a weak wake hit, a reading Parakeet already makes.
- **Thresholds.**
  - Dropping: 30% zeros or more in runs of 32 samples or less, held 30 s.
  - Deaf: 3 final checks at −35 dBFS or louder with no words, within 20 minutes. #58's morning had 14 empty checks, two at −23 and −25 dBFS. The false wakes on room noise in #13 were at −40 dBFS and quieter, under the bar.
- **The meter's cost**, measured: an hour of audio in 134 ms, 0.74 µs per 20 ms frame (release build, `cargo test --release -p ghostd meter_cost -- --ignored`). That is about 0.004% of a core on the passive path.
- **Nothing is added to a turn.**

## 6. Live, tonight

Under `flock /tmp/ghost-rubric/mic.lock`, the operator's `make dev` Ghost stopped, output at 100% (as found, left there).
- **`ghostd check`** on this machine names the case (on-chip), what each input hears and the suggested settings.
- **This branch's daemon**, run with every Ghost path in a temp folder, opened the wake's stream as `<array> @ 16000, channel 0 of 2 alone`. `pw-top` showed `ghost-wake-capture` at 16 kHz, 2 channels, 0 xruns, and `ghost.mic` suspended (not opened).
- **Ghost's voice through the room could not be measured.** A full-scale 1 kHz tone through the array's output didn't reach the uncleaned channel at all, five times between 00:00 and 01:22: the sound bar on the array's jack was off. The recordings made then (Ghost's voice from the Kokoro corpus, 8.5 min including 80 clips of Ghost itself saying "hey ghost", and synthetic music, on the canceller as it was after 8 days and fresh) hold the room's floor only. They are not used.

**Left on the machine** (the way back is in each line):
- `~/.config/ghost/settings.env`: `GHOST_INPUT_DEVICE=ghost.mic` is commented out. Unset, this branch hears the array's channel 0 for the wake and sessions; main's code before this branch falls back to the default source instead of failing. Backup: `settings.env.before-microphone-path-2026-11-13`.
- `ghost-echo-cancel` stopped and disabled: `systemctl --user enable --now ghost-echo-cancel` brings it back. The rendered `echo-cancel.conf` is untouched (backup `echo-cancel.conf.before-microphone-path-2026-11-13`).
- The output level is 100%, as found.
- The operator's `make dev` Ghost was stopped at 00:00 and is left stopped.

With this branch's daemon on those settings (paths in a temp folder), a file wake closed the wake's capture and opened the session's on the array's channel 0 (`ghost-capture`, 2 channels at 16 kHz, 0 xruns); after the session the wake's capture opened again.
