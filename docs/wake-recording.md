# Recording your wake takes

Ghost is moving from transcript wake to a trained "hey ghost" detector, with Parakeet confirming each hit ([ADR 0025](adr/0025-a-trained-wake-detector-with-parakeet-as-verifier.md), [issue #8](https://github.com/heka-vault/ghost/issues/8)). The detector was trained on synthetic voices. Only the operator's own voice, through their own mic and room, can show whether it works. About 70 takes, recorded once, are that test set. The ADR is accepted only if the detector plus verifier hears more of these takes than transcript wake does, and wakes no more often on the replay rig.

## Privacy

- You record every take deliberately, with a countdown. Nothing is taken from Ghost's passive audio, and the script offers to stop Ghost while you record.
- The takes stay on this machine under `.scratch/wake-takes/`, which git ignores. They are never uploaded or committed.
- The script plays nothing through the speakers.
- Scoring runs offline. It never writes a transcript to disk; `--show-text` prints them to the terminal only.

## Before you start

1. A quiet-ish room: no TV, and nobody else talking. Block E adds music on purpose.
2. The mic array plugged in.
3. `ghost-echo-cancel` running (`systemctl --user is-active ghost-echo-cancel` says `active`). Without it there is no `ghost.mic`, and the script stops.
4. Ghost stopped. If `ghostd` is running as a service, the script offers to stop it and starts it again when you finish, Ctrl+C included. If you started it with `make run` or `make start`, run `make stop` first.

## Recording

1. Look at the plan first: `make record-takes ARGS=--dry-run`. It lists every take and its file names, and records nothing.
2. Run `make record-takes`. Each block shows its instruction once; then each take shows the exact line to say.
3. Press Enter. The screen counts down 3-2-1, then shows "speak now". Say the line once. Bare takes are 3 s, takes with a request 5 s and near misses 4 s.
4. After each take the script prints the ghost.mic level: RMS and peak in dBFS. It warns when the peak is below −35 dBFS (too quiet, or the mic is muted) or at −0.5 dBFS or above (clipping).
5. Then press one key: Enter records the next take, `r` redoes the last one, `s` skips this one, `q` quits.
6. To stop, press `q` or Ctrl+C; everything recorded so far is kept. Run `make record-takes` again and it carries on from the next take.
   - The session is today's folder; to continue an older one, pass `ARGS="--session .scratch/wake-takes/2026-10-22"`.
   - Skipped takes are not asked again; `ARGS=--retry-skipped` brings them back.
7. To record only some blocks, run `make record-takes BLOCKS=A,C,F`. Take numbers stay the same whichever blocks you pick, so sessions recorded in parts don't overwrite each other.

| Block | What it asks | Takes | Why |
|---|---|---|---|
| A | At the desk, normal voice: "Hey Ghost." and "Hey Ghost, …" with a request | 10 + 5 | The everyday case, the baseline |
| B | At the desk, soft or tired voice | 5 | A quiet voice gives the detector less to go on |
| C | Across the room, 3 m or more, bare and with a request | 8 + 4 | Distance and room echo |
| D | At the desk, turned away from the mic | 6 | Speech that reaches the mic off the walls more than directly |
| E | Music on the desk speakers at your usual level (you start it), bare and with a request | 8 + 4 | What the echo canceller leaves behind |
| F | Near misses: "Hey coach.", "Hey Gus…", "He goes to the gym…", "Hey guys…" and others; 12 at the desk, 4 across the room | 16 | These must not wake Ghost; "he goes" is transcript wake's known confusion |
| G | Optional: "Okay Ghost." and "Ghost, are you there?" | 3 + 3 | Phrases transcript wake accepts and the detector does not; reported apart |

For C and the far part of F, the countdown is 8 s so you can press Enter and walk over. Count it in your head if you can't read the screen from there.

## Tips

- Speak the way you would to Ghost on an ordinary day. Don't over-enunciate; a careful "Hey. Ghost." is not what the detector will hear later.
- Vary it a little: faster, slower, a flat "hey ghost", a question.
- For requests, don't pause after "Ghost" any longer than you usually would.
- Redo a take with `r` whenever it went wrong: a cough, a stumble, or speaking before "speak now".

## Where the files go

`.scratch/wake-takes/<YYYY-MM-DD>/`:

- `NNN-<block>-<label>.aec.wav`: ghost.mic, mono, 16 kHz. This is what Ghost hears, and what is scored.
- `NNN-<block>-<label>.raw.wav`: the raw array under ghost.mic, 2 channels, 16 kHz, for comparing channels later. The array is read from `node.target` in `capture.props` of `~/.config/ghost/echo-cancel.conf`, or from `GHOST_RAW_MIC`. With neither, the script records ghost.mic alone.
- `takes.jsonl`: one line per take, with the file, block, condition, label (`wake`, `negative` or `other`), the line you were asked to say, its length, the RMS and peak levels and the time.
- `eval-<model>.md`: the score report, once you run the eval.

## Scoring them

Run `make wake-takes-eval`. It scores the newest session; `TAKES=.scratch/wake-takes/<date>` picks another, and `ARGS="--model path/to/model.onnx"` picks the detector model. It loads Parakeet and the detector and scores every `.aec.wav`, which takes a few minutes for a full session. Then it prints the report and writes it beside the takes as `eval-<model>.md`.

How to read the report:

- **Wake takes heard**: one row per block, and each cell is takes heard out of takes recorded.
  - *Transcript wake* is today's wake.
  - *Detector 0.1–0.7* is the trained model alone at each threshold.
  - *Two-stage 0.1 / 0.3* is the detector at that threshold with Parakeet confirming the hit. This is the design ADR 0025 proposes.
  - Higher is better.
- **False wakes on near misses**: the same columns over block F. Lower is better, and the two-stage columns should be 0.
- **Other phrases**: block G. Transcript wake should hear these; the detector and its verifier know only "hey ghost".
- **When the detector fires**: the detector's fire time minus the end of your speech, as a median.
  - For a bare "Hey Ghost." it is within a few tenths of a second of zero.
  - With a request it is well below zero: the detector fires after "ghost", while you are still talking.
  - Transcript wake only wakes after the whole utterance and its trailing silence.
- **Per take**: the detector's highest score on each take and when it first crossed each threshold. Use it to find the takes that missed.

The acceptance question for ADR 0025 is whether Two-stage beats Transcript wake on the wake rows with no more false wakes. False wakes on the replay rig (LibriSpeech, the room negatives, music) are measured separately.

## Trying the detector live

1. Copy the model to `~/.cache/ghost/wake/hey_ghost.onnx`, the default for `GHOST_WAKE_MODEL_PATH`, or set `GHOST_WAKE_MODEL_PATH` to its full path (the env file does not expand `~`).
2. Add these to `~/.config/ghost/settings.env`:

   ```sh
   GHOST_WAKE_MODE=onnx            # the trained detector: "hey ghost" only, no transcript fallback
   GHOST_WAKE_THRESHOLD=0.3        # the detector's first-stage score
   GHOST_WAKE_VERIFY=true          # Parakeet confirms each hit
   ```

   - `GHOST_WAKE_VERIFY=true`: Parakeet confirms each hit. Ghost chimes when you pause after "ghost", or 1 s after the hit.
   - `GHOST_WAKE_VERIFY=false`: the detector alone decides, and Ghost chimes the moment it fires.
   - Pick `GHOST_WAKE_THRESHOLD` from the report: the lowest threshold with no false wakes, read from the two-stage columns with verify on and the detector columns with it off.
3. Restart Ghost: `systemctl --user restart ghostd`.
4. To go back to transcript wake, set `GHOST_WAKE_MODE=transcript` and restart again.
