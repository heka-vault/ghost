# 0050: A fast model speaks first, and hands real work to the main one

- **Status:** proposed
- **Date:** 2026-11-12
- **Amends:** ADR 0037 (the connection may name a second model for a spoken turn's first
  call; everything else it says stands)
- **Builds on:** ADR 0049 (the wait before a reply is now the model's), issue
  [#9](https://github.com/heka-vault/ghost/issues/9)

Once a turn closes in 0.35 s (ADR 0049), what the operator waits for is the model's first
token: 1.1 to 1.5 s on the main model, against about 0.6 s for a small one on the same
connection (2026-10-19). A small model is good at "Doing well, what do you need?" and
worse at the work: through the proxy it answered 3 of 10 write requests with a question
(#9). The leaders split the two. OpenClaw's realtime voice gives its front model one
tool, an agent consult, that runs the full agent; Hermes's live mode makes every real
request a delegation to the agent behind it. In both, the front model talks and the agent
works.

## Decision

**The connection may name a fast model.** `GHOST_LLM_FAST_MODEL` is a second model on the
same connection: the same provider, address, password and headers (ADR 0037), another
model id. Unset, or set to the main model, there is no fast model and nothing changes.
`GET /model` shows it (`fast_model`, `fast_wire_model`), and it is saved like the rest of
the connection, in settings.env. Only a session's turns use it; compaction, a watch's model
step and everything that calls a tool stay on the main model.

**It gets a turn's first call, and only that.** With a fast model set, the first model
call of a turn (the speculative answer during the grace, or the turn's own run) goes to
it, with the same system prompt, the same context and the same tools, and one more tool
after them, `think_harder`, which takes no arguments and is never run.

- **Plain speech is the reply.** If the fast model answers in words and calls nothing,
  that is what Ghost says, and the main model is not asked.
- **Any tool call hands the turn over.** If it calls `think_harder`, or any other tool,
  or writes nothing, or fails, or hits the output cap, the turn goes to the main model
  on the same context (`Frame::HandOver`). The fast model's call is not run and nothing
  it wrote is spoken, recorded or put in the context: the main model makes the call
  itself. Every call after a tool's result is the main model's. So what runs a tool, and
  what reaches the gate, is always the main model.
- **Nothing of the fast model's is spoken until it has finished.** Its reply is held in
  the model's stage until it ends, because a call can follow any amount of text and the
  narration cut releases a long reply as it comes. That costs a long spoken answer its
  streaming: it starts once the fast model has written all of it.
- **An acknowledgment covers the gap.** At a hand-over Ghost says one of its fixed words
  ("Sure.", "One sec.", or one of the change words when the call was a change), once a
  turn, as it already does when the main model goes to an app. A goodbye or a hold is
  not acknowledged; its own words follow. Because nothing has been done for the turn yet,
  the operator may still be going on after a pause (ADR 0049): the word waits until the
  turn's grace has run from the end of their speech and is dropped if they went on, and
  words said over it, or speech still going after it ended, take the turn back as words
  in silence do.

**Prompt caching works for both.** Each model has its own cache at the provider. The fast
model's request is the main model's with one tool added at the end of the list, so its
tools, system prompt and messages are stable from turn to turn and cache as the main
model's do. The main model's request is unchanged, byte for byte, and what either model
said enters the context the same way, so each model's next call finds its prefix.

## Measured

With the scripted model taking each model's assumed time (`sonnet`: first token 1.2 s, 60
tokens/s; `haiku`: 0.45 s, 150 tokens/s; not a measurement of either), `make perf`,
[the benchmark](../benchmarks/perf/2026-11-12-voice-speed.md):

| reply p50, end of speech to Ghost's voice | the main model alone | a fast model in front |
|---|---|---|
| "Hi Ghost, how's it going?" | 1.99 s | 1.28 s |
| a question with no tool | 2.4 s | 1.66 s |
| one lookup | 4.18 s | 4.93 s |
| an issue and its comments | 5.61 s | 6.41 s |
| a goodbye only the model recognizes | 2.68 s | 2.98 s |
| first sound on a turn that goes to an app | 2.23 s | 1.48 s |

Both columns have the short grace and the first clause rendered ahead (ADR 0049); six
runs a row (the question with no tool is from twelve on a quiet machine; the six gave
2.71 s under load). A plain reply comes about a second sooner, where before it waited for the
main model's first token. A turn that goes to an app answers 0.7 s later, the 0.67 s
the fast model takes to ask, and its acknowledgment comes 0.75 s sooner. Of 90 spoken
turns in those runs 65 were handed over (the scenarios are mostly lookups and changes),
and none spoke a word of the fast model's.

## Considered options

- **The fast model's own tool calls run.** A read is safe to run (it is called directly,
  ADR 0034) and the main model would still write the answer, which would take a model
  call off every lookup instead of adding one. But a wrong repository or a wrong number
  from the small model becomes the main model's premise, and for a change the small model
  would be what reaches the gate. Hand-over keeps one rule: the main model does the work.
  Worth measuring with a real model before deciding again.
- **Both models asked at once, the slower one dropped.** No hand-over wait at all, for
  the main model's input tokens on every plain turn. It doubles the calls for the turns
  that needed it least.
- **A front model with a short prompt of its own and one consult tool** (OpenClaw,
  Hermes). Their front model is a speech-to-speech model with a small context; Ghost's
  is a text model on the same connection, and the same prompt and tools cost it nothing
  but cached tokens. With the same context it can answer "what did you just say about
  the cache?" itself.
- **A router that picks the model from the words.** Another model, or a rule that would
  be wrong in the ways Smart Turn is; the fast model deciding for itself is one call.

## Consequences

- A turn that needs a tool pays for the fast model's call before the main model starts:
  about 0.6 s with the assumed times, and the fast model's tokens. The acknowledgment
  comes sooner than it did; the answer comes later.
- Two models' caches are kept warm instead of one. After several plain turns the main
  model's cache may have expired (five minutes), and its next call writes it again.
- A goodbye only the model recognizes (`hang_up`) is a tool call, so it is handed over
  and is slower with a fast model than without. Letting the fast model hang up is a
  small follow-up if that grates.
- `handover` in `latency.jsonl` counts the turns handed over and the seconds the fast
  model took to ask; `model` on the record is the model that answered first.
- Whether a small model really answers plainly when it should and hands over when it
  should is not measured here: the harness's fast model is scripted to do the right
  thing. `make perf-llm` and the voice rubric with a real fast model are what decide
  whether this is turned on.
