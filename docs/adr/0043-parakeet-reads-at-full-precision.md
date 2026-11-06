# 0043: Parakeet reads at full precision

- **Status:** accepted
- **Date:** 2026-11-06
- **Amends:** ADR 0004 (Parakeet TDT 0.6B v3 stays the model; its int8 quantization is no
  longer what Ghost runs)

The int8 Parakeet returns nothing for a clip that ends in more than about half a second of
quiet. The hear harness found it on the operator's first recorded session
([#60](https://github.com/heka-vault/ghost/issues/60),
[#58](https://github.com/heka-vault/ghost/issues/58)): a clean desk take, 3.9 s with the
words at 1.7 to 3.1 s, read whole as 0 of 6 words. Cut the quiet after the words to 0.25 s
and it reads 5 of 6; leave 0.6 s and it reads nothing; lay −70 dBFS of noise under it and it
still reads nothing; append 0.2 s of exact zeros and every variant reads nothing. The fp32
files read every variant (the same clips through `onnx_asr` and through Ghost's port). The
live stage's segment ends in 0.4 s of quiet (VAD stop 0.2 s plus 0.2 s of trailing silence),
one notch under the cliff: at a 0.4 s VAD stop, 2 of the session's 49 spoken takes read
empty; at 1.0 s, 3; fp32 reads none of them empty. The −72 dBFS dither Ghost lays under
every read (2026-10-30) only covered the exact-zeros case. The STT review of 2026-10-30
had seen the int8 encoder go silent on 120 s windows and judged that the pipeline never
hands it one so long; it does, in effect, whenever a turn's close waits longer.

## Decision

Ghost reads speech with the full-precision Parakeet. `GHOST_STT_QUANTIZATION` unset or
`none` means fp32; `int8` still selects the quantized files, which the parity fixtures were
recorded with and `crates/ghost-infer/tests/parity.rs` still checks.

The fp32 files are the five of `istupakov/parakeet-tdt-0.6b-v3-onnx` at a pinned revision,
2.5 GB. `scripts/models.sh parakeet` fetches them, pinned by SHA-256, into
`~/.cache/ghost/models/parakeet-tdt-0.6b-v3/` as plain files, copying from the oracle's
Hugging Face snapshot when it already holds them (ONNX Runtime 1.24 refuses that
snapshot's symlinked external data, so the daemon never reads fp32 from there). `make dev`
and `make models` run it; the bundle does not carry the files, as it never carried
Parakeet (ADR 0038, docs/releasing.md). `GHOST_STT_DIR` still names any folder.

## Measured

On the operator's session of 2026-11-06 (62 takes, `make perf-hear`):

| | int8 (before) | fp32 |
|---|---|---|
| Word error rate, today's live read | 9.8% | 8.4% |
| Word error rate, the whole turn read once at the close | 8.4% | 4.2% |
| Empty reads, live / turn / whole file | 0 / 2 / 3 of 49 | 1 / 0 / 1 of 49 |
| Read at the close, p95 (live / turn) | 0.18 s / 0.26 s | 0.25 s / 0.32 s |

The model alone (`make perf-micro --only stt`, with the operator's daemon running
alongside): a one-word reply 0.21 s → 0.34 s, nine words 0.30 s → 0.46 s, twenty-two words
(6.7 s of audio) 0.43 s → 1.06 s. Resident memory with the model loaded 1.3 GB → 3.0 GB;
loading it about ten seconds. Numbers read as digits got worse on the session (4 → 7 of
16 wrong) and are still to be looked at.

A whole turn (`make perf RUNS=2 ARGS=--no-jev`, the same afternoon, both under the same
load): the `stt` stage p50 0.11 s → 0.17 s and p95 0.17 s → 0.25 s; the turn's `hear` p50
1.45 s both ways, since the 1.2 s hold after the words dominates it, and the reply p50
2.08 s → 2.05 s, p95 2.67 s → 2.59 s, the difference being run-to-run noise.

## Consequences

- Every read costs 1.6 to 2.5 times what it did, within the +0.15 s budget at the close
  on this session's takes but not on long turns; `make perf` keeps that visible. The
  interim reads during speech (barge-in) are capped at 4 s of audio and keep their cap.
- A machine that runs Ghost needs 2.5 GB more disk and about 1.7 GB more memory.
- The whole-turn read at the close (#52 item 1) is now worth building: with int8 it
  returned nothing for 2 of 49 turns.
- The parity fixtures stay int8 and are not evidence about fp32's exact output; a parity
  run at fp32 would need fixtures recorded with it.
- Tests that start a daemon point `GHOST_STT_DIR` at an empty folder, so they don't each
  load 2.4 GB.

## Alternatives considered

- **Trim the quiet tail before the int8 read.** Recovers 5 of 6 words on the failing take,
  not 6, and the stage would still lose what the longer stops and the whole-turn read
  need; a workaround for a model that is wrong about silence.
- **More dither, or noise under the clip.** −70 dBFS of noise didn't change the empty
  read; the failure isn't digital silence.
- **Another export or quantization of the same model.** Untested; the fp32 files are the
  reference the int8 ones were made from, and they read the clips.
