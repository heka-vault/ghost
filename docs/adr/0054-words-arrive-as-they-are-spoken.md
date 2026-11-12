# 0054: Early words may start the answer; the stop's words still decide it

- **Status:** proposed
- **Date:** 2026-11-12
- **Amends:** ADR 0049 (the early read is built for a second use), ADR 0044 (unchanged: the
  turn is still read whole at its close)

The answer to a turn starts on its words, and the words come from a read after the VAD's
0.2 s stop: 0.17 s at the p50. The second round built a read at the first quiet frame that
has the words when the stop is confirmed, and left it off because its words differ from the
stop's in about one segment in ten. The words a turn closes on must be the words Ghost hears
today. They need not be the words the answer is started on: the answer is held until the
turn is confirmed, and is started again when later words change the turn (ADR 0049).

## Decision

**The early read's words start the speculative answer and nothing else**
(`GHOST_STT_EARLY_READ=speculate`, off by default; `GHOST_STT_EARLY_READ_AFTER_SECS`, 0.06
measured). The speech stage reads a segment once 0.06 s of quiet is heard and sends the
words on as `Frame::EarlyTranscript` the moment they are read, then reads the segment again
at the stop as it always has. Early words are dropped wherever their transcript would be:
by the echo filter, by the hold gate, while a change waits for its yes, and when an
interceptor would take them (asked; nothing it decides is done, `TurnInterceptor` only
decides). The user aggregator gives them to the stop strategy for a turn's first segment
only, since a later one is read whole at the close. The answer starts on them once the hold
begins, and the stop's transcript then keeps it (the same words) or starts it again (other
words). The turn closes on, the model receives and the transcript stores the stop's words,
every time. Early words that arrive after the stop's, or before speech resumes, are moot.

## Measured

[The benchmark, third section](../benchmarks/perf/2026-11-12-voice-speed.md#part-b-words-before-the-stop).
Scripted with the fast model's measured times and the windowed voice: a plain reply 1.17 s
to 1.03 s at the p50, a reply after a lookup unchanged (its call waits for the grace
anyway). The stop's read waits behind the early one on Parakeet's lock, 45 ms later at the
p50; the turn's release moves 11 ms. On Kokoro's takes the word error rates are identical
(3.0 % stage, 2.1 % turn), and 3 of 54 early reads would have started an answer on other
words.

## Considered options

- **A streaming recognizer** (NVIDIA Nemotron Speech Streaming 0.6B, cache-aware, ONNX in
  the `parakeet-rs` crate). The words would be there as the speech ends, but it is a second
  0.6B model on six cores beside Parakeet, Smart Turn and Kokoro, under the NVIDIA Open
  Model License (the newer multilingual model's terms conflict between NVIDIA's card and
  its mirrors), with a new parity suite and a last chunk still to flush; over the early read
  it would save 0.05 to 0.1 s more. Worth it when a streaming turn model can decide the end
  on the audio as it goes, which would take the VAD's stop and most of the grace with it.
- **The early words as the segment's words** (`on`, the second round). Faster by the stop's
  read, but the words differ from today's in one segment in ten, and a full stop where
  there was a comma changes how the turn closes.

## Consequences

- One more Parakeet read per segment (about 0.17 s of a core), and the reads of half
  segments thrown away when speech resumes (16 of 37 takes, 3.5 s of model).
- About one answer in twenty is started on words that then differ, a model call thrown away.
- `hear.speculation` in `latency.jsonl` counts `early`, `early_same` and `early_differs`.
- The hear harness feeds a take faster than real time, so its close metric counts the whole
  early read as a wait (p95 0.74 s against 0.22 s); `make perf` is the measure of the close.
