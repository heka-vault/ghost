# 0055: Each connection is checked, and every request is shaped to what it takes

- **Status:** proposed
- **Date:** 2026-11-12
- **Amends:** ADR 0037 (Test connection becomes the provider check; a refused field is
  dropped and retried), ADR 0050 (the fast model's thinking setting is now a property of
  its route, not a rule about fast models)
- **Builds on:** ADR 0018 (performance is a feature), ADR 0036 (the key never comes out)

The owner's decision of 2026-11-12: Ghost works as well as it can for however the operator
has it configured, across the providers it supports: their own subscription through a proxy,
an API direct, OpenRouter, OpenAI, a local model. That morning's live run was OpenRouter
with Haiku 5.5 in front of Sonnet 5.5: the fast model's first byte came 1.1 to 2.7 s after
the call, against 0.5 to 0.9 s through the subscription proxy on the bench, and nothing in
the product said so or did anything about it. Earlier the same day Haiku 5.5 refused the
session's thinking setting (`between_tools`) with HTTP 400 on every turn. Only Sonnet 5.5
takes `between_tools`; Opus 5.5 and Haiku 5.5 refuse it too, so an operator who picked
either as the main model would have had every turn fail. A failed turn was silent: the stage
pushed `Frame::Error`, which no stage reads.

## Decision

**A provider check the product runs.** `POST /model/check`, the Test button in Settings ›
Model, and `ghostd llm test` run the same check (`ghost_llm::check`) through the connection
as configured, or a draft of it. The daemon reads the password from the vault at the point
of use; it is never shown. For the main model and the fast model it reports: whether it
answers, or the error in plain words; the time to the first byte and to the first word over
three short streamed calls (median and worst); whether the reply streams or arrives all at
once; whether a tool call comes back as one; whether a second identical request reads its
prefix from the cache (a fixed text of about 4,500 tokens, past every current model's
smallest cacheable prefix); and whether the session's thinking setting, or
`reasoning_effort` on Chat Completions, is accepted. Every prompt is fixed and synthetic,
never the operator's. Each model's result is kept in SQLite (`model_checks`, the last 20 per
route), with the time.

**Capabilities belong to a route** (`ghost_llm::capabilities`): a provider, its address and
a model. What a route has refused (a thinking setting, `reasoning_effort`, the cache markers)
is left out of every later request on it. A route learns two ways: the daemon seeds every
route's latest check at start, and a request refused with HTTP 400 or 422 that names a field
Ghost sent is sent again once without it, and the route remembers. So the Haiku 5.5 400 costs
one refused request in the life of the daemon, on any provider, instead of every turn. The fast
model's own rule stays (`ghost_voice::llm::fast_options`, ADR 0050).

**Sensible fallbacks, never a silent turn.** A rate limit (429), an overloaded provider (529,
503) or a passing server error (500, 502, 504) is tried once more after the wait the provider
asked for (`retry-after`, or OpenRouter's `retry_after_seconds`), at most 2 s, since the
operator is listening. Anything else, and a second failure, is said: the stage speaks a plain
sentence for the failure (`ghost_llm::spoken_failure`: "The model provider is limiting how
fast I can ask. Try again in a moment.") as the turn's reply, never a key, a body or an
address. A run with nobody there keeps its own retries (ADR 0047).

**A slow connection is said, with what would help.** When the model that answers a spoken
turn first (the fast model if there is one) takes more than 1.5 s to its first word, by its
latest check or by the operator's last ten spoken turns in `latency.jsonl` (the first model
call's time to first byte, at least five turns), the waiting list carries one note
(`model:slow`), and Settings shows it with the check. It says how slow, then what would help:
a fast model on the connection, the fastest of the operator's other tested routes that was at
least 0.3 s quicker, and that the connection sends replies all at once if it does. **Ghost
never switches provider or model on the operator's behalf.** The note names options. The
setting stays the operator's.

**A conformance suite** (`crates/ghost-llm/tests/conformance.rs`): a local fake server for
each shape the connection supports (Anthropic direct; a subscription proxy; OpenRouter on its
Anthropic API with its keep-alive comments; OpenRouter on its OpenAI API; OpenAI; Ollama,
which sends a reply's calls in one chunk; LM Studio; a custom URL). Each gets the same turns:
a streamed reply at its own path with its own key header, a tool call, a reply cut at the
cap, a rate limit that passes and one that lasts, an overloaded provider, a timeout, a
malformed chunk and a refused key. Every one must deliver the reply whole or fail with an
error Ghost says.

**Measured, per provider** (`make perf-providers`): the same check against every provider
named on the command line that the environment has a key for. The key comes from an
environment variable named on the command line, never the vault, and is never printed. The
result is a dated comparison under `docs/benchmarks/perf/`.

## Borrowed

Hermes Agent (MIT) classifies a provider's error into a recovery: its
`reasoning_mandatory` is "send the disable no more on this route and retry". That is the
shape of the refused-field rule here. Goose (Apache-2.0) reads a 429's wait from OpenRouter's
body before the `Retry-After` header and caps it (`extract_retry_after`). `ghost_llm::retry_after`
is adapted from it, with attribution in the file. Goose's `test_provider_configuration`
sends a sample weather tool to check a provider; Ghost's check adds timing, streaming, caching
and thinking, because a voice turn depends on all four.

## Considered options

- **Switch to the faster route automatically.** That is faster for the operator who wants
  it, and wrong for the one who chose a provider for its price, its privacy or its account. The
  owner's rule: never switch on the operator's behalf. The note names the option instead.
- **A table of capabilities per model id.** It is right on the day it is written and wrong
  on a proxy that strips a field, or the day a provider changes one. The route's own answer is
  the truth. The check asks, and a refusal teaches.
- **Retry everything with backoff, as the SDKs do (2 retries, exponential).** A spoken turn
  can't wait tens of seconds. One try after at most 2 s, then the operator hears why.
- **Run the check in the background on a schedule.** It would keep the note current
  without a click, but it spends the operator's tokens unasked. The operator's own turns
  already say whether the connection is slow, and those come free.

## Consequences

- A refused field costs one extra request, once per route per daemon, and none once a check
  has seeded it. A rate-limited turn waits up to 2 s more before it answers or says why.
- A check costs, per model: one call with the thinking setting, three timed calls, one tool
  call and two calls with a 4,500-token prompt. That is a few cents on Sonnet 5.5, and it
  runs only when asked.
- `/model/test` and its `Probe` are gone: `/model/check` replaces them, and
  `GET /model/check` returns the latest kept check of the saved connection.
- The failure sentence enters the conversation as Ghost's words, as the cut-off line
  already does (ADR 0037), so the model sees that the last turn failed.
