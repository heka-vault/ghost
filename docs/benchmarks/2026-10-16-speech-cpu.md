# Speech models on CPU: desktop workstation, 2026-10-16

**Host:** i5-11600K (6C/12T, AVX-512), 32 GB, no GPU, Ubuntu 26.04. Each engine ran alone with 6 threads. Python 3.12, `faster-whisper` (int8, greedy), `onnx-asr` 0.12 (Parakeet int8), `sherpa-onnx` (Moonshine int8), `kokoro-onnx`, `piper-tts`.

**Clips:** eight Ghost-style commands synthesized with Piper `en_US-ryan-high` and resampled to 16 kHz, plus the real-speech `jfk.wav` (11 s) from whisper.cpp. Latency is the median of three runs after a warm-up. Word error rate (WER) is after lowercasing, removing punctuation and hyphens, and writing the reference's spoken numbers as digits.

Raw output: [2026-10-16-speech-cpu.results.jsonl](2026-10-16-speech-cpu.results.jsonl). One clip named a private project; that name is redacted as `<project>` in the raw transcripts, and the clip script now says "Acme" instead. Scripts: [scripts/](scripts/) (`run_all.sh <bench_root> <scripts_dir>`).

## Speech-to-text

| Engine | Per command (p50) | Command WER | "Confirm." / "Next issue." | JFK (11 s) | RAM | Load |
|---|---|---|---|---|---|---|
| **Parakeet TDT 0.6B v3** | **0.16 s** | **19%** | right / right | 0.31 s, 0% | 1.7 GB | 10 s |
| whisper large-v3-turbo | 3.46 s | 7% | right / right | 3.56 s, 0% | 2.3 GB | 21 s |
| whisper medium.en | 2.66 s | 7% | right / right | 3.05 s, 0% | 2.4 GB | 20 s |
| whisper small.en | 0.97 s | 32% | "TEMPERM" / "next tissue" | 1.11 s, 0% | 0.9 GB | 8 s |
| whisper distil-small.en | 0.83 s | 38% | wrong / wrong | 0.90 s, 32% | 0.6 GB | 6 s |
| whisper distil-large-v3 | 3.46 s | 52% | wrong / wrong | 3.48 s, 0% | 2.3 GB | 20 s |
| Parakeet TDT 0.6B v2 | 0.16 s | 49% | "Come firm." / "Next tissue." | 0.32 s, 0% | 1.4 GB | 11 s |
| Moonshine base | 0.14 s | 37% | "Temper" / "Next issue. Next" | 0.27 s, 0% | 0.4 GB | 1 s |
| Moonshine tiny | 0.08 s | 51% | wrong / wrong | 0.19 s, 0% | 0.3 GB | 0.4 s |
| whisper base.en | 0.39 s | 60% | wrong / right | 0.39 s, 0% | 0.4 GB | 5 s |
| whisper tiny.en | 0.22 s | 61% | wrong / right | 0.22 s, 0% | 0.3 GB | 2 s |

Every engine heard "Redis and Postgres" as something like "Riddies and Posters". The likeliest cause is that Piper mispronounced the words, so that clip says little about the engines. The "Confirm." clip is only 0.5 s long and may be clipped, which makes the short-command results look worse than real use would.

## Text-to-speech

| Engine | First sentence (1.6–1.9 s audio) | 2-sentence reply (~4 s) | Long reply (15–17 s) | Real-time factor | RAM |
|---|---|---|---|---|---|
| **Kokoro v1.0 fp32** (`af_heart`) | **0.37 s** | 0.99 s | 3.98 s | 0.23 | 1.1 GB |
| Kokoro v1.0 int8 | 0.37 s | 1.02 s | 4.61 s | 0.26 | 0.9 GB |
| Piper ryan-high | 0.18 s | 0.36 s | 1.68 s | 0.11 | 0.4 GB |
| Piper lessac-medium | 0.04 s | 0.09 s | 0.34 s | 0.02 | 0.3 GB |

int8 Kokoro was no faster than fp32 on this CPU.

## Conclusions

- **STT: Parakeet v3.** Fast enough to disappear from the latency budget, and the only fast model that got one-word commands right. A whisper model could be a slow second pass for confirmations if needed.
- **TTS: Kokoro fp32,** streamed one sentence at a time. Piper as a fallback voice.
- **Turn budget:** see the LLM section below; about 1.5–1.8 s end to end, under the PRD's 2 s p50 target.
- **Memory:** Parakeet, Kokoro, and the wake-word model together need about 3 GB.

## LLM time to first token

Claude through an Anthropic-compatible proxy on the LAN, streaming, a one-line Ghost system prompt, 5 runs each ([scripts/llm_ttft.py](scripts/llm_ttft.py)):

| Model | `thinking` | First text token (p50) | Full reply (p50) |
|---|---|---|---|
| **`claude-sonnet-5-5`** | **`{"type": "between_tools"}`** | **0.77 s** (0.71–0.80) | 2.2 s |
| `claude-haiku-5-5` | `{"type": "disabled"}` | 0.88 s (0.82–0.94) | 1.3 s |
| `claude-haiku-5-5` | adaptive (default) | 1.85 s | 2.2 s |
| `claude-sonnet-5-5` | adaptive (default) | 3.30 s | 4.0 s |

- Turn off thinking before the reply, or speech starts 2–4x later. Sonnet 5.5 rejects `{"type": "disabled"}` and asks for `{"type": "between_tools"}` (no thinking before the reply; short updates between tool calls come back as thinking blocks). Haiku 5.5 accepts `disabled`.
- With adaptive thinking and a small `max_tokens`, Haiku spent the whole budget thinking and returned no text.
- **Turn budget with these numbers:** end-of-turn detection about 0.2–0.5 s (not measured), plus STT 0.16 s, plus Sonnet first token 0.77 s, plus TTS first sentence 0.37 s, comes to about 1.5–1.8 s.

## Not measured yet

- The operator's real voice and microphone, with music playing.
- Everything running at once (wake word, STT, TTS) during a live conversation.
