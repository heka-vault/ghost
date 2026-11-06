# The Hearing test: recording your takes

The [hear harness](benchmarks/hear-harness.md) scores how well Ghost turns your speech into
words. Kokoro can stand in for your voice, but only your own voice through your own mic,
room and echo canceller says how Ghost hears *you*. About 60 takes, recorded once in about
15 minutes, are that test set. Every change to the speech-in path ([#60](https://github.com/heka-vault/ghost/issues/60), [#52](https://github.com/heka-vault/ghost/issues/52)) is then
scored against them. You record them in the desktop app: **Hearing test** in the sidebar
([ADR 0042](adr/0042-the-operator-records-takes-to-test-how-ghost-hears.md)).

## Privacy

- You record every take on purpose: a take is only what you hold the key for (or start
  yourself). Nothing is taken from Ghost's passive audio.
- While the recorder is open, Ghost is not listening for its name; the sidebar says so.
  Leaving the Record tab, closing the window, or ten minutes without a take gives the mic
  back.
- The takes stay on this machine under `~/.local/share/ghost/hear/`. They are never
  uploaded, committed or sent to a model.
- What Ghost heard in a take is shown on the screen once and never written anywhere. The
  manifest keeps the line you were asked to say, the levels and where the speech was.
- Takes play back through Ghost's own speaker.

## Before you start

1. A quiet-ish room: no TV, nobody else talking. Block G adds music on purpose.
2. The mic array plugged in, and `ghost-echo-cancel` running (`systemctl --user is-active
   ghost-echo-cancel` says `active`). Without it there is no `ghost.mic` and the recorder
   won't open.
3. Ghost running and not in a session (end a session first; the recorder says so).

## Recording

Open **Hearing test → Record**. It opens a session for today (or carries on today's);
**New session** starts another, and **Sessions → Continue** goes back to an older one.

Each block shows its instruction; then each take shows the line, big.

- **A line at the desk: hold to talk.** Hold Space (or the button) for the whole line,
  pauses too, and let go when you finish. Half a second before you press and half a second
  after you let go are kept, so a late press or an early release doesn't cut a word.
  A tap shorter than a quarter second is thrown away.
- **Across the room (block F): press once.** Space starts a countdown long enough to walk
  3 m away; speak when it reaches zero. The take ends by itself two seconds after your
  last word.
- **Silent takes (block H, the end of block G): press once.** A short countdown, then the
  take runs for its time. Do what the line says and nothing else.
- **Escape** cancels a take that is going.

After each take Ghost reads it at once and shows **what it heard**, word by word against
the line: missed words struck through, misheard ones with what was heard, extra ones
marked. A bar shows where the speech was in the take. Warnings say when to redo it:
speech that ran into the start or the end of the take, the line's last words not heard, no
speech, clipping, or speech in a silent take.

Then: **Enter** for the next take, **R** to redo it (just record again; the new one
replaces it), **P** to play it back, **S** to skip it. Click any take in the list on the
right to go to it; a warning sign marks the ones worth a redo.

| Block | What it asks | Takes | Why |
|---|---|---|---|
| A | At the desk, normal voice: everyday requests with issue numbers and names | 10 | The baseline |
| B | A pause of about a second mid-sentence, where the line shows "... (pause) ..." | 8 | Today each piece is read alone (#52 item 1) |
| C | "Um", "uh", restarts and changes of mind, said as written | 6 | Fillers (#55) and corrections ("twelve, no wait, thirteen") |
| D | One- and two-word replies: "Yes.", "Stop.", "Not yet." | 8 | Short answers Ghost must not drop |
| E | Soft or tired voice | 5 | Less signal for the VAD and the model |
| F | Across the room, 3 m or more | 6 | Distance and the room's echo |
| G | Music on the desk speakers at your usual level (you start it); the last three takes silent | 9 | What the canceller leaves, and words made up from music |
| H | Saying nothing: sitting still, typing, moving about, a breath or a sigh | 10 | Words made up from the room (#58, #60 item 6) |

## Tips

- Speak the way you talk to Ghost on an ordinary day. Don't over-enunciate.
- Press, then speak; finish, then let go. The half second either side covers the rest.
- In block B, make the pause a real thinking pause, about a second, and keep holding.
- In block C, say the fillers naturally. They are left out of the score; the words around
  them aren't.

## Scoring

**Runs → Score** scores a session on `ghost.mic` or on the raw array beneath it (#52 item
2). The run keeps the mic until it finishes, a few seconds for a whole session. A run's
report shows the three reads side by side, by block, by class and the takes to listen to
(each with a play button). **Compare with** another run shows what changed and the pass
rule a change must meet ([benchmarks/hear-harness.md](benchmarks/hear-harness.md)): keep
the run on `main` as the baseline, make the change, score again, and compare. A note on
each run says what it was for.

From a terminal the same takes are scored with:

```sh
make perf-hear TAKES=latest                    # the newest session
make perf-hear TAKES=latest ARGS=--show-text   # and print what each read heard
ghostd perf hear-check ~/.local/share/ghost/hear/sessions/<date>   # re-check every take
```

## The take list

**Take list** edits the lines: add, change or drop takes and blocks. Ghost checks the list
before saving it (unique ids that start with their block, a line for every take that isn't
silent, each term in its line). **Reset** goes back to the list Ghost ships
([benchmarks/hear-plan.json](benchmarks/hear-plan.json)). A take's id never changes once
recorded: give a new line a new id. Each take keeps the line it was recorded with, so a
session stays valid when the list changes later.

## Where the files go

`~/.local/share/ghost/hear/`:

- `plan.json`: your take list, once you edit it.
- `sessions/<YYYY-MM-DD>[-n]/`: `<id>-<kind>.aec.wav` (`ghost.mic`, mono, 16 kHz: what
  Ghost hears, and what is scored), `<id>-<kind>.raw.wav` (the raw array under
  `ghost.mic`, 2 channels, 16 kHz), and `takes.jsonl`, one line per take: the files, the
  block, the line you were asked to say, its tags and terms, the levels, where the speech
  was, how the take ended, the check's warnings by name, and the time.
- `runs/<stamp>/`: `results.json` (numbers only), `report.md`, and `run.json`.
