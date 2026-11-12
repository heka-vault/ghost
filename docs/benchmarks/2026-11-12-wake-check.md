# Wake: why the operator had to speak up, and the check of a weak hit, 2026-11-12

**The report:** "the wake keeps requiring louder speaking to wake Ghost". In the rubric run that morning (`GHOST_WAKE_MODE=onnx`, threshold 0.6, `GHOST_WAKE_VERIFY=false`, input `ghost.mic` over the reSpeaker XVF3800):
- About half of roughly fifteen "hey ghost"s scored 0.32–0.59, in a quiet room and with music.
- Each of those went to Parakeet as a weak hit (the 0.3 floor, `GHOST_WAKE_CHECK_THRESHOLD`).
- Parakeet wrote "A ghost.", "A ghost?", "A ghost. A ghost." or one unrelated word, and five of seven were turned down.
- `pw-top` showed 24,921 xruns on `ghost.mic` and 11,555 on the array.

**What was measured, offline.** The Hearing recorder opened on today's session for 13 s and kept no audio, so today's wakes can't be replayed. The measurements use the operator's own recorded wake attempts from 2026-10-30 (`.scratch/clips`). Each has `ghost.mic` (aec) and the array (raw) recorded side by side:
- 21 "hey ghost" takes in all, at the desk, under music and in live sessions, labelled from what the raw stream reads.
- **Negatives:**
  - 6.4 min of the operator's ordinary speech (the 2026-11-06 Hearing session);
  - eight live-session clips with Ghost talking and the operator barging in, "Hang on, Ghost" among them;
  - three clips with no wake in them;
  - the replay rig's Kokoro clips: 48 "hey ghost" and the named confusables, digitally and from the desk speakers on the array's channel 0, quiet, with music and far.

The tool is `make wake-check-replay` ([scripts/wake_check_replay.py](scripts/wake_check_replay.py)). It scores every 80 ms as the processor does and reads each weak hit with Parakeet (fp32) twice, to just after the hit and 0.8 s later. Nothing in this note quotes the operator's speech beyond Parakeet's spellings of the wake phrase.

## 1. Why the scores were low

**Not the level.** Each of 11 takes was scored with its gain changed from −18 dB to +12 dB:
- On the array's stream 10 of 11 moved by 0.08 or less across all 30 dB.
- On `ghost.mic` five moved by more than 0.1, in both directions: 0.83 → 0.50 and 0.62 → 0.87 with more gain, 0.21 → 0.44 with less.
- No gain lifts the low scores as a rule.

The check windows of today's refused wakes read −19 to −25 dBFS, the same as the windows of the wakes that fired (−20 dBFS). Normalising the level before the detector would cost a gain stage per frame and change nothing, so it isn't built.

**Not the model.** On the array's own stream the detector scores the same takes higher:

| take score (best in ±1.2 s), 14 takes | `ghost.mic` | raw array |
|---|---|---|
| at 0.6 or more | 11 | 13 |
| median | 0.89 | 0.94 |
| worst pairs | 0.21, 0.56 | 0.92, 0.97 |

The model gives the operator's voice 0.9 and more when the stream is clean, so retraining is not indicated.

**The stream: the host canceller.** `ghost.mic` is the WebRTC canceller in PipeWire, running on top of the array's own canceller.
- It loses some takes the array keeps whole: 0.21 against 0.92, and 0.56 against 0.97.
- It scored 0.40 and 0.54 on two stretches, music and a word to Ghost, that the array scores 0.01.
- Today it had run up 24,921 xruns, twice the array's count. That is issue #58's picture of `ghost.mic` going deaf over the day.

**The framing is by design.** The processor takes the first frame at the 0.3 floor as the hit, though the score may still be rising:
- On real takes the score goes from 0.3 to 0.6 within one frame (80 ms), and within three on the replay's voices.
- So a take caught on its way up is checked by Parakeet, and a take that peaked at 0.92 can be turned down.

Waiting up to three frames for the peak was measured (the "hold" rows below):
- It heard 5 more of the 21 takes on `ghost.mic`.
- It also let the near-misses that climb past 0.6 wake unchecked: 13 false wakes for 10 digitally, 15 for 9 in the room, and 1 and 2 on the operator's own negatives.

That trades away issue #13, so it isn't built. A test now holds the behaviour (`test_a_score_that_rises_through_the_band_is_checked`).

## 2. The check of a weak hit

Parakeet writes the operator's "Hey Ghost" on `ghost.mic` as **"A ghost."** more than anything else: today, on 2026-10-30, and even on takes the detector scored 0.9. `match_wake` at 0.8 compares phonetic keys: "a ghost" is `agst` against `hgst`, a ratio of 0.75, so it was refused.

**The rule** (`match_heys_spelling`, `crates/ghost-infer/src/wake/matcher.rs`):
- **What it accepts:** a sentence of the reading that opens with the word "ghost", after at most one of `HEY_SPELLINGS` ("a", "eh", "ah", "uh", "oh", "hey", "hi", "he", …).
- **When it applies:** only to a weak hit of the `hey_ghost` model, which the detector has already scored in the checked band.
- **What it rejects:** the name inside a sentence ("I saw a ghost"), "the ghost story", "ghostly", Parakeet's "It goes" for room talk, and two leading words ("So a ghost walked in").

It also gives the request back once Ghost is woken: "A ghost, what needs my sign-off?" now has its request as the first turn. Before, the request was dropped.

| set | before (`match_wake` 0.8) | after (shipped) | rejected: hold | rejected: hold + rule |
|---|---|---|---|---|
| operator's takes, `ghost.mic` | **10/21** | **16/21** | 15/21 | 17/21 |
| operator's takes, raw array | 19/21 | 20/21 | 20/21 | 20/21 |
| false wakes, operator's negatives (`ghost.mic` / raw) | 0 / 0 | **0 / 0** | 1 / 2 | 1 / 2 |
| false wakes in the take clips (`ghost.mic` / raw) | 1 / 0 | 1 / 0 | 1 / 2 | 1 / 2 |
| Kokoro digital: heard, false wakes | 46/48, 10 | 46/48, **10** | 46/48, 13 | 46/48, 13 |
| Kokoro room, array ch0 (desk, music, far): heard, false wakes | 48/72, 9 | 49/72, **9** | 51/72, 15 | 51/72, 15 |
| Parakeet runs, `ghost.mic` takes | 15 | 15 | 6 | 6 |

Notes on the table:
- **The one false wake in the take clips** is a score of 0.6 or more on music (`music-80pct`), so the check never sees it.
- **The 10 and 9 false wakes before the fix** are the existing weak check's own: "He goes to the gym", "Hey Gus", "Hey, goat cheese" at 0.8.

The rule adds no false wake in any set. Every hit it flips is a real call to Ghost: read on the raw stream, eight are "Hey Ghost" and one is "Ghost, are you there?" (not counted as a take of this model).

**Cost (ADR 0018):**
- Nothing per frame.
- The rule is a word scan of a reading Parakeet already made, microseconds, and Parakeet runs exactly as often as before.
- The chime comes no later: a weak hit already waited for its reading.

## 2b. Waking on the name alone

The idea: "Ghost" is rare enough in ordinary speech to wake on whenever it is heard for sure. It does not hold as well for the operator, who builds Ghost and says its name about the product and in session commands. Every speech segment with the word "ghost" in 6.2 min of the operator's recorded speech (the 2026-11-06 Hearing session and the 2026-10-30 clips, `ghost.mic`):

| what was said | segments | hey_ghost score |
|---|---|---|
| a call: "Hey ghost.", "Ghost.", "A ghost." | 8 | 0.87–0.97; one bare "Ghost." 0.07 |
| the product, mid-sentence | 2 | 0.01, 0.04 |
| a sign-off: "Later ghost.", "Talk to you later, ghost. Bye." | 2 | 0.01, 0.01 |
| a session command: "Hang on, ghost.", "Hang up ghost.", "… stop ghost …" | 3 | 0.27, 0.78, 0.02 |

Half the times the word was said were not calls. A sign-off is said just as passive listening resumes, so "the word anywhere" would wake Ghost again right after the operator hangs up.

**Inside the detector's hits** (a weak hit whose reading has the word anywhere, the "name anywhere" row of `make wake-check-replay`):
- It hears no more of the operator's takes than the shipped rule: 16/21 on `ghost.mic`, 20/21 raw.
- It adds false wakes on "Hang on, Ghost" and "Hang up ghost" at 0.32–0.59: 1 more on `ghost.mic`, and 1 more each among the operator's takes and negatives on raw.
- On the Kokoro sets it adds none.

The shipped rule is this idea with the guard the data asks for. It accepts the name when it opens a sentence, bare ("Ghost.") or after one short sound for "hey", and when the detector already scored it. Every call above passes; none of the other seven does.

**Without the detector** (transcript wake, Parakeet reading every utterance, the name opening it):
- On the same speech it would have heard all 8 calls, the bare "Ghost." at 0.07 among them, with no false wake.
- That is one call more, at the cost ADR 0025 moved away from: Parakeet on every utterance in the room, and the chime after the operator stops rather than as "ghost" ends.
- 6 minutes is too little to measure false wakes an hour, and the LibriSpeech stream that measured them in October is no longer on disk. So it is not built; it needs an hour of the room recorded on purpose first.

## 3. Not built

- **Level normalisation before the detector:** see §1. The level was not the problem.
- **Scoring `ghost.mic` and the raw array both:**
  - It would hear what the array hears: 13 of 14 takes at 0.6, against 11.
  - The cost is a second capture and a second scorer pass, 17.5 ms per 80 ms frame, about 22% of a core more while passive.
  - The better fix is a setting, below. A daemon that keeps a second idle stream also widens what passive audio touches, for a gain the setting gives whole.

## 4. For the operator, at the desk (minutes)

1. **See whether `ghost.mic` is degraded now.**
   - Find the array's input with `pactl list short sources` (the XVF3800 `alsa_input`, not a monitor).
   - Then: `GHOST_RAW_MIC=<that source> make record NAME=wake-ab-1 SECS=30`.
   - Say "Hey Ghost" eight times at your normal voice, three seconds apart, and nothing else.
   - Then: `make wake-check-replay ARGS="--stream .scratch/clips/wake-ab-1-aec.wav:count=8 --stream .scratch/clips/wake-ab-1-raw.wav:count=8"`.
   - Each row reads "heard/8": `ghost.mic` first, the array second.
2. **If `ghost.mic` hears fewer:**
   - Pin the graph, then restart the canceller (issue #58): `pw-metadata -n settings 0 clock.force-rate 16000 && systemctl --user restart ghost-echo-cancel`.
   - Then repeat step 1 as `wake-ab-2`.
   - If it recovers, the canceller drifts over the day (#58), and a restart is the crutch until it is replaced.
3. **If the array keeps beating `ghost.mic` after a restart**, set `GHOST_INPUT_DEVICE` to the array's source in `~/.config/ghost/settings.env` (#52 item 2).
   - The array cancels Ghost's own voice on chip, since Ghost plays through its output.
   - This changes what sessions hear too. Score both with `make perf-hear` before switching.
   - The array is about 13 dB quieter than `ghost.mic`. The detector doesn't mind (§1); the VAD's `min_volume` may.
4. **Still to check live:**
   - The rule with the operator talking: wake 15 times as this morning, half of them quietly, with `GHOST_DEBUG_AUDIO=1`.
   - A "not confirmed" whose reading opens with "ghost" would be a miss of the rule.
   - Leave music on for a few minutes with nobody talking: any wake there is a false one (#13).
