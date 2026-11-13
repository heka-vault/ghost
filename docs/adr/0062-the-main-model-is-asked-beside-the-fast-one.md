# 0062: The main model is asked beside the fast one

- **Status:** proposed
- **Date:** 2026-11-12
- **Amends:** ADR 0050 (a hand-over no longer starts the main model; it finds it started),
  ADR 0046's amendment of 2026-11-12 on twin connections (a twin is read when the first
  cannot find a thing)
- **Builds on:** ADR 0018, ADR 0056

Measured call by call through a recording pass-through on 2026-11-12
([the benchmark](../benchmarks/perf/2026-11-12-reply-time.md)), a turn's time is spent in
three places Ghost controls: Haiku 5.5's hidden thinking (100 to 280 tokens, 0.5 to 1.1 s of
every call, its first byte coming at 0.5 s whatever the prompt's size), Sonnet 5.5's own
start (1.4 to 2.1 s to its first token whatever the thinking or effort setting), and the
number of calls. A change paid the fast model's whole call (1.3 to 1.9 s) to learn it was
not its to make, then the main model's start from nothing.

## Decision

**With a fast model, the main model is asked beside it** (`GHOST_LLM_RACE`, on). Every call
the fast model takes (a turn's first, and the answer after its own reads) also goes to the
main model with the same context, tools and options the main model would get. Its reply
waits in the stage. If the fast model hands over (a change, `think_harder`, a session tool,
a failure), that reply is the turn's, already on its way; if the fast model answers or makes
its reads, the main model's request is ended unread. Nothing of the main model's is spoken,
recorded, or put in the context unless the fast model handed over; what reaches the gate is
still the main model's call, made once (`with_the_race_a_hand_over_asks_the_main_model_once_and_its_change_runs_once`).

It also keeps the main model's prompt cache written: live, the main model's first call of a
turn read nothing from the cache (2.90 s to its first byte) because it had not been asked
since the fast model's tool list moved.

**A read a connection cannot find is read through its twin.** Two connections to the same
service with the same tools may be two accounts that see different things (live: an issue
only the second account could see was answered as not found). When a read's error says the
thing was not found (a fixed check on its text: `not found`, `404`, `does not exist`), the
dispatcher reads it through the twin with the same arguments, under the operator's block
rules, and returns what the twin found with the connection named. The twin line now says so,
and asks for both at once to list or count across them.

**The warm-up is keyed on the history too.** It was skipped when the same system prompt and
tools had been sent in the last four minutes, so a session that opened with more carried
turns than the last wrote them on its first call (about 3,000 tokens live).

**The fast model's thinking stays its own.** `GHOST_LLM_FAST_EFFORT` (`output_config.effort`)
and `GHOST_LLM_FAST_ANSWER_THINKING` (the answer after its own reads) are settings, off by
default. Thinking off on every call made each call 0.5 to 1.1 s sooner and lost accuracy
(a change made in 2 of 4 runs against 4 of 4, an issue by number read in 3 of 4); `low`
effort and thinking off for the answer only were inside the noise of a loaded machine.

## Costs

- The race: one main-model request per fast call, nearly all of it read from the cache
  (p50 20,400 tokens read, 0 written, in the harness); 65 of 106 ended before the main model
  sent anything. At Sonnet 5.5's cache-read price that is about $0.002 a fast call. Off with
  `GHOST_LLM_RACE=false`.
- The twin read: one more read on a not-found, which would otherwise have been a model round
  or a wrong answer.
- The warm-up: one more cache write per session when the history grew, off the turn's path;
  the first turn reads it.

## Considered options

- **The fast model makes changes itself.** It would save the main model's call on a change
  (about 3 s), and the gate would hold. But a change is where the small model's judgement
  matters most, and ADR 0050's rule stands until the voice rubric says otherwise.
- **A smaller first call for the fast model** (a short prompt, the core tools). Measured,
  the prompt's size is not where the time is: Haiku 5.5's first byte came at 0.46 s on a
  141-token prompt and 0.47 to 0.64 s on 20,000 cached tokens. Not built.
- **A realtime speech-to-speech model in front** that hands work to the agent: in the
  benchmark's last section, for the owner to decide. Not built.
