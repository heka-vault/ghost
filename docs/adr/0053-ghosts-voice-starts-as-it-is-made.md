# 0053: Ghost's voice may start as it is made: Kokoro's vocoder a window at a time

- **Status:** proposed
- **Date:** 2026-11-12
- **Amends:** ADR 0004 (Kokoro stays the voice; how it renders a piece is a setting),
  ADR 0026 (the cloud voice connects ahead of the words and cuts its first piece early),
  ADR 0049 (the clause rendered ahead streams too)

Kokoro rendered a piece whole before any of it played, and on this workstation that costs
0.16 to 0.21 s per second of speech: 340 ms for four words, 1.3 s for sixteen. Measured
([the benchmark, third section](../benchmarks/perf/2026-11-12-voice-speed.md#third-round-a-voice-that-starts-as-it-is-made-and-words-read-before-the-stop)),
92 to 97 % of that is the vocoder, which grows with the audio; the text side is 20 to 100 ms.
The session's threads and graph options were already the fastest. The CPU options for a
first chunk near 200 ms were Kyutai Pocket TTS (CC-BY-4.0 weights, gated; slower than
Kokoro here once started, and a port of its loop to build) and Kokoro's own vocoder run a
few frames at a time.

## Decision

**Kokoro's graph is cut in two and its vocoder runs a window of frames at a time**
(`ghost_infer::kokoro_windowed`, `GHOST_TTS_STREAM=windowed`). The encoder renders the
piece's frames, the pitch and the durations; the decoder renders a first window of 8
frames (0.2 s of speech) and windows twice as long after it, each with 16 frames of
context on both sides that are decoded and thrown away (`GHOST_TTS_WINDOW_FRAMES`,
`GHOST_TTS_WINDOW_CONTEXT`), each fading in over the one before for 50 ms. The harmonic
source's phase is carried into each window (`phase`), so the pitch runs on across a join
as it does in the whole render. The leading pad's frames are not rendered; a piece of 1.2 s
or less after them is one window and sounds exactly as it does whole. First audio, by the
piece's words: 180 ms for four, 181 for eight, 193 for sixteen, 236 for thirty-two, where
the whole render took 340, 630, 1,288 and 2,591 ms.

**It is a setting, off until the operator has heard it.** The vocoder's AdaIN layers
normalize each channel over the frames they are given, so a window is the vocoder's answer
for its frames, as a clause cut is today: 1 to 3 dB quieter than the same span of a whole
render, 2 to 4 dB of mean log-mel distance from it, no click at a join, the pitch and timing
exactly the whole render's. Context does not close that gap (1.2 s of it still leaves 2 dB).
Whether it sounds like Ghost is the voice rubric's D5 and D6; `whole` stays the default and
the old path.

**The stage plays what it gets as it gets it.** `Synth::render` hands a piece's audio on a
window at a time (a model that renders whole hands it once); the TTS stage pushes each
window as its own frame under the reply's token, so a barge-in clears the speaker as before
and stops the render at its next window. The echo filter is told a piece's words before its
first window. The first clause rendered ahead during the grace streams the same way and,
dropped, stops at its next window.

**The halves get to their place by a script.** `scripts/models.sh kokoro-windowed` cuts
`~/.cache/ghost/kokoro/kokoro-v1.0.onnx` (pinned by SHA-256) with
`tools/voice-trial/voice_trial/split_kokoro.py` under `uv` (onnx 1.19.1, locked) into
`~/.cache/ghost/kokoro/windowed/` (`kokoro-encoder.onnx` 112 MB, `kokoro-decoder.onnx`
213 MB), each pinned by SHA-256; `GHOST_TTS_STREAM_DIR` names another folder. The licence is
Kokoro's, Apache-2.0 (hexgrad/Kokoro-82M; the ONNX of thewh1teagle/kokoro-onnx's
`model-files-v1.0` release). The bundle carries neither, as it never carried Kokoro. Asked
for and missing or broken, Ghost speaks with the whole render and the waiting list says
what is missing (`Models::problem`); `ghostd check` names it too.

**The cloud voice connects while the model thinks.** ElevenLabs streamed already, but its
socket opened when the first clause was ready, so the first reply of a session and the
first after a voice change paid the handshake after the words. It opens at the turn's first
frame now, its first piece is cut early as Kokoro's is, and a quiet socket is kept alive
(`inactivity_timeout=180`, an empty text a minute). Against a local stand-in with the
service's times, Ghost's share of first audio fell from 219 ms (fresh socket) and 62 ms
(open) to 26 and 25 ms.

## Consequences

- A windowed render costs 14 to 72 % more CPU than the whole one (the context is decoded
  twice); playback never waited for a window on this machine, under a load of 8.
- Kokoro is loaded with its halves beside it when the setting is on: 325 MB more memory.
- With the fast model in front and the real models, a plain reply went from 1.69 s to
  1.46 s at the p50, a reply after a lookup from 3.57 s to 2.95 s.
- The parity bar holds for the halves: run over every frame they are the whole model's
  samples bit for bit (`crates/ghost-infer/tests/kokoro_windowed.rs`).

## Considered options

- **Kyutai Pocket TTS.** First chunk near 200 ms reported on two cores of an M4; through
  sherpa-onnx here its callback came per sentence (288 ms for two words, 1.2 s for sixteen)
  and it rendered at 0.28 to 0.6 of real time against Kokoro's 0.16 to 0.21. Frame-by-frame
  streaming needs a port of its autoregressive loop over five graphs, and its voice is a
  clone of a reference clip, a new voice for the operator. The sherpa export's README calls
  it non-commercial while its licence file and Kyutai's card say CC-BY-4.0. Worth trying
  again on a faster CPU or with a native port.
- **Piper.** About ten times faster and clearly more robotic (ADR 0004).
- **Chatterbox-Turbo.** Needs a GPU; this machine has none.
- **More threads, another graph level, the int8 or fp16 Kokoro.** The threads and the level
  were already the fastest measured; the smaller files change the sound and the parity bar.
- **Shorter first pieces only (the early cut of ADR 0049).** Kept: it changes where the text
  is cut. The windows change none of the text and none of the prosody.
