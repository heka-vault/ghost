---
name: voice-rubric
description: >-
  Run the voice rubric with the operator at the desk: start Ghost in debug mode, walk
  them through docs/voice-rubric.md one line at a time, read the log after each, record
  the run and file what failed. Use when asked to run the rubric, to test the voice or
  turn-taking by talking to Ghost, or to check a voice change live before or after it merges.
---

# The voice rubric, run

[docs/voice-rubric.md](../../../docs/voice-rubric.md) is the script: parts A to H, the
line to say for each item and the debug line that marks it passed. This skill is how a run
goes. The operator talks; you drive the terminal, read the log, keep score. Every verdict
comes from a log line, never from how it sounded.

## 1. Start

1. Say which commit is under test (`git log --oneline -1`) and that the checkout is on the
   branch to test. The daemon is the working tree's build.
2. Stop whatever Ghost runs: `make stop`; if a `make dev` is up, `kill -INT` its
   `bash scripts/dev.sh` and wait until no `target/debug/ghostd run` or
   `heka-ghost-desktop` is left. One Ghost may have the microphone.
3. `GHOST_DEBUG_AUDIO=1 make dev > /tmp/ghost-dev.log 2>&1` in the background with the
   longest timeout you have (it is stopped at that limit; restart it if the run is still
   going). Wait for `audio in: streaming`, then confirm `GHOST_DEBUG_AUDIO=1` in the
   daemon's `/proc/<pid>/environ` and `desktop ghost: connected`.
4. `since.sh --mark` (this folder) sets the log mark. Run `scripts/voice-rubric-check.sh`
   once: part A passes or fails before anyone speaks.
5. Tell the operator: say the wake phrase, wait for the opening sound, then the line; `…`
   is a half-second stop; music off until part H.

## 2. Walk

One item per message. Give the exact line from the rubric and the pass mark in one
sentence, ask for "done", and stop. When they say done:

1. `since.sh` prints every turn, wake, echo, tool and latency line since the mark and
   moves the mark. Read it all before judging.
2. Verdict from the rubric's lines: **pass**, **fail**, or **pass with a note** when the
   turn logic did its part and something else went wrong (the words misheard, a slow
   model, a double goodbye). Name the line that decided it, with its time.
3. Anything unexpected in the lines is a finding, whether or not the item passed: a word
   dropped by `echo:`, a `heard nothing in a 1.4s segment at -14 dBFS`, a wake that took
   three tries, an xrun count climbing in `pw-top`. Keep a running list with times.
4. Then the next item.

When the operator reports something the log does not show (no audio, a wake that never
fired), read the machine before changing anything: `pw-top -b -n 2` for the driver's
quantum/rate and the ERR column, `pw-metadata -n settings` for the graph rate,
`pactl list sink-inputs`, the canceller's `systemctl --user status ghost-echo-cancel`.
A daemon restart is `make reload`; a canceller restart is `systemctl --user restart
ghost-echo-cancel`, then `make reload` so the daemon reopens `ghost.mic`; both with the
operator's say-so, and both recorded as findings with the before and after numbers.

## 3. Collect

After the last item, or when the operator stops early:

1. `scripts/voice-rubric-check.sh` and `make latency LAST=30`; keep both outputs.
2. Stop Ghost: `kill -INT` the `bash scripts/dev.sh`, wait for nothing left, undo any
   live audio setting you changed (`pw-metadata -n settings 0 clock.force-rate 0`).
   Debug audio ends with the process; nothing to clean.
3. Every fail and every finding that is not already an open issue gets one: the log lines
   verbatim with their times, the commit, the mic and speakers, and the cause as far as
   the code shows it (`gh issue create --label needs-triage`). A finding that matches an
   open issue is a comment there with the new evidence. Say the numbers in the reply.

## 4. Record

Append the run to **Runs** in `docs/voice-rubric.md`: the date, the commit, the mic and
speakers, one row per item with its verdict and the deciding line's gist, and an "Also
seen" paragraph with the issue numbers. Items not run say so. Commit on a branch
(`rubric/<date>`) from a worktree, push, open the PR with the checker's PASS/FAIL lines
as its evidence, and link it. The run is complete when that PR exists and every finding
has an issue number.

## Lessons the log taught

- The echo guard drops the operator's first words when they run straight on after the
  wake (#54). Leaving the gap keeps the turn tests clean; part G measures the run-on.
- `pw-top`'s RATE column is the quantum a node asked for; FORMAT is its audio's rate. A
  graph at 48 kHz shows the driver at `256/48000`; at 16 kHz, `128/16000`.
- A canceller restart can move the graph to 48 kHz and leave the reSpeaker's output
  xrunning and silent; `clock.force-rate 16000` brings it back live.
- `pgrep -f 'target/debug/ghostd run$'` and `'scripts/dev.sh'` match your own command
  line when it quotes them; anchor the pattern or exclude `$$`.
