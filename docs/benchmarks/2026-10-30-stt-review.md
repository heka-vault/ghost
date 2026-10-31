# Speech to text on the desk: where the words go, 2026-10-30

**Question:** Ghost hears the operator less precisely than the Android keyboard does. Is
it Linux, the model, or the way Ghost uses it?

**Host:** i5-11600K, no GPU, PipeWire 1.6.2 with `libwebrtc-audio-processing` 1.3. **Mic:**
the reSpeaker XVF3800 array on its 16 kHz USB firmware, channel FL (the chip's processed
beam: its own AEC, beamformer, noise suppression and AGC), with the sound bar on the
array's own jack since 21:24 UTC so the chip cancels Ghost's playback. **Models:** Parakeet
TDT 0.6B v3 int8 through the oracle (`tools/oracle`, onnx-asr 0.12, ONNX Runtime 1.24.4),
the same files and session options as `ghost-infer`, and the fp32 export of the same
model from the same Hugging Face repository (`istupakov/parakeet-tdt-0.6b-v3-onnx`).

**Material:** the day's paired clips in `.scratch/clips` (`ghost.mic` and the raw array
recorded side by side, the operator's real voice during live sessions), the operator's
stored turns in `ghost.db`, the latency records, and the 14 perf-harness lines with
known text. Scripts in `.scratch/stt-review/`. Every read puts Ghost's −72 dBFS dither
under the audio, as `ghost_voice::stt` does.

**Not measured:** a word error rate on the operator's voice against a written reference.
There is no labelled set yet; the raw channel's whole-clip read, which was word-perfect
on the clips checked by eye, stands in for one below. The operator's own takes are the
next step (see the end).

## Findings

1. **It is not Linux.** The array's beam, read whole, transcribes the operator and Ghost
   word for word with punctuation. The loss happens between that channel and the model:
   in the host canceller and in how the audio is cut up.
2. **The segments are fragments.** Ghost closes a VAD segment after 0.2 s of silence and
   reads each one alone. Since 2026-10-28: 68 of 241 turns had two or more segments (up
   to 12), 65 of 354 stored user turns are one word, and the median pause the operator
   went on after was 0.46 s. Reading the same audio per turn instead of per segment:
   see table 2.
3. **The host canceller eats the operator's words.** `ghost.mic` is the chip's output run
   through PipeWire's WebRTC AEC, noise suppression and gain control again. Across the
   14 paired clips, after Ghost's own lines (from `ghost.db`) are subtracted from the raw
   read, 480 operator words remain; 266 of them are missing from `ghost.mic`'s read
   (table 1). Some clips were deliberately weak or far, so the share is rough; the
   direction holds on every clip with the operator in it.
4. **Ghost's playback does not leak on the beam channel.** With the sound bar on the
   array, 46 s of Ghost's voice through the speakers left the chip's beam at −53 to
   −57 dBFS, `ghost.mic` at −62 to −67 dBFS, and Parakeet read nothing from either.
   The beam channel is usable as Ghost's input without the host canceller.
5. **The graph resampled three times.** PipeWire ran at 48 kHz, the only allowed rate,
   so the 16 kHz array was upsampled into the canceller, the reference beside it, and
   the 48 kHz `ghost.mic` downsampled again for ghostd. Fixed on this branch
   (`infra/pipewire`): the canceller renders at the mic's rate and the installer allows
   it on the graph; measured with `pw-top`, the driver runs 128/16000 and every node's
   format is 16000. No change in what Parakeet hears on the leak test; it is waste
   removed, not accuracy gained.
6. **int8 against fp32:** table 3.
7. **Two long windows read as nothing:** table 4.

## Table 1: operator words on the raw beam and on `ghost.mic`, whole-clip reads (int8)

Raw words minus Ghost's lines in the clip's window of `ghost.db`; "missing" are those
absent from the `ghost.mic` read of the same seconds.

| Clip | Operator words (raw) | Missing on `ghost.mic` |
|---|---|---|
| barge-183525 (88 s) | 43 | 17 |
| over-ghost-195521 (150 s) | 102 | 59 |
| wake-184852 (58 s) | 41 | 35 |
| wake-weak-194309 (150 s, weak voice) | 37 | 24 |
| weak-empty-203018 (150 s, weak voice) | 28 | 16 |
| empty-seg-204159 (150 s) | 17 | 5 |
| barge-181215 (88 s) | 16 | 5 |
| respeaker-out-212422 (150 s, after the sink move) | 10 | 4 |
| all 14 | 480 | 266 (55 %) |

## Table 2: fragments against turns, on the audio Ghost saw

Ghost's VAD (ported to Python, Ghost's parameters) over each clip's `ghost.mic` channel;
each segment read alone with the 1.0 s pre-roll and 0.2 s trailing silence as Ghost does,
then the same segments grouped into turns (gaps under 1.5 s) and read as one span.

| | int8 | fp32 |
|---|---|---|
| VAD segments over 19 clips | 164 | 164 |
| grouped into turns | 112 | 112 |
| segments read as one word | 26 | 18 |
| segments read as nothing | 20 | 8 |
| words the per-segment reads add beyond the whole-span read | 248 | 203 |
| the same, per-turn reads | 218 | 154 |

The counts say less than the reads themselves. The same audio, per segment and then per
turn (int8):

| Per segment | Per turn |
|---|---|
| "Can you read issue?" / "Four oh three." | "Can you read issue four oh three?" |
| "Read me issue four one three from the" / "from the quotient research ecosystem project." | "Read me issue four one three from the quotient research ecosystem project." |
| "Can you read me issue" / "Issue four one five." / "From the" | "Can you read me issue four one five from motion?" |
| "Give me the details about issue." / "" / "Four, one, three." | "Give me the details about issue four one three." |

A per-segment read of a number after a pause is a sentence of its own, which is what put
"four? Or Thirteen" in front of the model on 2026-10-30 (#51). Reading the turn whole costs
0.3 to 0.9 s of Parakeet per clip here, about 0.15 s per turn on int8.

## Table 3: int8 against fp32

**Rendered lines with known text** (the perf harness's 14 operator lines in Kokoro's
`am_michael` voice), each read twice:

| | Word error rate | Per line, p50 | Per line, max |
|---|---|---|---|
| int8 | 12.1 % (11 of 91) | 0.12 s | 0.20 s |
| fp32 | 11.0 % (10 of 91) | 0.18 s | 0.29 s |

Both write "Close in for seventeen" for "Close infra 17" and "Market ready for agent" for
"Mark it ready for agent"; the one word fp32 gets right that int8 does not is "17" against
"seventeen", a spelling. The errors are the model's, not the quantization's.

**The operator's own turns** (the turn spans of eight clips, `ghost.mic` channel): the two
read 25 of 35 spans the same. Where they differ, fp32 read "quotient" where int8 read
"motion", "Thank you" where int8 read nothing, "Come on" for "Come", and was otherwise no
better on the short fragments ("You sat up first ahead" against "You stop that for a
second"). Per turn: int8 0.11 to 0.26 s, fp32 0.17 to 0.30 s.

**What fp32 does change:** it reads far more from a long window of the cancelled channel,
939 words against 354 over the 19 clips read whole. Most of that is Ghost's own residue
past the canceller and the room, which int8 leaves blank. More sensitivity is not a gain
on `ghost.mic` as it stands: it would feed the echo filter and the barge-in rule more.

**Verdict:** the model's own errors dominate; int8 against fp32 is a one-word difference
on this material at 1.5x the time. Not worth switching before the operator's labelled
takes say otherwise. The two files stay staged in `.scratch/stt-review/parakeet-fp32/`
(ONNX Runtime 1.24 refuses the Hugging Face cache's symlinked external data, so the
oracle loads them from a plain directory).

## Table 4: the empty long windows

Two clips (`fw-211018`, `greeting-205227`) read as nothing over 120 to 150 s windows on
both channels even with the dither. Re-read in 30 s windows and with fp32:

| Window | int8 | fp32 |
|---|---|---|
| fw-211018 raw, 120 s whole | nothing | a video playing in the room, read in full |
| fw-211018 raw, 30 s windows | nothing, nothing, nothing, then the video's words | |
| greeting-205227 raw and `ghost.mic`, 120 s whole | nothing | nothing |
| greeting-205227, 30 s windows | "Okay." in the last one | |

So the int8 encoder goes silent on long inputs that fp32 reads, and the first clip's
content was a video on the operator's screen, not the operator. The second clip holds
one "okay". Ghost never hands Parakeet a window that long (the interim read is capped at
4 s, a segment at the VAD's length), so this does not affect the pipeline; it does mean
the int8 model is not the tool for reading a whole recording, and the whole-clip reads in
tables 1 and 2 undercount what is on the tape. That makes table 1 a lower bound on the
words `ghost.mic` loses, not an upper one.

## What to do, in order

1. **Read the turn, not the segment.** Keep the turn's audio in the speech-in stage and
   re-read it whole when the turn closes; per-segment reads stay for the speculative
   answer and barge-in. Costs one Parakeet pass of the turn's length at the close.
2. **Try the beam channel as Ghost's input.** `.scratch/stt-review/ghost-mic-raw.conf`
   publishes the array's FL alone as `ghost.mic.raw` with no host canceller; set
   `GHOST_INPUT_DEVICE=ghost.mic.raw` and listen for Ghost hearing itself. If it holds,
   the canceller service becomes optional for this hardware.
3. **Record the operator's takes.** Thirty commands with their text, at the desk and
   across the room, with and without music, on both channels at once
   (`scripts/record-clip.sh`). That set decides table 3 for real and is the regression
   set for 1 and 2.
