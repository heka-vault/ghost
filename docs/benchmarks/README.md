# Benchmarks

Measured on the target workstation, one report per day's work, newest last. The numbers
that decide something are also in the ADR or the issue that decided it; a report here is
the record of how they were measured.

| Report | What |
|---|---|
| [2026-10-16-speech-cpu.md](2026-10-16-speech-cpu.md) | STT and TTS engines on CPU (ADR 0004), model time to first token |
| [2026-10-17-jev-confirmation.md](2026-10-17-jev-confirmation.md) | Jev against the regex allowlist on short operator turns |
| [2026-10-19-latency.md](2026-10-19-latency.md) | Where a turn's time goes, the first harness runs (ADR 0018) |
| [2026-10-20-remote-cellular.md](2026-10-20-remote-cellular.md) | The phone over cellular through the tunnel (ADR 0020) |
| [2026-10-21-wake.md](2026-10-21-wake.md), [2026-10-22-wake-detector.md](2026-10-22-wake-detector.md), [2026-10-23-ghost-openers.md](2026-10-23-ghost-openers.md) | The wake phrase: the transcript matcher, the trained detector, "Ghost, <opener>" (ADR 0025) |
| [2026-10-22-cloud-speech.md](2026-10-22-cloud-speech.md) | A cloud voice and interim barge-in (ADR 0026) |
| [latency-harness.md](latency-harness.md) | How the live timeline and `make perf` work |
| [perf/](perf/) | Every release's `make perf` report |

## The scripts

`make perf`, `make perf-micro` and `make perf-llm` are `ghostd perf run|micro|llm`, in
the daemon (`crates/ghost-voice/src/perf/`, ADR 0039). The Python under
[scripts/](scripts/) is what the reports above were made with; each says what it needs:

| Script | Runs with | What |
|---|---|---|
| `wake_eval.py` | `make wake-eval` (the oracle, `tools/oracle`) | What Parakeet hears in recorded clips and whether the transcript matcher fires |
| `wake_takes_eval.py` | `make wake-takes-eval` (the oracle) | The wake takes scored: the matcher against the trained detector, alone and checked |
| `jev_make_utterances.py` | the oracle | Short operator turns as Parakeet hears them, for a classifier's test set |
| `llm_ttft.py` | Python 3 alone | Time to the first token over an Anthropic-compatible endpoint, the 2026-10-16 probe |
| `stt_bench.py`, `tts_bench.py`, `gen_clips.py`, `run_all.sh` | their own venv (`faster-whisper`, `moonshine`, `piper`, `jiwer`, `soundfile`) | The 2026-10-16 engine comparison; `run_all.sh` runs them all |

Two scripts went with the Python daemon (W10 of heka-vault/ghost#39), since they imported
its packages and the Rust harness measures what they measured:

- `barge_in_bench.py` (how long Ghost kept talking once the operator talked over it, interim
  transcripts off and on): `ghostd perf run` times every barge-in from the operator's second
  heard word to the device cleared, and the 2026-10-29 report has the number (0.7 ms).
- `jev_eval.py` (Jev against the regex allowlist on a labelled set, accuracy per intent and
  latency): `ghostd perf micro --only jev` times the classifier; the accuracy study is the
  2026-10-17 report, made once, and the classifier's own tests in `ghost_core::jev` and
  `ghost_core::confirmation` keep its verdicts.

## The live rubric

[../voice-rubric.md](../voice-rubric.md) is the scripted conversation the operator has
with Ghost after a change to the voice path, with the debug lines that mark each item
passed; `scripts/voice-rubric-check.sh` reads a run out of the log and the latency
records. Each run's results go at the end of that file.
