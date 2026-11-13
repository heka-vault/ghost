# Wake takes: Ghost learns your voice

Ghost keeps the sound of each time it thinks it heard its name, on this computer, to learn
your voice ([ADR 0065](adr/0065-ghost-keeps-its-wake-attempts-to-learn-the-operators-voice.md)).
The wake detector was trained on synthetic voices; your own "hey Ghost", through your own
microphone, is what makes it hear you reliably.

## What is kept

Only when the trained detector (`GHOST_WAKE_MODE=onnx`) scores something as its name and the
decision is made. Transcript wake, which a fresh install uses, has no detector score and keeps
no takes: Ghost keeps nothing it hears before its name. The first run and Settings › Sound say
the keeping line, with its switch, only while the detector is the wake (the status's
`wake_mode`, which turns to `transcript` when the detector cannot load); takes kept earlier stay
there to delete.

What is kept for each decided hit:

| | Woke (accepted) | Turned down by Parakeet's check (refused) |
|---|---|---|
| Sound, from the stream the detector hears | 12 s before to 1.5 s after | 4 s before to 1.5 s after |
| Sound from the array's own stream, the same moment | when the detector hears `ghost.mic` | the same |
| The detector's score every 80 ms, the score, the input, the time | yes | yes |
| What Parakeet read for it | yes | yes |

The 12 s before a wake holds an attempt Ghost missed, when you had to say it again.

A take is shorter only when the listener had just started: right after a conversation ends,
Ghost restarts, or listening is turned back on. What came before was the conversation (or
nothing), not a wake attempt, so it is not kept; the take is marked `cut_at_start` and the
Hearing test says it begins where Ghost started listening.

Files are in `~/.local/share/ghost/wake/takes/` (0600, the folder 0700): about 1.3 MB for a
wake with the array beside it, 0.5 MB for a refused hit. At most 2,000 takes or 1 GB; the
oldest go first. Nothing between hits is kept, and nothing is sent anywhere. The phone, the
outside tools you connected and every model are never offered them.

## Turning it off, deleting

Settings › Sound › *Wake takes*: the switch turns keeping off (what is there stays until you
delete it); *Delete* removes every take and the folder. Each take can be deleted on its own
in the Hearing test › *Wake takes*.

## Labelling (two minutes a day)

Hearing test › *Wake takes*. Click the list, then one key per take; the next unlabelled one
plays through Ghost's speaker at once.

| Key | |
|---|---|
| `Y` | me calling Ghost |
| `N` | not a wake (music, the TV, someone else, Ghost itself) |
| `B` | me calling Ghost, and I had called just before and it missed |
| `U` | clear the label |
| `Space` / `A` | play again / play the array's own stream |
| `↓` `↑` | next, previous |
| `Del` | delete this take |

*Count unlabelled wakes as me* lets the retraining treat a wake you never labelled as you.
A refused hit you did not label is never used.

## Retraining, on this computer

```
make wake-train
```

It reads your labelled takes, the takes recorded on 2026-10-30 (`tools/wake-train/sets.txt`),
synthetic voices it makes with Kokoro and some music, adds level, noise, room, speed and
compression changes, and trains a small head beside the shipped detector on the CPU (about
40 minutes here, half of it only the first time; the features are cached in
`~/.local/share/ghost/wake/train-work/`). Whole recordings are held out and scored by a model
that never heard them. It picks the threshold from the new detector's own curve: the lowest
at which it wakes no more often than the shipped detector at 0.6 on the training negatives.
It writes `~/.local/share/ghost/wake/detectors/<name>/` (`hey_ghost.onnx`, `card.json`,
`report.md`) and installs nothing. When no threshold of the new detector wakes no more often and asks Parakeet no more often than the shipped one, it says so and writes only `report.md`: keep labelling and run it again. On the 21 takes of 2026-10-30 alone it does not beat the shipped detector ([2026-11-13](benchmarks/2026-11-13-wake-takes.md)).

## Switching, and back

```
make wake-detectors                      # every detector, its numbers, * the one in use
make wake-use NAME=operator-20261113-0900
make reload                              # or restart the app
make wake-rollback                       # back to the shipped detector
```

`ghostd check` (and the daemon at start) refuses a detector whose file no longer matches its
card or whose card shows more false wakes than the shipped detector on any set.

**Before switching, read `report.md`:**
- *Cross-check*, your takes on `ghost.mic` and on the array: woke at once + checked by
  Parakeet, out of all takes. Higher is better; "woke at once" matters most.
- *False wakes* on every negative set (your speech, Ghost talking, music, the Kokoro rig and
  its confusables): after must be no higher than before on every row.
- *checked*: how often Parakeet is asked about something that was not you; lower is better.

## For the README's privacy section

> **Wake takes.** Ghost keeps the sound of each time it thinks it heard its name, on this
> computer, to learn your voice: a few seconds around each wake and each near miss, what it
> read, and how sure the detector was. It is on by default and never leaves the computer.
> Turn it off or delete everything in Settings › Sound; label takes in the Hearing test and
> run `make wake-train` to retrain the detector on your own voice
> ([docs/wake-takes.md](docs/wake-takes.md)).
