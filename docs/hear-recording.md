# Recording your hear takes

The [hear harness](benchmarks/hear-harness.md) scores how well Ghost turns your speech into
words. Kokoro can stand in for your voice, but only your own voice through your own mic,
room and echo canceller says how Ghost hears *you*. About 60 takes, recorded once in about
15 minutes, are that test set. Every change to the speech-in path ([#60](https://github.com/heka-vault/ghost/issues/60), [#52](https://github.com/heka-vault/ghost/issues/52)) is then
scored against them.

## Privacy

- You record every take deliberately, with a countdown. Nothing is taken from Ghost's
  passive audio.
- The takes stay on this machine under `.scratch/hear-takes/`, which git ignores. They are
  never uploaded or committed.
- The recorder plays nothing through the speakers.
- Scoring runs offline and writes numbers only; `ARGS=--show-text` prints what Ghost heard
  to the terminal.

## Before you start

1. A quiet-ish room: no TV, nobody else talking. Block G adds music on purpose.
2. The mic array plugged in, and `ghost-echo-cancel` running (`systemctl --user is-active
   ghost-echo-cancel` says `active`). Without it there is no `ghost.mic` and the recorder
   stops.
3. Ghost stopped, so it doesn't hear the takes and answer. If `ghostd` runs as a service
   the recorder offers to stop it and starts it again when you finish. If you started it
   with `make dev` or `make start`, or it is inside the desktop app, run `make stop` or
   quit the app first; the recorder checks.

## Recording

1. Look at the plan first: `make record-hear ARGS=--dry-run`. It lists every take and
   records nothing.
2. Run `make record-hear`. Each block shows its instruction once; then each take shows the
   exact line.
3. Press Enter. The screen counts down 3-2-1, then shows "speak now". Say the line once.
   Each take records for a fixed time, a little longer than the line needs: stop talking
   and wait for "done".
4. After each take the recorder prints the `ghost.mic` level, and warns when it is too
   quiet (below −35 dBFS peak) or clipping.
5. Then one key: Enter records the next take, `r` redoes the last one, `s` skips this one,
   `q` quits. Redo a take whenever it went wrong: a cough, a stumble, speaking before
   "speak now", or a word that isn't the line's.
6. `q` or Ctrl+C stops; everything so far is kept, and `make record-hear` again carries on
   from the next take. `BLOCKS=A,D` records only those blocks; `ARGS="--session
   .scratch/hear-takes/<date>"` continues an older session.

| Block | What it asks | Takes | Why |
|---|---|---|---|
| A | At the desk, normal voice: everyday requests with issue numbers and names | 10 | The baseline |
| B | A pause of about a second mid-sentence, where the line shows "... (pause) ..." | 8 | Today each piece is read alone (#52 item 1) |
| C | "Um", "uh", restarts and changes of mind, said as written | 6 | Fillers (#55) and corrections ("twelve, no wait, thirteen") |
| D | One- and two-word replies: "Yes.", "Stop.", "Not yet." | 8 | Short answers Ghost must not drop |
| E | Soft or tired voice | 5 | Less signal for the VAD and the model |
| F | Across the room, 3 m or more (8 s countdown to walk over) | 6 | Distance and the room's echo |
| G | Music on the desk speakers at your usual level (you start it); the last three takes silent | 9 | What the canceller leaves, and words made up from music |
| H | Saying nothing: sitting still, typing, moving about, a breath or a sigh | 10 | Words made up from the room (#58, #60 item 6) |

## Tips

- Speak the way you talk to Ghost on an ordinary day. Don't over-enunciate.
- In block B, make the pause a real thinking pause, about a second. Not a dramatic one.
- In block C, say the fillers naturally. They are left out of the score; the words around
  them aren't.
- In block H, do what the line says and nothing else. A word said by mistake turns a silent
  take into a speech take: redo it with `r`.

## Where the files go

`.scratch/hear-takes/<YYYY-MM-DD>/`:

- `<id>-<kind>.aec.wav`: `ghost.mic`, mono, 16 kHz. What Ghost hears, and what is scored.
- `<id>-<kind>.raw.wav`: the raw array under `ghost.mic`, 2 channels, 16 kHz, for #52
  item 2 (`ARGS="--channel raw"`).
- `takes.jsonl`: one line per take: the files, block, condition, kind, the line you were
  asked to say, its tags and terms, the levels and the time. The harness scores each take
  against the line recorded here, so a session stays valid if the plan changes later.

## Scoring them

```sh
make perf-hear TAKES=latest                    # the newest session; or TAKES=.scratch/hear-takes/<date>
make perf-hear TAKES=latest ARGS=--show-text   # and print what each read heard
```

The report lands in `.scratch/perf/<timestamp>-hear/report.md`; how to read it, and the
pass rule a change must meet, are in [benchmarks/hear-harness.md](benchmarks/hear-harness.md).
Keep the first run's `results.json`: it is the baseline the next change is compared to.
