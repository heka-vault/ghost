# The voice rubric: a live test of talking to Ghost

A scripted conversation the operator has with Ghost at the desk, after any change to the
voice path, and a checker that reads the result out of the debug log and the latency
records. It takes about fifteen minutes. The spoken lines are fixed so that a run is
comparable with the last one; the pass marks are the debug lines the pipeline writes, not
an impression. First run 2026-10-31, after PR #50 (turn-taking) and PR #53 (the 16 kHz
path); results at the end.

## Setup

```sh
GHOST_DEBUG_AUDIO=1 make dev > /tmp/ghost-dev.log 2>&1 &   # or in its own terminal
tail -f /tmp/ghost-dev.log | grep -aE 'turn:|echo:|wake:|heard|barge|latency'
```

Debug audio logs everything the microphone hears, wake word or not, to that file only
(`ghost_voice::debug`). It is for this session; never leave it in a unit or a config.

After each part:

```sh
scripts/voice-rubric-check.sh            # /tmp/ghost-dev.log, the last 6 hours of turns
scripts/voice-rubric-check.sh LOG LAST   # another log, more turns
```

Timings in play, from `.env.example` unless `settings.env` says otherwise: the hold after
Smart Turn hears an end 1.2 s (`GHOST_TURN_GRACE_SECS`), or 0.35 s when it is sure (0.9 or
more) and the words end a sentence (`GHOST_TURN_CONFIDENT_GRACE_SECS`,
`GHOST_TURN_CONFIDENT_PROB`, ADR 0049); the close after a finished sentence when Smart
Turn did not hear an end 1.6 s (`GHOST_TURN_STOP_SECS`); the close after an unfinished one
3 s (`GHOST_TURN_UNFINISHED_SECS`).

**Say the wake phrase, wait for Ghost's opening sound, then the line.** Running straight on
is a separate test (G1): the first words after the wake are dropped by the echo warm-up
guard (#54), and that would hide what the turn tests measure. `…` is a stop of about half
a second, then go on. Pause music for parts B to E; part H brings it back.

## Part A: the audio path

| | Do | Pass |
|---|---|---|
| A1 | Nothing. While Ghost listens: `pw-top -b -n 3 \| grep -E 'RATE\|ghost\|alsa_'` | The FORMAT column of every `ghost`/`alsa_` node ends in 16000, and the driver (`R` at the left, the microphone) runs at 16000. `ghost.mic`'s RATE column may say 48000: that is the quantum it asked for, not a resample. `ghost-wake-capture` is the wake's own stream (ADR 0064); on the array it carries both channels and Ghost keeps the first. The checker reads this. |
| A2 | Play music through the speakers for a minute | No crackle, dropouts or pitch change. If there is, `~/.config/pipewire/pipewire.conf.d/50-ghost-allowed-rates.conf` is the first suspect. |
| A3 | Open Slack or Meet | The microphone list is not empty. If it is, restart that app, not PipeWire (a Chromium thing). |
| A4 | Nothing. `ghostd check \| grep microphone` (ADR 0064) | The case is the machine's (`on-chip` for the array that Ghost plays through), and the wake hears what you chose: on the array, `<array>#0`. The checker shows the daemon's `microphone:` line and `audio in: ... channel 0 of 2 alone` for the wake. No `fell back` line. |
| A5 | Say "Hey Ghost" ten times at your normal voice, a few seconds apart, ending each session with "bye" | Ten wakes. The checker's wake counts: at most one hit checked by Parakeet and turned down. |
| A6 | Ask for something long ("read me the three newest issues") and say nothing while Ghost reads | No `turn:` line, no barge-in and no wake from Ghost's own voice; the session ends as you end it. With the array as input, Ghost's voice is cancelled on its chip. |
| A7 | "Hey Ghost, what's waiting for me?" in one breath | The first turn is "what's waiting for me" (`passive: woken by hey_ghost, heard ...` under debug audio): the words after the wake are read from the wake's own stream and not lost at the switch to the session's input. |
| A8 | Nothing. The checker's `canceller` lines | On the on-chip path: `ghost-echo-cancel` inactive, no `ghost.mic` node. With the canceller in use: its xruns, and no `input:degraded` note on the waiting list. If there is one, Settings › Sound › Repair, then A5 again. |

## Part B: a pause mid-sentence stays one turn

| | Say | Pass |
|---|---|---|
| B1 | "Can you count … to three." | One reply that counts. `turn: went on after a 0.xs pause`, or `turn: reopened, nothing of the answer heard: "Can you count"`. Fail: two answers, or an answer to "Can you count". |
| B2 | "Tell me a little bit more about … the weather today." | One answer. Either `Smart Turn says not done` on "about" and `went on after`, or `turn: [...] sounds unfinished, waiting`. |
| B3 | "Can you read … issue … four one five." | One turn, one `github_*_issue_read` in the log. (The number may be misheard; that is STT, #52, not turn-taking.) |
| B4 | A long sentence with three or four short pauses, as if explaining a problem | One turn, one reply. Every piece is listed under one `turn: answering "..." while the close waits`. |
| B5 | "What's waiting for triage … in the app repo?" and "Remind me tomorrow morning … to review the notes.": the first half is a sentence by itself | One reply, to the whole sentence (ADR 0049). The first half may close after the short grace (`ends a sentence and Smart Turn is sure`); then `turn: reopened, nothing of the answer heard`, or, if Ghost had just begun, `turn: reopened, the answer had only begun` and Ghost stops within a word. Fail: an answer to the first half, a tool run for it, or two replies. Note how often Ghost said anything into your pause. |

## Part C: unfinished words wait, finished sentences don't

| | Say | Pass |
|---|---|---|
| C1 | "Can you" and stay quiet | Nothing for about 3 s (`turn 3.x` in the latency line), then Ghost asks. Fail: an answer after 1.6 s. |
| C2 | "Tell me about um" and stay quiet | About 3 s, then a question back. `Smart Turn says not done` or `sounds unfinished, waiting`. |
| C3 | "What time is it?" | `Smart Turn says done (0.9x)` followed within a few ms by `turn: answering "What time is it?" while the close waits`, then `ends a sentence and Smart Turn is sure (0.9x); closing after 0.3xs`; a quick reply. |
| C4 | "All right, I'll talk to you later, Ghost." | Hangs up in about a second. The full stop gives `ends a sentence; closing 1.6s` or the quick intent fires. |

A full stop is overridden when Smart Turn is confident you are not done (`ends a sentence,
but Smart Turn is sure it's not done (0.0x); waiting`). That is by design: "Can you read
issue?" wants its number.

## Part D: the head start on answers

Five clean one-sentence questions in a row, finished, no pauses: "What's on my calendar
today?" "Read me issue four one three." "How many open issues are there?" "What did we
change yesterday?" "Say good morning."

Pass: the checker shows `speculation {"kept":1,"started":1}` on most of them, `close
confident` with `hold` about 0.35 on most (`close grace` and 1.2 where Smart Turn was less
sure or Parakeet wrote no stop; `close number` and 1.7 after "four one three"), and
`answered during it` in the latency lines. Fail: `missed` on every turn or no
`speculation` at all, which is the regression the Rust port had until PR #50. A `withdrawn`
count is fine on turns where you paused (part B). With a fast model set (ADR 0050), "Say
good morning" should come from it (`model` in the record) and the lookups should show
`handover` and an acknowledgment before the answer.

Then, for the second round's changes (2026-11-12, ADR 0049 and 0050 amended):

| | Do | Pass |
|---|---|---|
| D2 | "Say this back to me word for word: I found three issues waiting and two of them need your sign-off today." Then again with `GHOST_TTS_FIRST_CLAUSE_WORDS=0` in settings.env and `make reload` | The first time Ghost starts sooner (the latency line's `tts`), cut before "and two". Listen at that join: no gap you notice, no restart that sounds like a new sentence. Fail: a hitch or a rushed "and" you can hear; set it to 0 and note it. |
| D3 | With a fast model (`GHOST_LLM_FAST_MODEL=claude-haiku-5-5`, `GHOST_LLM_FAST_READS` on, its default): "What's waiting for triage?", "What's going on with app one forty two?", then "Mark app one fifty one ready for agent." | The two questions: `model` is the fast one in the record, no `handover`, an acknowledgment, then the right issues (the right repository and number). The change: `handover` 1 and `fast model: handed the turn over` in the log; the main model makes it. Fail: a wrong repository or number from the fast model; set `GHOST_LLM_FAST_READS=false` and note what it got wrong. |
| D4 | Optional, one session with `GHOST_STT_EARLY_READ=verify` | `early read differs` lines in the debug log, one per segment the early read would have heard differently. Count them against the segments: under one in twenty, and none where it matters (a number, a name, a full stop for a comma), is the evidence for turning it `on`. |

Then, for the third round (2026-11-12, ADR 0053 and 0054). D5 and D6 need Kokoro's halves:
`scripts/models.sh kokoro-windowed` once.

| | Do | Pass |
|---|---|---|
| D5 | `GHOST_TTS_STREAM=windowed` in settings.env, `make reload`. "Tell me a long sentence about the weather, about thirty words." Then the same with `GHOST_TTS_STREAM=whole` | Windowed, the voice starts sooner (the latency line's `tts` near 0.2 s against 0.6 s or more) and you hear no click, no gap and no change of level or timbre a few tenths of a second in, where the windows join. It is the same voice. Fail: a step in loudness, a buzz or a different timbre at the start; set it back to `whole` and note where. |
| D6 | Windowed: "Say good morning." Then "Read me the three most recent issues." and say "stop" halfway | The short reply sounds exactly as it does whole (one window). The long one stops within a word of "stop", as it does whole. |
| D7 | `GHOST_STT_EARLY_READ=speculate` with a fast model set; the five questions of part D | `speculation` shows `early` and `early_same` on most turns and `kept`; the answers are to what you said, word for word in the transcript, as without it. An `early_differs` is fine if the answer was to the stop's words. Fail: an answer to words you didn't say, or a turn whose transcript differs from the session without it. |

## Part E: interrupting still works

| | Do | Pass |
|---|---|---|
| E1 | "Count to twenty slowly", then after a few numbers: "Stop, that's enough." | Ghost stops within a second and answers you. `turn: barge-in on interim` and `cut off 0.xs after the operator began`. Fail: your words merge into the counting request. |
| E2 | One word, "Okay", while Ghost is talking | Ghost keeps talking (cutting it off takes two words). `turn: dropped "Okay" while Ghost speaks`. |
| E3 | Through the whole run, watch for turns Ghost starts from its own words | No `turn:` line whose text is a sentence Ghost just said. |

## Part F: the screens

| | Look at | Pass |
|---|---|---|
| F1 | Desktop, Session screen, after B4 | One bubble for the whole sentence, not one per piece. (Capitals mid-sentence and "um" kept: #55.) |
| F2 | Desktop, Agents screen, `ghost`'s history | The same. |
| F3 | The phone app's captions (optional) | The split sentence is one line and Ghost's reply still fits. |

## Part G: running on after the wake

| | Say, in one breath | Pass |
|---|---|---|
| G1 | "Hey Ghost, can you read issue four one five." | The turn starts with "can you read", not with your repeat. Fail: `echo: dropped "Can you" (inside the canceller's warm-up after Ghost's voice resumed)` and `passive: woken by hey_ghost, heard "..."` holding the words that were then lost (#54). |

## Part H: with music

Play music through the speakers, then B1 and C3 again, and three wakes.

Pass: the same lines as before, and no `heard nothing in a 1.4s segment at -1x dBFS` lines
while you are not talking (music residue opening segments). Wakes: `detector heard hey_ghost
(0.6+)` on the first or second try. Note what Parakeet's check read on any hit under 0.6
(`detector hit not confirmed: "..."`): under music it writes the phrase as "a ghost", "it
goes", "there it goes" (#13).

## Reading the results

The checker prints, per part, the lines above with counts, and for part D one line per
turn from `~/.local/share/ghost/latency.jsonl`: time, surface, hold, segments, speculation,
reply. `make latency LAST=30` gives the per-stage numbers. When something fails, note the
item and the time of day; the log has everything around it.

## Runs

### 2026-10-31, main at 3fb4af0 (PR #50, PR #53), reSpeaker XVF3800 as mic and speakers

| Item | Result | Note |
|---|---|---|
| A1 | pass | every node 16000; driver 128/16000 |
| A2 | pass | music through the array, operator's ear |
| A3 | not run | |
| B1 | pass | `went on after a 0.32s pause`; STT heard "count um Two three" (dropped "to") |
| B2 | pass | two pauses, one turn, every word heard |
| B3 | pass | pauses of 1.40 s and 0.32 s, one turn, one read; number misheard "four oh oh five" |
| B4 | pass | six pieces, five pauses to 0.86 s, one turn |
| C1 | pass | 3.10 s of silence before anything |
| C2 | pass | 3.14 s; Smart Turn carried it (0.01) |
| C3, C4 | not run | the goodbyes in B and C sessions all hung up in 0.7 to 1.6 s |
| D | partial | every turn so far `answered during it`, `speculation kept`; five clean questions not yet run |
| E1 | pass | "Okay you can stop" over a reply: `cut off 0.91s after the operator began` |
| E2, E3 | not run | no self-turn seen in any session |
| F1 | pass | one bubble; case and fillers wrong (#55) |
| F2, F3 | not run | |
| G1 | fail | two of four early tests lost their opening words to the echo warm-up (#54) |
| H | partial | wakes under music took two to four tries; Parakeet's check read "There it goes", "A ghost. A ghost." (#13); `heard nothing` segments at -14 to -22 dBFS while music was audible |

Also seen: Ghost said goodbye twice after a model `hang_up` (#56); the echo canceller's
`ghost.mic` node accumulated 111 xruns over 30 minutes while the wake scores fell (#58), and a canceller restart moved the
graph to 48 kHz until `clock.force-rate 16000` brought it back (output silent meanwhile).
