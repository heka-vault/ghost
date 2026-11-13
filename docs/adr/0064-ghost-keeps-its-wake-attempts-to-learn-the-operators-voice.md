# 0064: Ghost keeps its wake attempts to learn the operator's voice

- **Status:** accepted
- **Date:** 2026-11-13
- **Amends:** ADR 0010 and the idle-audio rule in CLAUDE.md, for wake attempts only.

On 2026-11-12 about half of the operator's live "hey ghost"s scored 0.32 to 0.59 against the
0.60 threshold ([wake check](../benchmarks/2026-11-12-wake-check.md), issue #101). The
detector was trained on synthetic voices; the operator's own voice, through their own
microphone path, is the evidence a better detector needs, and the 21 takes recorded on
2026-10-30 are too few. The owner asked that Ghost, as it runs, keep its wake attempts so
the detector can be retrained on them, and then (2026-11-12, 23:45) that it be on by default:
"Until we are 9/10 with our competitors, let's be bolder about finding solutions and how we
obtain data we need for the solutions. It's my workstation, I'm the only person using it.
Turn retention on by default for now."

## Decision

**Ghost keeps the sound of each time it thinks it heard its name, on this computer**
(`ghost_core::wake_takes`, `ghostd::wake_takes`). On by default; one switch turns it off.

- **What is a wake attempt.** A hit of the trained detector (ADR 0025) once it is decided:
  Ghost woke (*accepted*), or Parakeet's check turned it down (*refused*). Nothing else
  passive is kept: no audio between hits, no transcript of the room, nothing in
  transcript-wake mode (it has no detector score).
- **What is kept for each.**
  - The stream the detector heard, from 12 s before the frame that scored (an accepted wake:
    an attempt missed just before, when the operator had to say it again, is inside it) or
    4 s (a refused hit) to 1.5 s after.
  - The same moment from the array's own stream, when the detector hears the echo-cancelled
    `ghost.mic`: a second capture of the raw array (the capture target of
    `echo-cancel.conf`) runs only while the passive listener runs, kept in memory for the
    last 20 s, and never while listening is off (ADR 0057).
  - The detector's score every 80 ms over the take, the score, threshold and model, the
    input, the time, what Parakeet read for it, and the operator's label.
- **Where and how much.** `~/.local/share/ghost/wake/takes/`, a 0700 folder of 0600 files,
  capped at 2,000 takes and 1 GB, the oldest removed first.
- **What it costs.** Disk: about 1.3 MB for an accepted take with the array beside it,
  0.5 MB for a refused one; a day of 50 wakes and 50 refused hits is about 90 MB, and the
  cap holds about ten such days. CPU: the score curve is a number per frame the detector
  already computes; the array's capture is one more 16 kHz stereo stream into memory, with
  no model run on it; a take is written off the async runtime once per decided hit. Nothing
  is added to a turn.
- **Where it is said.** Because it is on without being asked, one plain line says so where a
  person sees it: the first run's wake step and Settings › Sound ("Ghost keeps the sound of
  each time it thinks it heard its name, on this computer, to learn your voice."), with Turn
  off and Delete, and the privacy section of the README.
- **Who reaches it.** The operator, through the control API: the list (metadata, no audio),
  the switch, labels, delete one, delete all (the folder goes), and playing one take through
  Ghost's own speaker, as the Hearing test plays its takes. No audio crosses the API. The
  phone listener, the listener for outside tools and Ghost's own app serve none of it, and no
  tool of any app, agent, run or model reaches the takes or the switch.
- **Nothing leaves the machine** by Ghost's doing.

**Labelling** is a tab of the Hearing test, *Wake takes*: one key per take ("me calling
Ghost", "not a wake", "me, after a miss"), and the next unlabelled take plays at once. An
accepted wake with no label counts as the operator only if they say so (a switch beside the
list); a refused hit with no label is never used, since nobody confirmed what it was.

**Retraining** is `make wake-train` (`tools/wake-train`), on this machine's CPU: a small
head beside the shipped classifier on the same frozen features (the openWakeWord mel and
embedding graphs), trained with numpy on the labelled takes, the recorded takes of
2026-10-30, Kokoro voices the replay rig does not use, and music, with augmentation (level,
noise, room, speed, compression). Whole recordings are held out for testing; the threshold
and the floor come from the new detector's own curve on the training negatives: the lowest
threshold at which it wakes no more often than the shipped one does at 0.6, and the lowest
floor at which Parakeet would be asked no more often than at 0.3 (issue #13: false wakes
are not traded away). It writes `hey_ghost.onnx` and a card with its hash and every number,
before and after, beside the shipped detector; nothing is installed.

**Swapping it in** is the operator's: `ghostd wake use <name>` sets `GHOST_WAKE_DETECTOR`
in `settings.env` (the card's threshold and floor come with it), `ghostd wake use shipped`
goes back, and `ghostd check` refuses a detector whose file does not match its card's hash
or whose card shows more false wakes than the shipped detector on any negative set.

## Consequences

- The idle-audio rule changes for wake attempts only: a decided detector hit's audio, its
  reading and its score curve are kept locally unless the operator turned keeping off.
  Passive audio the detector did not score as a hit is still never kept, logged or sent.
- The kept takes are the most sensitive thing Ghost stores: sounds that were not the
  operator calling Ghost (the refused hits) are among them. That is why they never reach a
  model or another device, and why delete all removes the folder.
- Training on a rented GPU with the operator's voice needs the operator's say-so each time
  (the owner allowed RunPod for this on 2026-11-12, kept out of git); the product path is
  the local one.
- When more than one person lives with Ghost, on by default must be revisited: their voices
  would be kept too.
