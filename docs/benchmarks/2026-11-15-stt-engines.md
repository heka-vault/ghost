# Speech-to-text engines: transcribe.cpp against Ghost's Parakeet (2026-11-15)

Numbers and method only. The question was whether to take three things from the dictation app Handy (MIT,
commit `aca8985`): a streaming model, inference in a child process, and transcribe.cpp's
GGUF runtime with its Vulkan backend. The bench is `docs/benchmarks/scripts/stt-engines/`: its own Cargo
workspace, not a member of the repo's and not built by CI. It scores with Ghost's own scorer
(`ghost_voice::perf::hear::score`) by path dependency.

Everything was measured on the operator's recorded Hearing session of 2026-11-06. The tables
are the second pass of 2026-11-15 evening: every read is scored again with the scorer fix
(`032607a6`, a spoken zero written as "oh" or "o" between digits; on this branch as
`90185ae3`). The streams' first-word measure now counts finalize, and the three batch
candidates are also compared on Ghost's own turn span. Accuracy is deterministic for a given
build and model: three passes of the three batch candidates gave identical scores, take for
take. The three batch candidates' timings are three passes in rotated order. **Every other
timing is a single pass** on a shared workstation (other agents were measuring; every build
and timed run held `flock /tmp/ghost-perf.lock`). The isolation figures are from the first
pass, which nothing here changes.

## Machine

Intel Core i5-11600K (6 cores, 12 threads, AVX-512), 31 GB RAM. The GPU is the CPU's
integrated Intel UHD 750 ("RKL GT1"), driven by Mesa 26.0.8's Vulkan driver. Ubuntu 26.04.
ONNX Runtime 1.24.4 through `ort` (`ghost_infer::Parakeet`). transcribe-cpp 0.4.2 is built
statically with its default Linux flags plus the `vulkan` feature.

The Vulkan build needs the Vulkan headers, the loader's link, `glslc` and SPIRV-Headers.
None of these are installed: they come from Ubuntu's `libvulkan-dev`, `glslc`, `libshaderc1`
and `spirv-headers` packages. The packages were fetched with `apt-get download` and unpacked
with `dpkg -x` into `~/.cache/ghost-stt-bench/vulkan-sdk/`, with no root
(`docs/benchmarks/scripts/stt-engines/env.sh`).

## Method

**Takes.** The recorded session's 62 takes on the channel Ghost hears (`*.aec.wav`): 49
spoken (2.3 to 8.3 s, median 5.2 s; 285 reference words) and 13 silent. The reference
words are `docs/benchmarks/hear-plan.json`, paired with the takes by
`ghost_voice::perf::hear::plan::recorded`.

**Batch reads.** Each take is read two ways where the run says so:

- `file`: the whole take, with no VAD.
- `turn`: the span the harness's `turn` read uses, from the first VAD segment's pre-roll to
  the last segment's end. The spans come from Ghost's own speech-in stage (`ParakeetStt`
  with Silero and the session's VAD settings: confidence 0.7, start 0.2 s, stop 0.2 s, early
  read off). The take is fed on the audio's own clock with 2 s of −70 dBFS room after it
  (`src/span.rs`, a copy of the harness's private `hear_stage` keeping only the VAD frames).
  The spans were computed once and saved as sample ranges (`stt-engines spans`), and every
  candidate reads the same spans. 50 of 62 takes have a turn.

Both reads get exactly the input the harness gives them: the audio, then `TRAILING_SECS`
(0.2 s) of zeros, with Ghost's −72 dBFS dither under it. One warm-up read comes before the
timed pass. The bench's own ort fp32 reads reproduce the harness's columns exactly, take for
take (the paired interval is 0 to 0): `file` 4.6% (13 errors over 285 words) and `turn` 2.1%
(6 errors). The `turn` read was given to the three batch candidates only (ort fp32, TDT v3
Q8_0 and Parakeet Unified English Q8_0 on the CPU).

**Scoring.** `score::score` per take, exactly as the harness scores, with the scorer fix.
Each comparison is
paired take by take with the harness's bootstrap (`score::paired_delta`, 4,000 resamples,
seed 60). "Within noise" means the 95% interval includes zero.

**Baselines.** `make perf-hear TAKES=latest` at `90185ae3` (the scorer fix on top of
`2ffc42c4`). The first pass's figures, before the fix, are in brackets:

| Ghost's read (ort, 4 threads) | stage | turn | file |
|---|---|---|---|
| fp32 | 8.4% (8.4%) | 2.1% (4.2%) | 4.6% (6.7%) |
| int8 (`GHOST_STT_QUANTIZATION=int8`) | 9.8% (9.8%) | 6.3% (8.4%) | 9.1% (10.2%) |

**Timing passes.** The three batch candidates were run three times in rotated order: ort,
TDT, Unified; then TDT, Unified, ort; then Unified, ort, TDT. Each pass reads both `file` and
`turn`. A pass's figure is its p50 of model seconds per second of audio over the 49 spoken
takes (over the turn spans' length for `turn`). The table gives the three passes, their
median and their spread (lowest to highest).

**Threads.** 4 everywhere: ort's intra-op threads (Ghost's `GHOST_STT_THREADS`) and
transcribe.cpp's `SessionOptions::n_threads`. For one configuration
(`tdtv3-Q8_0-cpu-default`) the library chooses its own count, which is what Handy uses.

**Load and memory.** Each model file is read once beforehand so the timed load finds it in
the page cache. RSS is `VmRSS` after the load and after all reads; peak is `VmHWM`.
transcribe.cpp memory-maps the weights, so its load is lazy and the first read pays for it.
"Load + first read" is the honest load figure. On Vulkan, RSS leaves out memory the driver
allocates for the iGPU, so the Vulkan rows' memory is not comparable.

**Streaming.** Each take, with the same input as the batch read, is fed in 100 ms frames
(`Session::stream`, `StreamOptions` with `CommitPolicy::Auto` as Handy runs it), then
finalized. The clock is simulated real time. Frame *k* becomes available when its audio
would have arrived, and its compute starts once both that has happened and the previous
frame's compute has finished. Each update's text (`committed + tentative`) is normalized
with the scorer's rules.

- *First word / two words shown after said.* The simulated time at which the shown text
  first holds one (two) words, minus the time the first (second) spoken word ended. Word end
  times come from Parakeet TDT v3's own word timestamps (transcribe.cpp, F32, CPU, batch
  read, counted with the fixed scorer's rules) and are the same clock for every candidate.
  A take whose words appear only when the stream is finalized arrives at finalize: the end
  of the audio, plus the backlog, plus `finalize`. The tables say how many takes that is.
  Takes with fewer than one (two) timestamped words, or whose read has fewer words, have no
  figure (n in the table).
- *Against Ghost's interim.* The p50 on the takes both the stream and Ghost's interim
  simulation have a figure for, and the number of those takes where the stream was later.
- *First two right.* The first two words shown were the reference's first two.
- *Work left at close.* The backlog when the last frame arrives, plus the time `finalize`
  takes: what a turn's close would wait for.
- *Rewrites.* Updates that change or remove an already shown word other than the last one.
  A last word growing token by token is not counted. Separately, the number of takes where
  `finalize` changed the last word shown.
- Stream settings: by default each model's own default, which is its max-accuracy,
  max-latency entry and what Handy runs. The latency menus are read from each GGUF
  (`scripts/gguf_menu.py`). Nemotron Streaming English accepts lookahead in encoder frames
  ∈ {13, 6, 1, 0} (1,040, 480, 80, 0 ms). Parakeet Unified English accepts left 5,600 ms,
  chunk ∈ {80, 160, 560, 1,040} ms and right ∈ {0, 80, 160, 240, 320, 560, 1,040} ms.

**Ghost's barge-in path today, for comparison.** Simulated on ort fp32 with Ghost's
`InterimSchedule`. The VAD's start is placed at the manifest's speech start plus 0.2 s of VAD
start and one 32 ms step. Every 0.3 s after that, the segment so far (with its 1 s pre-roll,
at most the last 4 s, plus 0.2 s of zeros, dithered) is read again, and a tick is skipped
while a read is in flight. The simulation stops once two words are shown. It does not
include the condition that Ghost is speaking, which gates the interim live.

**Isolation.** The child is the bench's own binary (`stt-engines worker`), shaped like
Handy's `worker.rs`: the protocol runs on a duplicate of stdout, a thread reads stdin into a
channel, and the main loop answers. Frames use Handy's format,
`[u32 json_len][json][u32 pcm_len][f32 LE PCM]` (`pipe.rs`, after Handy's `protocol.rs`).
The round trip sends 2, 5 and 15 s of 16 kHz f32 PCM with a `Run` request, and the child
answers with a sentence-long `Transcript` with no model: 20 warm-up calls, then 300 timed.
Spawn plus load: a fresh worker is spawned, told to load a model, then given two reads of a
5.2 s take, three times per model.

**Privacy.** The takes and every transcript stay in memory. The output files hold numbers
only (`~/.cache/ghost-stt-bench/runs-v2/`; the first pass in `runs/`), and nothing was printed or written as text. Model
downloads came from Handy's catalog at its pinned revisions, with each file's SHA-256 checked
(`scripts/models.txt`, `scripts/fetch.sh`).

## Results

### Batch: the whole take read once

The baseline is Ghost's fp32 `file` read (4.6%). Model s/s is model seconds per second of
audio over the 49 spoken takes. For ort fp32, TDT v3 Q8_0 and Unified English Q8_0 on the
CPU these are the first timing pass; see *Timing* for all three.

| Run | WER | Δ vs fp32 `file` (95% CI) | Numbers wrong | Term errors | Terms heard | Empty reads | Silent takes with words | Model s/s p50 / p95 | Load + first read | RSS after reads | Peak |
|---|---|---|---|---|---|---|---|---|---|---|---|
| ort fp32 (Ghost) | 4.6% | — | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.043 / 0.049 | 2.17 s | 2,568 MB | 2,568 MB |
| ort int8 | 9.1% | +4.6 pt (−0.8 to +11.3), within noise | 4/16 | 3/10 | 6/9 | 3/49 | 0/13 | 0.031 / 0.033 | 1.62 s | 1,144 MB | 1,144 MB |
| TDT v3 F32, tc CPU | 4.2% | −0.4 pt (−1.1 to 0.0), within noise | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.048 / 0.051 | 4.01 s (0.26 s in the first pass) | 2,567 MB | 2,600 MB |
| TDT v3 F16, tc CPU | 4.2% | same | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.053 / 0.055 | 0.47 s | 1,654 MB | 1,686 MB |
| TDT v3 Q8_0, tc CPU | 4.2% | same | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.050 / 0.053 | 0.42 s | 1,170 MB | 1,201 MB |
| TDT v3 Q8_0, tc CPU, library's threads | 4.2% | same | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.043 / 0.046 | 0.40 s | 1,169 MB | 1,201 MB |
| TDT v3 F32, tc Vulkan | 4.2% | same | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.109 / 0.129 | 1.67 s | (GPU) | (GPU) |
| TDT v3 F16, tc Vulkan | 4.2% | same | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.143 / 0.179 | 0.91 s | (GPU) | (GPU) |
| TDT v3 Q8_0, tc Vulkan | 4.2% | same | 4/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.071 / 0.083 | 0.50 s | (GPU) | (GPU) |
| Parakeet Unified English Q8_0, tc CPU | 3.9% | −0.7 pt (−4.9 to +3.0), within noise | 2/16 | 3/10 | 6/9 | 0/49 | 0/13 | 0.048 / 0.051 | 0.40 s | 1,114 MB | 1,145 MB |
| Parakeet Unified English F16, tc CPU | 3.9% | same | 2/16 | 3/10 | 6/9 | 0/49 | 0/13 | 0.052 / 0.053 | 0.44 s | 1,597 MB | 1,628 MB |
| Parakeet Unified English Q8_0, tc Vulkan | 3.9% | same | 2/16 | 3/10 | 6/9 | 0/49 | 0/13 | 0.073 / 0.084 | 0.57 s | (GPU) | (GPU) |
| Nemotron Streaming English Q8_0, tc CPU | 11.2% | +6.7 pt (+1.7 to +11.8), worse | 3/16 | 4/10 | 6/9 | 3/49 | 0/13 | 0.048 / 0.052 | 0.40 s | 1,111 MB | 1,144 MB |
| Nemotron Streaming English F16, tc CPU | 10.9% | +6.3 pt (+1.4 to +11.5), worse | 3/16 | 4/10 | 6/9 | 3/49 | 0/13 | 0.053 / 0.054 | 0.43 s | 1,595 MB | 1,628 MB |
| Nemotron 3.5 Streaming Q8_0, tc CPU | 9.5% | +4.9 pt (+1.3 to +8.9), worse | 3/16 | 4/10 | 6/9 | 0/49 | 0/13 | 0.059 / 0.064 | 0.49 s | 1,206 MB | 1,241 MB |
| Canary 180M Flash Q8_0, tc CPU | 13.7% | +9.1 pt (0.0 to +20.5), within noise | 3/16 | 5/10 | 5/9 | 3/49 | 0/13 | 0.024 / 0.028 | 0.15 s | 345 MB | 453 MB |
| Canary 180M Flash Q8_0, tc Vulkan | 13.0% | +8.4 pt (−0.7 to +19.8), within noise | 3/16 | 5/10 | 5/9 | 3/49 | 0/13 | 0.054 / 0.063 | 0.55 s | (GPU) | (GPU) |
| Moonshine Streaming Medium Q8_0, tc CPU | 10.5% | +6.0 pt (0.0 to +13.1), within noise | 2/16 | 2/10 | 7/9 | 3/49 | 1/13 | 0.067 / 0.085 | 0.32 s | 659 MB | 670 MB |

`tc` is transcribe.cpp 0.4.2. "Same" means the same reads as TDT v3 F32 on transcribe.cpp,
take for take. No configuration made up a term. TDT v3 F32's 4.01 s to load and read first in
this pass is a single reading under memory pressure (swap was full). The first pass gave
0.26 s for the same thing. ort fp32's load without the first read is 1.96 s, and its RSS
right after load is 2,224 MB. transcribe.cpp's RSS right after load is 175 to 595 MB,
because it maps the weights and touches them on the first read.

### Batch: Ghost's turn span, like for like

The same span Ghost's `turn` read gets, read by the three candidates (first timing pass). The
baseline is Ghost's fp32 `turn` read (2.1%, 6 errors over 285 words).

| Run | WER (errors) | Δ vs fp32 `turn` (95% CI), verdict | Numbers wrong | Term errors | Terms heard | Empty reads | Silent takes with words | Model s/s p50 / p95 |
|---|---|---|---|---|---|---|---|---|
| ort fp32 (Ghost) | 2.1% (6) | — | 2/16 | 1/10 | 8/9 | 0/49 | 0/13 | 0.046 / 0.064 |
| TDT v3 Q8_0, tc CPU | 1.8% (5) | −0.4 pt (−1.1 to 0.0), within noise | 2/16 | 1/10 | 8/9 | 0/49 | 0/13 | 0.052 / 0.059 |
| Parakeet Unified English Q8_0, tc CPU | 4.2% (12) | +2.1 pt (−0.7 to +5.3), within noise | 2/16 | 3/10 | 6/9 | 0/49 | 0/13 | 0.049 / 0.054 |

On the whole take the same three read 4.6% (13 errors), 4.2% (12) and 3.9% (11).

### Timing: three passes, rotated

Model s/s p50 per pass, in pass order (each pass ran the three in a different order), then
the median of the three passes and their spread.

| Run | Read | Pass 1 / 2 / 3 | Median | Spread |
|---|---|---|---|---|
| ort fp32 | `file` | 0.0434 / 0.0447 / 0.0442 | 0.0442 | 0.0434–0.0447 |
| TDT v3 Q8_0, tc CPU | `file` | 0.0499 / 0.0491 / 0.0500 | 0.0499 | 0.0491–0.0500 |
| Parakeet Unified English Q8_0, tc CPU | `file` | 0.0480 / 0.0484 / 0.0504 | 0.0484 | 0.0480–0.0504 |
| ort fp32 | `turn` | 0.0461 / 0.0468 / 0.0477 | 0.0468 | 0.0461–0.0477 |
| TDT v3 Q8_0, tc CPU | `turn` | 0.0515 / 0.0517 / 0.0528 | 0.0517 | 0.0515–0.0528 |
| Parakeet Unified English Q8_0, tc CPU | `turn` | 0.0494 / 0.0501 / 0.0522 | 0.0501 | 0.0494–0.0522 |

On both reads ort's spread lies wholly below both transcribe.cpp models' spreads. TDT v3 Q8_0
and Unified English overlap.

### Streaming: fed in 100 ms frames

The WER is of the finalized stream, against Ghost's fp32 `turn` (2.1%) and `stage` (8.4%)
reads. Arrival is p50 / p95 in seconds after the word was said, over all takes with a figure
(n, of which k came only at finalize). "vs interim" is the stream's p50 against Ghost's
interim simulation's p50 for two words, on the takes both have, and how many of those takes
the stream was later on.

| Run | Setting | WER (final) | Δ vs `turn` 2.1% | Δ vs `stage` 8.4% | First word (n, at finalize) | Two words (n, at finalize) | Two words vs interim, p50 (takes later / common) | First two right | Compute s/s p50 / p95 | Slowest frame p95 | Work left at close p50 / p95 | Rewrites before the last word | Finalize changed the last word |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Parakeet Unified English Q8_0 | default (C 1,040, R 1,040 ms) | 6.0% | +3.9 (+0.4 to +7.7), worse | −2.5 (−7.5 to +2.6), within noise | 1.83 / 2.72 (48, 11) | 1.60 / 2.51 (41, 7) | 1.60 vs 0.14 (41 / 41) | 26/34 | 0.177 / 0.229 | 336 ms | 0.30 / 0.55 s | 0 of 95 updates | 6 takes |
| Parakeet Unified English Q8_0 | C 560, R 560 ms | 7.4% | +5.3 (+2.0 to +9.0), worse | −1.1 (−6.4 to +4.2), within noise | 1.25 / 1.65 (48, 4) | 1.08 / 1.49 (41, 2) | 1.08 vs 0.14 (41 / 41) | 30/39 | 0.267 / 0.353 | 307 ms | 0.28 / 0.52 s | 0 of 173 | 4 |
| Parakeet Unified English Q8_0 | C 160, R 160 ms | 7.7% | +5.6 (+2.3 to +9.3), worse | −0.7 (−4.9 to +3.4), within noise | 0.76 / 1.81 (48, 0) | 0.62 / 1.63 (41, 0) | 0.62 vs 0.14 (39 / 41) | 28/41 | 0.953 / 2.417 | 886 ms | 1.22 / 8.04 s | 1 of 347 | 0 |
| Parakeet Unified English Q8_0 | C 80, R 0 ms | 30.2% | +28.1 (+18.8 to +38.0), worse | +21.8 (+11.9 to +32.5), worse | 0.61 / 1.12 (43, 0) | 0.48 / 1.53 (36, 0) | 0.48 vs 0.14 (34 / 36) | 21/36 | 1.675 / 2.809 | 1,068 ms | 4.44 / 11.61 s | 0 of 295 | 0 |
| Parakeet Unified English F16 | default | 6.0% | +3.9 (+0.4 to +7.7), worse | −2.5, within noise | 1.85 / 2.74 (48, 11) | 1.61 / 2.51 (41, 7) | 1.61 vs 0.14 (41 / 41) | 26/34 | 0.190 / 0.245 | 363 ms | 0.33 / 0.61 s | 0 of 95 | 6 |
| Parakeet Unified English Q8_0, Vulkan | default | 6.0% | +3.9 (+0.4 to +7.7), worse | −2.5, within noise | 1.90 / 2.76 (48, 11) | 1.67 / 2.57 (41, 7) | 1.67 vs 0.14 (41 / 41) | 26/34 | 0.261 / 0.314 | 432 ms | 0.43 / 0.72 s | 0 of 95 | 6 |
| Nemotron Streaming English Q8_0 | default (lookahead 1,040 ms) | 10.9% | +8.8 (+4.7 to +13.7), worse | +2.5 (−2.6 to +7.7), within noise | 0.84 / 1.69 (45, 5) | 0.52 / 1.29 (39, 1) | 0.52 vs 0.14 (37 / 39) | 29/38 | 0.076 / 0.091 | 116 ms | 0.07 / 0.11 s | 0 of 112 | 4 |
| Nemotron Streaming English Q8_0 | lookahead 480 ms | 10.2% | +8.1 (+4.3 to +12.6), worse | +1.8 (−3.0 to +6.9), within noise | 0.81 / 1.41 (46, 4) | 0.57 / 0.93 (40, 1) | 0.57 vs 0.15 (38 / 40) | 29/39 | 0.096 / 0.109 | 75 ms | 0.06 / 0.09 s | 0 of 174 | 1 |
| Nemotron Streaming English Q8_0 | lookahead 80 ms | 11.9% | +9.8 (+5.5 to +15.1), worse | +3.5 (−1.1 to +8.8), within noise | 0.63 / 1.05 (43, 1) | 0.43 / 0.85 (39, 1) | 0.43 vs 0.15 (32 / 39) | 24/38 | 0.205 / 0.232 | 60 ms | 0.05 / 0.09 s | 0 of 324 | 0 |
| Nemotron Streaming English F16 | default | 10.9% | +8.8 (+4.5 to +13.8), worse | +2.5 (−2.9 to +8.1), within noise | 0.84 / 1.69 (45, 5) | 0.59 / 1.46 (39, 1) | 0.59 vs 0.14 (37 / 39) | 31/38 | 0.081 / 0.095 | 103 ms | 0.08 / 0.13 s | 0 of 112 | 3 |
| Nemotron Streaming English Q8_0, Vulkan | default | 11.2% | +9.1 (+4.9 to +14.2), worse | +2.8 (−2.3 to +8.3), within noise | 1.06 / 1.84 (45, 5) | 0.70 / 1.60 (39, 1) | 0.70 vs 0.14 (38 / 39) | 29/38 | 0.224 / 0.253 | 264 ms | 0.24 / 0.31 s | 0 of 112 | 4 |
| Nemotron 3.5 Streaming Q8_0 | default (1,040 ms) | 9.5% | +7.4 (+3.5 to +12.0), worse | +1.1 (−4.3 to +6.8), within noise | 0.93 / 1.79 (48, 9) | 0.68 / 1.48 (41, 3) | 0.68 vs 0.14 (39 / 41) | 26/38 | 0.090 / 0.110 | 136 ms | 0.08 / 0.13 s | 1 of 114 | 3 |
| Nemotron 3.5 Streaming Q8_0 | lookahead 480 ms | 10.5% | +8.4 (+4.7 to +12.8), worse | +2.1 (−2.9 to +7.4), within noise | 0.81 / 1.42 (47, 8) | 0.54 / 0.94 (40, 2) | 0.54 vs 0.15 (38 / 40) | 24/38 | 0.103 / 0.112 | 75 ms | 0.06 / 0.10 s | 2 of 180 | 1 |
| Moonshine Streaming Medium Q8_0 | default | 10.2% | +8.1 (+2.7 to +14.9), worse | +1.8 (−4.0 to +8.8), within noise | 0.37 / 1.50 (47, 0) | 0.18 / 0.99 (41, 0) | 0.18 vs 0.14 (27 / 41) | 16/44 | 1.480 / 1.735 | 444 ms | 3.36 / 6.56 s | 90 of 332 (33 takes) | 0 |
| **Ghost today: fp32 interim re-read every 0.3 s (ort)** | — | (finals are the batch rows) | | | 0.46 / 0.86 (48, —) | 0.14 / 0.50 (41, —) | — | 33/41 | one read ≈ 0.11 s | — | — | — | — |

*First two right* counts only takes whose first two words appeared before finalize. No
configuration's `finalize` changed a shown word before the last one; the last column counts
the takes where it changed the last one. For Ghost's interim, the median take needed one
read: the first tick, 0.3 s after the VAD started, reading about 1.5 s of audio, already
showed two words.

### Isolation

**The pipe alone.** These are round trips in Handy's frame format to a child with no model,
300 calls each after 20 warm-up calls.

| PCM | Bytes | Round trip p50 / p95 / max | Encode + decode in one process p50 |
|---|---|---|---|
| 2 s | 128,000 | 0.10 / 0.12 / 0.21 ms | 0.08 ms |
| 5 s | 320,000 | 0.16 / 0.21 / 0.37 ms | 0.18 ms |
| 15 s | 960,000 | 0.49 / 0.60 / 0.98 ms | 0.57 ms |

**A worker's spawn and load.** These are three fresh workers per model. The first repetition
of each started with the model file out of the page cache, because other work had evicted
it. The load time includes reading the file through once.

| Model in the worker | Spawn to loaded (rep 1 / 2 / 3) | First read of a 5.2 s take | Second read |
|---|---|---|---|
| ort fp32 (2.5 GB) | 11.16 / 2.12 / 2.10 s | 0.71 / 0.22 / 0.22 s | 0.23 / 0.22 / 0.22 s |
| ort int8 | 4.42 / 2.91 / 1.50 s | 0.19 / 0.16 / 0.16 s | 0.16 / 0.16 / 0.16 s |
| TDT v3 F32, tc CPU (2.5 GB) | 7.28 / 0.24 / 0.25 s | 1.81 / 0.28 / 0.27 s | 0.25 / 0.25 / 0.25 s |

## What was not measured

- **Vulkan on a discrete GPU.** The only GPU here is the integrated UHD 750.
- **Vulkan's memory and the CPU it frees.** RSS does not see the driver's allocations, and
  the CPU busy time during Vulkan reads was not recorded.
- **Handy's shipped CPU build.** Its `dynamic-backends` build picks a CPU variant at run time
  (`GGML_CPU_ALL_VARIANTS`). This bench is a static build with the library's default Linux
  flags.
- **Streaming in the live pipeline.** Ghost's echo filter and barge-in rule, Ghost's own
  voice over the operator, and a stream left running across a turn's pauses are not
  measured. The streams here start at the take's first sample and end at its last.
- **Arrival against Ghost's VAD-stop path.** Ghost's arrival is measured only for its
  interim path, which runs live only while Ghost is speaking. Otherwise a segment's words
  arrive at its VAD stop.
- **Repeated timing runs beyond the three batch candidates.** Every other timing is a single
  pass on a shared machine.
- **The turn span for the other models.** Only the three batch candidates read Ghost's
  turn span.
- **The large models.** Voxtral 4B and Canary-Qwen 2.5B were skipped as asked, as were the
  other ~60 catalog entries.

## Second review (2026-11-15), and what it changes

Each point is marked **answered**, by the second pass's tables above, or **open**.

A second reviewer read the bench's code and saved numbers. The batch comparison stands
(same audio, scorer, threads, warm-up left out). These readings do not:

- **Answered: the scores predated a scorer fix** made the same day (a spoken zero the fp32
  model writes as the letter "o" counted as three errors). Everything is now scored again
  with it, and the tables above are the rescored ones. The reviewer expected Unified
  English's 3.9% to be about level with Ghost's file read and behind its turn read, and so it
  is. On the whole take it reads 3.9% against 4.6% (−0.7 pt, −4.9 to +3.0, within noise). On
  the turn span it reads 4.2% against 2.1% (+2.1 pt, −0.7 to +5.3, within noise).
- **Open: "within noise" means not decided.** On 49 takes the intervals admit several points
  either way. From the paired variance here, telling models 2 points apart takes about 425
  comparable takes, 1 point about 1,700.
- **Answered: first-word delay left out takes whose first words came only at finalize**
  (12 of 49 for Unified English at its defaults), so the stream's delays read better than
  they were. They now arrive at finalize and are counted: 11 of 48 for Unified English at its defaults,
  against the fixed scorer's clock. Each stream is also compared with the interim on the
  takes both have.
- **Open: the interim simulation is a proxy**, not Ghost's schedule: one VAD start, no
  restarts, no contention, the scorer's words and not the barge-in rule's. It shows no demonstrated
  gain for barge-in; it does not rule one out.
- **Open: the close.** The stream's work at the close was measured to the end of the
  recording plus padding, not to the VAD's stop, and Ghost's whole-turn read already runs inside the
  turn's grace (ADR 0044). The 0.25 s a stream might save is not an end-to-end figure. A
  hybrid (a stream for interims and the provisional close, the batch model for the
  transcript) was not measured.
- **Open (no new measurement): isolation.** A worker's start is 1.3 to 5.7 ms; the
  seconds are the model's load, paid once for a worker kept for the model's life. The numbers give no performance reason
  against it; deferring it is a call about complexity and the absence of a crash.
- **Open: Vulkan.** Pipelines compile lazily per shape, so one warm-up take does not prove
  the timed reads were free of compilation. The slowdown is large enough to decline this UHD 750
  for now, not GPUs in general.
- **Answered:** "no finalize changed words on screen" holds only before the last word
  shown: Unified English at its defaults rewrote at finalize in six takes. The streaming
  table now counts it per configuration (6 for Unified English at its defaults).
- **Open:** Parakeet Unified English can be exported to ONNX (an RNN-T, not TDT, decoder),
  so it could be measured on Ghost's own runtime; that needs a decoder and parity work.
- **Answered (asked with this pass): like-for-like spans.** The three batch candidates read
  Ghost's turn span (*Batch: Ghost's turn span*), and their timings are three rotated passes
  (*Timing*).
