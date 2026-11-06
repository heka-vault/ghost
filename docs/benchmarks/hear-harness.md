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
| `turn` | The turn's audio, from the first segment's pre-roll to the last segment's end, read once: the whole-turn re-read of #52 item 1, measured before it is built. |
| `file` | The whole take read once, no VAD: what Parakeet can do with the audio. |

The takes come from one of two places:

- **Recorded** (`TAKES=latest` or a session directory): the operator's own voice through
  `ghost.mic`, recorded with `make record-hear` ([../hear-recording.md](../hear-recording.md)).
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

## What it does not measure

- Turn-taking: whether Smart Turn closed the turn after the first piece. Each take is one
  turn here; [../voice-rubric.md](../voice-rubric.md) part B is the live check.
- Ghost talking over the operator: no take plays Ghost's voice, so the echo filter, the
  warm-up guard and barge-in (#54, #58) are the rubric's.
- The model's answer: a word wrong in a way the model still understands counts as wrong.

## Results

| Run | What |
|---|---|
| [hear/2026-11-05-kokoro.md](hear/2026-11-05-kokoro.md) | The first run, rendered takes in three voices: reading the whole turn takes the word error rate from 7.2% to 4.2% (better beyond noise) for 0.05 s more at the close's p95 |
