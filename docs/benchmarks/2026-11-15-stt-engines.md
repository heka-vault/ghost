# Speech-to-text engines: transcribe.cpp against Ghost's Parakeet (2026-11-15)

Numbers and method only. The question was whether to take three things from the dictation app Handy (MIT,
commit `aca8985`): a streaming model, inference in a child process, and transcribe.cpp's
GGUF runtime with its Vulkan backend. The bench is `tools/stt-engines/`: its own Cargo
workspace, not a member of the repo's and not built by CI. It scores with Ghost's own scorer
(`ghost_voice::perf::hear::score`) by path dependency.

**Every number below comes from one run** of each configuration on the operator's recorded
Hearing session of 2026-11-06. Accuracy is deterministic for a given build and model. The
timings are a single pass on a shared workstation (other agents were measuring; every build
and timed run held `flock /tmp/ghost-perf.lock`). The spawn and load figures are three
repetitions each.

## Machine

Intel Core i5-11600K (6 cores, 12 threads, AVX-512), 31 GB RAM. The GPU is the CPU's
integrated Intel UHD 750 ("RKL GT1"), driven by Mesa 26.0.8's Vulkan driver. Ubuntu 26.04.
ONNX Runtime 1.24.4 through `ort` (`ghost_infer::Parakeet`). transcribe-cpp 0.4.2 is built
statically with its default Linux flags plus the `vulkan` feature.

The Vulkan build needs the Vulkan headers, the loader's link, `glslc` and SPIRV-Headers.
None of these are installed: they come from Ubuntu's `libvulkan-dev`, `glslc`, `libshaderc1`
and `spirv-headers` packages. The packages were fetched with `apt-get download` and unpacked
with `dpkg -x` into `~/.cache/ghost-stt-bench/vulkan-sdk/`, with no root
(`tools/stt-engines/env.sh`).

## Method

**Takes.** The recorded session's 62 takes on the channel Ghost hears (`*.aec.wav`): 49
spoken (2.3 to 8.3 s, median 5.2 s; 285 reference words) and 13 silent. The reference
words are `docs/benchmarks/hear-plan.json`, paired with the takes by
`ghost_voice::perf::hear::plan::recorded`.

**Batch reads.** Each take is read whole, with no VAD. Every engine gets exactly the input
the hear harness's `file` read gets: the take, then `TRAILING_SECS` (0.2 s) of zeros, with
Ghost's −72 dBFS dither under it. One warm-up read comes before the timed pass. The bench's
own ort fp32 pass reproduces the harness's `file` column exactly: 6.7%, 19 errors over 285
words.

**Scoring.** `score::score` per take, exactly as the harness scores. Each comparison is
paired take by take with the harness's bootstrap (`score::paired_delta`, 4,000 resamples,
seed 60). "Within noise" means the 95% interval includes zero.

**Baselines.** `make perf-hear TAKES=latest` at commit `2ffc42c4`:

| Ghost's read (ort, 4 threads) | stage | turn | file |
|---|---|---|---|
| fp32 | 8.4% | 4.2% | 6.7% |
| int8 (`GHOST_STT_QUANTIZATION=int8`) | 9.8% | 8.4% | 10.2% |

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
  read) and are the same clock for every candidate. Takes with fewer than one or two
  timestamped words drop out (n in the table).
- *First two right.* The first two words shown were the reference's first two.
- *Work left at close.* The backlog when the last frame arrives, plus the time `finalize`
  takes: what a turn's close would wait for.
- *Rewrites.* Updates that change or remove an already shown word other than the last one.
  A last word growing token by token is not counted.
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
only (`~/.cache/ghost-stt-bench/runs/`), and nothing was printed or written as text. Model
downloads came from Handy's catalog at its pinned revisions, with each file's SHA-256 checked
(`scripts/models.txt`, `scripts/fetch.sh`).

## Results

### Batch: the whole take read once

The baseline is Ghost's fp32 `file` read (6.7%). Model s/s is model seconds per second of
audio over the 49 spoken takes.

| Run | WER | Δ vs fp32 `file` (95% CI) | Numbers wrong | Term errors | Terms heard | Empty reads | Silent takes with words | Model s/s p50 / p95 | Load + first read | RSS after reads | Peak |
|---|---|---|---|---|---|---|---|---|---|---|---|
| ort fp32 (Ghost) | 6.7% | — | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.042 / 0.047 | 2.16 s | 2,561 MB | 2,561 MB |
| ort int8 | 10.2% | +3.5 pt (−3.8 to +11.6), within noise | 6/16 | 3/10 | 6/9 | 3/49 | 0/13 | 0.030 / 0.036 | 1.65 s | 1,148 MB | 1,148 MB |
| TDT v3 F32, tc CPU | 6.3% | −0.4 pt (−1.1 to 0.0), within noise | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.048 / 0.050 | 0.26 s | 2,567 MB | 2,600 MB |
| TDT v3 F16, tc CPU | 6.3% | same | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.053 / 0.056 | 0.49 s | 1,660 MB | 1,692 MB |
| TDT v3 Q8_0, tc CPU | 6.3% | same | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.049 / 0.052 | 0.42 s | 1,168 MB | 1,200 MB |
| TDT v3 Q8_0, tc CPU, library's threads | 6.3% | same | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.044 / 0.048 | 0.41 s | 1,168 MB | 1,200 MB |
| TDT v3 F32, tc Vulkan | 6.3% | same | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.113 / 0.130 | 1.83 s | (GPU) | (GPU) |
| TDT v3 F16, tc Vulkan | 6.3% | same | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.145 / 0.180 | 1.83 s | (GPU) | (GPU) |
| TDT v3 Q8_0, tc Vulkan | 6.3% | same | 8/16 | 1/10 | 8/9 | 1/49 | 0/13 | 0.070 / 0.083 | 0.74 s | (GPU) | (GPU) |
| Parakeet Unified English Q8_0, tc CPU | 3.9% | −2.8 pt (−9.0 to +2.3), within noise | 2/16 | 3/10 | 6/9 | 0/49 | 0/13 | 0.048 / 0.056 | 0.39 s | 1,111 MB | 1,143 MB |
| Parakeet Unified English F16, tc CPU | 3.9% | same | 2/16 | 3/10 | 6/9 | 0/49 | 0/13 | 0.052 / 0.054 | 0.43 s | 1,596 MB | 1,628 MB |
| Parakeet Unified English Q8_0, tc Vulkan | 3.9% | same | 2/16 | 3/10 | 6/9 | 0/49 | 0/13 | 0.072 / 0.083 | 0.49 s | (GPU) | (GPU) |
| Nemotron Streaming English Q8_0, tc CPU | 11.2% | +4.6 pt (−1.2 to +10.5), within noise | 3/16 | 4/10 | 6/9 | 3/49 | 0/13 | 0.051 / 0.058 | 0.43 s | 1,109 MB | 1,143 MB |
| Nemotron Streaming English F16, tc CPU | 10.9% | +4.2 pt (−1.6 to +10.0), within noise | 3/16 | 4/10 | 6/9 | 3/49 | 0/13 | 0.053 / 0.054 | 0.44 s | 1,594 MB | 1,628 MB |
| Nemotron 3.5 Streaming Q8_0, tc CPU | 9.5% | +2.8 pt (−2.1 to +7.4), within noise | 3/16 | 4/10 | 6/9 | 0/49 | 0/13 | 0.060 / 0.063 | 0.50 s | 1,206 MB | 1,241 MB |
| Canary 180M Flash Q8_0, tc CPU | 15.8% | +9.1 pt (−0.6 to +20.9), within noise | 7/16 | 5/10 | 5/9 | 3/49 | 0/13 | 0.025 / 0.031 | 0.14 s | 344 MB | 457 MB |
| Canary 180M Flash Q8_0, tc Vulkan | 15.1% | +8.4 pt (−1.1 to +20.2), within noise | 7/16 | 5/10 | 5/9 | 3/49 | 0/13 | 0.054 / 0.064 | 1.33 s | (GPU) | (GPU) |
| Moonshine Streaming Medium Q8_0, tc CPU | 10.5% | +3.9 pt (−3.7 to +11.8), within noise | 2/16 | 2/10 | 7/9 | 3/49 | 1/13 | 0.067 / 0.082 | 0.31 s | 658 MB | 669 MB |

`tc` is transcribe.cpp 0.4.2. "Same" means the same reads as TDT v3 F32 on transcribe.cpp,
take for take. No configuration made up a term. ort fp32's load without the first read is
1.98 s, and its RSS right after load is 2,217 MB. transcribe.cpp's RSS right after load is
176 to 595 MB, because it maps the weights and touches them on the first read.

Against the fp32 `turn` read (4.2%, one read of the turn at the close, what Ghost ships),
batch Unified English is −0.4 pt (−4.9 to +3.6), within noise. TDT v3 Q8_0 on
transcribe.cpp is +2.1 pt (−2.6 to +7.5), and ort int8 is +6.0 pt (+0.4 to +12.8), worse.

### Streaming: fed in 100 ms frames

The WER is of the finalized stream. The arrival columns are p50 / p95 in seconds after the
word was said (n = takes with that many timestamped words).

| Run | Setting | WER (final) | Δ vs fp32 `turn` 4.2% | Δ vs fp32 `stage` 8.4% | First word shown | Two words shown | First two right | Compute s/s p50 / p95 | Slowest frame p95 | Work left at close p50 / p95 | Rewrites of shown words |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Parakeet Unified English Q8_0 | default (C 1,040, R 1,040 ms) | 6.0% | +1.8 (−2.4 to +5.9), within noise | −2.5 (−7.5 to +2.6), within noise | 1.98 / 2.72 (37) | 1.66 / 2.48 (34) | 26/34 | 0.116 / 0.166 | 322 ms | 0.30 / 0.52 s | 0 of 95 updates |
| Parakeet Unified English Q8_0 | C 560, R 560 ms | 7.4% | +3.2 (−0.9 to +7.4), within noise | −1.1, within noise | 1.25 / 1.65 (44) | 1.09 / 1.49 (39) | 30/39 | 0.214 / 0.304 | 304 ms | 0.28 / 0.52 s | 0 of 173 |
| Parakeet Unified English Q8_0 | C 160, R 160 ms | 7.7% | +3.5 (−0.7 to +7.6), within noise | −0.7, within noise | 0.69 / 1.05 (48) | 0.49 / 0.80 (41) | 28/41 | 0.798 / 1.060 | 290 ms | 0.90 / 2.42 s | 1 of 347 |
| Parakeet Unified English Q8_0 | C 80, R 0 ms | 30.2% | +26.0 (+16.1 to +36.6), worse | +21.8, worse | 0.56 / 1.03 (43) | 0.42 / 1.02 (36) | 21/36 | 1.609 / 2.066 | 515 ms | 4.18 / 9.33 s | 0 of 295 |
| Parakeet Unified English F16 | default | 6.0% | +1.8, within noise | −2.5, within noise | 2.00 / 2.73 (37) | 1.68 / 2.50 (34) | 26/34 | 0.126 / 0.182 | 360 ms | 0.33 / 0.60 s | 0 of 95 |
| Parakeet Unified English Q8_0, Vulkan | default | 6.0% | +1.8, within noise | −2.5, within noise | 2.06 / 2.75 (37) | 1.74 / 2.56 (34) | 26/34 | 0.184 / 0.248 | 431 ms | 0.43 / 0.71 s | 0 of 95 |
| Nemotron Streaming English Q8_0 | default (lookahead 1,040 ms) | 10.9% | +6.7 (+1.2 to +12.3), worse | +2.5 (−2.6 to +7.7), within noise | 0.91 / 1.69 (40) | 0.51 / 1.29 (38) | 29/38 | 0.061 / 0.068 | 95 ms | 0.07 / 0.12 s | 0 of 112 |
| Nemotron Streaming English Q8_0 | lookahead 480 ms | 10.2% | +6.0 (+1.4 to +10.8), worse | +1.8 (−3.0 to +6.9), within noise | 0.89 / 1.41 (42) | 0.57 / 0.93 (39) | 29/39 | 0.084 / 0.093 | 72 ms | 0.06 / 0.09 s | 0 of 174 |
| Nemotron Streaming English Q8_0 | lookahead 80 ms | 11.9% | +7.7 (+2.1 to +13.8), worse | +3.5 (−1.1 to +8.8), within noise | 0.65 / 1.05 (42) | 0.43 / 0.85 (38) | 24/38 | 0.195 / 0.203 | 57 ms | 0.05 / 0.08 s | 0 of 324 |
| Nemotron Streaming English F16 | default | 10.9% | +6.7 (+1.2 to +12.5), worse | +2.5, within noise | 0.91 / 1.69 (40) | 0.67 / 1.45 (38) | 31/38 | 0.065 / 0.072 | 101 ms | 0.08 / 0.13 s | 0 of 112 |
| Nemotron Streaming English Q8_0, Vulkan | default | 11.2% | +7.0 (+1.4 to +13.0), worse | +2.8, within noise | 1.06 / 1.83 (40) | 0.74 / 1.60 (38) | 29/38 | 0.180 / 0.201 | 247 ms | 0.24 / 0.34 s | 0 of 112 |
| Nemotron 3.5 Streaming Q8_0 | default (1,040 ms) | 9.5% | +5.3 (0.0 to +10.8), within noise | +1.1, within noise | 1.00 / 1.79 (39) | 0.69 / 1.47 (38) | 26/38 | 0.070 / 0.081 | 113 ms | 0.08 / 0.13 s | 1 of 114 |
| Nemotron 3.5 Streaming Q8_0 | lookahead 480 ms | 10.5% | +6.3 (+1.1 to +11.6), worse | +2.1, within noise | 0.89 / 1.42 (39) | 0.54 / 0.94 (38) | 24/38 | 0.092 / 0.100 | 77 ms | 0.06 / 0.10 s | 1 of 180 |
| Moonshine Streaming Medium Q8_0 | default | 10.2% | +6.0 (−0.7 to +13.5), within noise | +1.8, within noise | 0.37 / 1.45 (47) | 0.18 / 0.97 (41) | 16/44 | 1.368 / 1.676 | 359 ms | 3.08 / 6.43 s | 90 of 332 (33 of 49 takes) |
| **Ghost today: fp32 interim re-read every 0.3 s (ort)** | — | (finals are the batch rows) | | | 0.44 / 0.87 (48) | 0.13 / 0.45 (41) | 33/41 | one read ≈ 0.11 s | — | — | — |

No configuration's `finalize` changed the words already on screen. In 49 of 49 takes the
words shown before the close were a prefix of the final text, apart from the last word.
For Ghost's interim, the median take needed one read: the first tick, 0.3 s after the VAD
started, reading about 1.5 s of audio, already showed two words.

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
- **Repeated timing runs.** Each timing is a single pass on a shared machine.
- **The large models.** Voxtral 4B and Canary-Qwen 2.5B were skipped as asked, as were the
  other ~60 catalog entries.
