# How Ghost hears: the hear harness

`make perf-hear` measures how well Ghost turns the operator's speech into words, so a
change to the speech-in path ([#60](https://github.com/heka-vault/ghost/issues/60), [#52](https://github.com/heka-vault/ghost/issues/52)) can be called better or worse
with numbers, not by ear. `make perf` measures how long a turn takes; this measures what it
heard. The code is `crates/ghost-voice/src/perf/hear/`; the takes are
[hear-plan.json](hear-plan.json).

## What it runs

Each take goes through the real speech-in stage (`ghost_voice::stt::ParakeetStt`: the real
Silero VAD with the session's settings, the real Parakeet, the dither, the pre-roll), fed
as fast as the models go on the audio's own clock, so Silero resets every 5 s of audio as
it does live. Each take is heard three ways:

| Read | What it is |
|---|---|
| `stage` | What the stage hands the aggregator: every final of the take joined with a space, as `turns` joins them. Today each VAD segment is read alone. When the stage changes, this is the column that moves. |
| `turn` | The turn's audio, from the first segment's pre-roll to the last segment's end, read once: what the aggregator hands the model for a turn of two segments or more (ADR 0044, `GHOST_TURN_REREAD`; the harness reads the span itself, so `stage` against `turn` is that change's measure). |
| `file` | The whole take read once, no VAD: what Parakeet can do with the audio. |

The takes come from one of two places:

- **Recorded** (`TAKES=latest` or a session directory): the operator's own voice through
  `ghost.mic`, recorded in the desktop app's Hearing test ([../hear-recording.md](../hear-recording.md),
  ADR 0042). The app scores a session too (Runs), and compares two runs with the same pass rule.
  `ARGS="--channel raw"` scores the raw array under `ghost.mic` instead (#52 item 2).
- **Rendered** (no `TAKES`): the plan's desk blocks spoken by Kokoro over −60 dBFS of
  room noise, in `ARGS="--voices am_michael,af_heart,bm_george"`. Distance and music are
  the room's, so blocks F and G are recordings only. Rendered takes are clean speech: they
  find what the cutting does, not what the canceller or the room does.

## What it scores

Both sides are normalized the same way (`perf::hear::score::normalize`): lowercase,
punctuation gone, "sign-off" as two words, fillers (um, uh, er, hmm) left out of every
count, and spoken numbers as digits ("four oh three", "403" and "4 0 3" are all `403`;
"one forty two" and "one hundred forty two" are `142`). The words are aligned with the
fewest edits, ties going to the alignment a person would draw (most words matched, a
number substituted for a number).

| Measure | Over | What it catches |
|---|---|---|
| word error rate | requests and replies | everything, (substituted + dropped + inserted) / words said |
| number, term, function-word, other-word errors | requests and replies | a change that helps on average but breaks issue numbers or names |
| terms heard, terms made up | takes with terms | names Ghost must hear (Mobbin, GitHub, heka vault), and a term list that inserts them where they weren't said (#60 item 5) |
| empty reads, replies dropped | requests and replies | "Not yet." read as nothing |
| silent takes with words, made-up words | silent takes | "Thank you" from typing or room noise (#13, #58, #60 item 6) |
| segments per take, takes split in pieces | `stage` | how often the VAD cuts a sentence |
| read at the close, p50 / p95 | requests and replies | what the read costs once the operator stops: the stage's last final, or the one read of `turn` and `file` |
| model time per second of audio | all | the CPU the reads cost |

Results (`results.json`) hold numbers only. `ARGS=--show-text` prints what each read heard
to the terminal; nothing writes a transcript to disk.

## Comparing, and the pass rule

Two runs, or two reads in one run, are compared take by take. Each measure's change gets a
95% interval from a paired bootstrap over the takes (4,000 resamples, a fixed seed, so a
report renders the same every time). A change is **better** when the whole interval is
below zero, **worse** when it is above, and **within noise** otherwise. Per take, the
report also counts fewer errors / the same / more; "more" is the damage rate.

Against a baseline (`BASELINE=.scratch/perf/<run>/results.json`), the takes are paired by
id and source, and the **pass rule** applies to `stage`, the read Ghost ships:

1. no measure in the table is worse beyond noise, and
2. the close's p95 has not grown by more than the budget (150 ms;
   `ARGS="--close-budget-ms N"`).

`make perf-hear` exits 1 when the rule fails. `ghostd perf hear-report RESULTS --baseline
BASE` renders a comparison again without running anything.

A change to the speech-in path is measured like this:

```sh
make perf-hear TAKES=latest                       # on main: the baseline
# ...make the change...
make perf-hear TAKES=latest BASELINE=.scratch/perf/<baseline>-hear/results.json
```

and the same on the rendered takes, against the committed baseline in [hear/](hear/).
`ARGS="--set vad_stop_secs=0.4"` tries a setting without a code change; `--set` takes
the same `GHOST_*` names as `make perf`, `stt_dir` and `stt_quantization` included.

## When the turn would have closed: `make perf-turns`

The same takes answer a second question: when would Ghost have taken the turn for over?
`make perf-turns` (`ghostd perf turns`, `crates/ghost-voice/src/perf/turns.rs`) runs each
take through the stage as above and judges every segment's end as a session does: Smart
Turn on the audio, the words the aggregator would hold then (the whole turn read once
from the second segment on), and the wait the rule gives that end
(`ghost_voice::turns::TurnTiming`, ADR 0049). A take is one turn as the operator meant
it, so every end but its last is a pause mid-turn of known length, and a wait shorter
than the pause is a turn closed too soon.

```sh
make perf-turns TAKES=latest                              # the recorded takes, the settings as they are
make perf-turns TAKES=latest ARGS="--set turn_confident_prob=0.95"
ghostd perf turns-report .scratch/perf/<run>-turns/turns.json --set turn_confident_grace_secs=0.5
```

The report (`turns.md`) sets the waits as they are beside the one grace Ghost had before:
pauses closed early and how many of them the short grace closed, how many of those are
still closed when Ghost's voice could start (0.8, 1.2 and 2.0 s after the VAD's stop; the
others are reopened whole with nothing heard), and the wait at each take's real end, which
is what a shorter grace buys. It sweeps the short grace and its probability, and lists
every pause with Smart Turn's verdict and what the words read as. `turns.json` holds
numbers and flags, never words. The harness's pauses include the VAD's usual 0.2 s start,
where a live session re-arms in 0.1 s while a close waits, so the counts err toward more
early closes.

## What it does not measure

- Turn-taking live: `make perf-turns` says when the turn would have closed, not what
  happened next; whether the operator going on was reopened as one turn is the rule's
  tests and [../voice-rubric.md](../voice-rubric.md) part B.
- Ghost talking over the operator: no take plays Ghost's voice, so the echo filter, the
  warm-up guard and barge-in (#54, #58) are the rubric's.
- The model's answer: a word wrong in a way the model still understands counts as wrong.

## Results

| Run | What |
|---|---|
| [hear/2026-11-05-kokoro.md](hear/2026-11-05-kokoro.md) | The first run, rendered takes in three voices: reading the whole turn takes the word error rate from 7.2% to 4.2% (better beyond noise) for 0.05 s more at the close's p95 |
| [perf/2026-11-12-voice-speed.md](perf/2026-11-12-voice-speed.md) | `make perf-turns` on the recorded session of 2026-11-06: the short grace closes 3 of 39 pauses mid-turn early against 1, and a turn's real end waits 0.85 s on average against 1.37 s |
