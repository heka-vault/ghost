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
Smart Turn hears an end 1.2 s (`GHOST_TURN_GRACE_SECS`); the close after a finished
sentence when Smart Turn did not hear an end 1.6 s (`GHOST_TURN_STOP_SECS`); the close
after an unfinished one 3 s (`GHOST_TURN_UNFINISHED_SECS`).

**Say the wake phrase, wait for Ghost's opening sound, then the line.** Running straight on
is a separate test (G1): the first words after the wake are dropped by the echo warm-up
guard (#54), and that would hide what the turn tests measure. `…` is a stop of about half
a second, then go on. Pause music for parts B to E; part H brings it back.

## Part A: the audio path

| | Do | Pass |
|---|---|---|
| A1 | Nothing. While Ghost listens: `pw-top -b -n 3 \| grep -E 'RATE\|ghost\|alsa_'` | The FORMAT column of every `ghost`/`alsa_` node ends in 16000, and the driver (`R` at the left, the microphone) runs at 16000. `ghost.mic`'s RATE column may say 48000: that is the quantum it asked for, not a resample. The checker reads this. |
| A2 | Play music through the speakers for a minute | No crackle, dropouts or pitch change. If there is, `~/.config/pipewire/pipewire.conf.d/50-ghost-allowed-rates.conf` is the first suspect. |
| A3 | Open Slack or Meet | The microphone list is not empty. If it is, restart that app, not PipeWire (a Chromium thing). |

## Part B: a pause mid-sentence stays one turn

| | Say | Pass |
|---|---|---|
| B1 | "Can you count … to three." | One reply that counts. `turn: went on after a 0.xs pause`, or `turn: reopened, nothing of the answer heard: "Can you count"`. Fail: two answers, or an answer to "Can you count". |
| B2 | "Tell me a little bit more about … the weather today." | One answer. Either `Smart Turn says not done` on "about" and `went on after`, or `turn: [...] sounds unfinished, waiting`. |
| B3 | "Can you read … issue … four one five." | One turn, one `github_*_issue_read` in the log. (The number may be misheard; that is STT, #52, not turn-taking.) |
| B4 | A long sentence with three or four short pauses, as if explaining a problem | One turn, one reply. Every piece is listed under one `turn: answering "..." while the close waits`. |

## Part C: unfinished words wait, finished sentences don't

| | Say | Pass |
|---|---|---|
| C1 | "Can you" and stay quiet | Nothing for about 3 s (`turn 3.x` in the latency line), then Ghost asks. Fail: an answer after 1.6 s. |
| C2 | "Tell me about um" and stay quiet | About 3 s, then a question back. `Smart Turn says not done` or `sounds unfinished, waiting`. |
| C3 | "What time is it?" | `Smart Turn says done (0.9x)` followed within a few ms by `turn: answering "What time is it?" while the close waits`; a quick reply. |
| C4 | "All right, I'll talk to you later, Ghost." | Hangs up in about a second. The full stop gives `ends a sentence; closing 1.6s` or the quick intent fires. |

A full stop is overridden when Smart Turn is confident you are not done (`ends a sentence,
but Smart Turn is sure it's not done (0.0x); waiting`). That is by design: "Can you read
issue?" wants its number.

## Part D: the head start on answers

Five clean one-sentence questions in a row, finished, no pauses: "What's on my calendar
today?" "Read me issue four one three." "How many open issues are there?" "What did we
change yesterday?" "Say good morning."

Pass: the checker shows `speculation {"kept":1,"started":1}` on most of them, `hold` about
1.2, and `answered during it` in the latency lines. Fail: `missed` on every turn or no
`speculation` at all, which is the regression the Rust port had until PR #50. A `withdrawn`
count is fine on turns where you paused (part B).

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
`ghost.mic` node accumulated 111 xruns over 30 minutes and a canceller restart moved the
graph to 48 kHz until `clock.force-rate 16000` brought it back (output silent meanwhile).
