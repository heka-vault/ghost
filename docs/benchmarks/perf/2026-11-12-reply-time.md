# Perf: where a whole spoken reply's time goes, call by call (2026-11-12)

Branch `perf/reply-time`, after [the first sound](2026-11-12-first-sound.md) (PR #86). That round
brought the first sound to about 1.3 s on the bench; the whole reply was still 2.1 s for small
talk and 5.1 s for a lookup (three model calls), and about 11 s for a lookup on a slow provider.
This round measured every model call to find where that time goes, and checked the owner's live
run of the same evening (main `ed64dd26`, OpenRouter, Haiku 5.5 in front of Sonnet 5.5).

## How it was measured

- **A recording pass-through** between the harness and the provider (a small aiohttp proxy kept
  out of the tree): every request's body saved, and every server-sent event timed from the
  request (the message's start, each thinking, text and tool block's start and end), with the
  usage, including `output_tokens_details.thinking_tokens`. Its own overhead is a local hop.
- **Replays** of the recorded bodies with the thinking setting changed (left out, `effort: low`,
  `disabled`), three each, to time one decision without the pipeline around it.
- **The harness** (`ghostd perf run --no-jev --llm-url <tap> --llm-model claude-sonnet-5-5
  --llm-fast-model claude-haiku-5-5 --fast real`), the prompt padded to the operator's size
  (`pad=11000`, about 20,000 tokens of system and tools) and the tools hidden as on the operator's
  setup (`apps=deferred`, or `deferred-kept` for the state after a restart), with the provider's
  first byte delayed by 0.5 and 1.0 s for a slower provider. Seconds from the end of the operator's
  speech; p50 of three or four runs; *right* is the harness's checks (the issue read, the change
  made through the gate), and for the triage lookup that it listed the issues rather than reading
  Ghost's own status only.
- **The machine was shared**: other agents built at the same time. The load average is given with
  each run; it reached 44. The waits on the model are the network's and the provider's and do not
  move with load; the read and the voice do.
- The real provider of the bench is the local subscription proxy. The operator's is OpenRouter.

## Where the time goes

From the tap, the baseline run (main, `apps=deferred`, padded, 12 turns, load 2 to 5):

| | first byte (message start) | thinking ends | first word or call | thinking tokens | a call's end |
|---|---|---|---|---|---|
| Haiku 5.5, a turn's first call | 0.47 to 0.64 s | 1.0 to 1.6 s | 1.0 to 2.2 s | 61 to 281 | 1.1 to 2.4 s |
| Haiku 5.5, after a tool result | 0.47 to 0.61 s | 0.7 to 1.8 s | 0.7 to 1.8 s | 36 to 279 | 1.0 to 2.0 s |
| Sonnet 5.5 (thinking `between_tools`, so none) | 1.37 to 1.83 s | | 1.37 to 1.83 s | 0 | 1.4 to 2.5 s |

- **The prompt's size is not where the time is.** Haiku 5.5's first byte was 0.46 s on a
  141-token prompt and 0.47 to 0.64 s on 20,000 tokens read from the cache. The "first byte" of
  1.1 to 1.9 s the last round measured on the padded prompt was the first *visible* byte: after
  the hidden thinking. A smaller first call (the brief's first item) would save tens of
  milliseconds, so it was not built.
- **Haiku 5.5's hidden thinking is 0.5 to 1.1 s of every call.** Left out, the field means
  adaptive thinking at the model's default effort; it thought on 22 of 25 calls, small talk
  included sometimes.
- **Sonnet 5.5 starts in 1.4 to 2.1 s whatever is asked.** Replayed with `between_tools` alone,
  with `effort: low` and with `effort: medium`: 1.72, 2.10 and 1.82 s on a call, 1.49, 1.53 and
  1.44 s on an answer. Nothing in the request moves it.
- **The calls a turn makes**: small talk 1; the triage lookup 2 to 4 (a tool search when the
  list is hidden, the lists, the answer); an issue by number 3 (search, read, answer); a change
  4 (search, the fast model's change call, which it may not make, the main model's call, its
  answer). A mock tool takes 10 to 30 ms; the operator's real GitHub reads 0.2 to 1.6 s.
- **The fast model's call that hands a change over is all waste**: 1.3 to 1.9 s to write a call
  it may not make, and then the main model starts from nothing.

## The fast model's thinking, by setting

Replays of seven recorded Haiku 5.5 calls, three each (p50; *visible* is the first word or call):

| call | left out: thinking, visible | `effort: low` | `disabled` | same decision? |
|---|---|---|---|---|
| small talk | 0 tok, 0.58 s | 58, 0.81 | 0, 0.53 | yes |
| what's waiting for triage | 214, 1.62 | 209, 1.56 | 0, 0.51 | all three vary alike |
| an issue by number | 208, 1.70 | 175, 1.45 | 0, 0.52 | yes |
| close an issue | 136, 1.13 | 88, 1.00 | 0, 0.55 | yes |
| the read after a search | 208, 1.61 | 120, 1.16 | 0, 0.53 | yes |
| the answer from a read | 129, 1.09 | 81, 0.83 | 0, 0.50 | yes |
| the change after a search | 200, 1.71 | 132, 1.13 | 0, 0.54 | 1 of 3 read instead, left out |

One call at a time the decisions look the same. Whole turns do not: in the harness
(`apps=deferred-kept`, four runs each, load 2 to 22), thinking off made every turn quicker and
lost accuracy where the decision matters:

| scenario | left out: reply, right | `disabled`: reply, right |
|---|---|---|
| small talk | 1.95 s, 4/4 | 1.83 s, 4/4 |
| what's waiting for triage | 4.88 s, 3/4 listed the issues | 2.78 s, **0/4** (Ghost's own status only) |
| an issue and its comments | 4.73 s, 4/4 | 3.49 s, 4/4 |
| an issue by number | 5.79 s, 4/4 | 4.54 s, **3/4** |
| close an issue (a change) | 9.08 s, 4/4 | 6.59 s, **2/4** |

`effort: low` on every fast call and thinking off for the answer after reads only were tried in
the same harness and came out inside the noise of that run (load up to 22), with one change
missed with the answer's thinking off. So the fast model's thinking stays left out by default,
as PR #79 left it, and both settings are there for the operator
(`GHOST_LLM_FAST_EFFORT`, `GHOST_LLM_FAST_ANSWER_THINKING`; `GHOST_LLM_FAST_THINKING` as before).
The effort is sent as `output_config.effort` and a route that refuses it learns to leave it out
(ADR 0056).

## What was changed

- **The main model is asked beside the fast one** (`GHOST_LLM_RACE`, on, ADR 0062). Every call
  the fast model takes also goes to the main model with the same context; if the fast model hands
  over, the main model's reply is already on its way, and if not, its request is ended unread.
  The gate still sees only the main model's call, made once. Measured in one run, four each
  (`apps=deferred-kept`, padded): close an issue 9.26 s without, **7.20 s** with, 4/4 right both;
  the other scenarios inside the noise (small talk 2.59 and 2.94 s, the triage lookup 4.99 and
  4.76, an issue and its comments 4.52 and 5.54, an issue by number 6.47 and 5.85).
- **A read one connection cannot find is read through its twin**, in the same call, under the
  operator's block rules (ADR 0046's amendment; live item 4 below).
- **The warm-up is keyed on the carried history too** (live item 2 below).
- **Per-call settings for the fast model** (above), off by default.

## Before and after

Main (`c8608073`) against this branch, built separately, three runs each, `apps=deferred`, the
prompt padded, the provider's first byte delayed as shown. First sound / reply (p50), and right.
Load average p50 14 (max 45) for *before*, 8 (max 25) for *after*.

| scenario | +0: before | +0: after | +0.5 s: before | +0.5 s: after | +1.0 s: before | +1.0 s: after |
|---|---|---|---|---|---|---|
| small talk | 2.18 / 2.56, 3/3 | 1.81 / 2.87, 3/3 | 1.63 / 2.99, 3/3 | 1.73 / 3.28, 3/3 | 1.62 / 3.62, 3/3 | 1.63 / 3.50, 3/3 |
| what's waiting for triage | 1.64 / 5.32, 3/3 | 1.61 / 6.44, 3/3 | 1.62 / 7.17, 2/3 | 1.61 / 8.64, 3/3 | 1.65 / 9.02, 3/3 | 1.64 / 8.58, 2/3 |
| an issue by number | 2.57 / 8.12, 3/3 | 2.04 / **6.97**, 3/3 | 3.24 / 7.91, 3/3 | 2.34 / 8.11, 3/3 | 2.96 / 9.72, 3/3 | 3.39 / 8.76, 3/3 |
| close an issue (a change) | 1.96 / 9.36, 3/3 | 1.98 / **8.59**, 3/3 | 2.72 / 11.90, 3/3 | 2.48 / **11.25**, 3/3 | 2.85 / 12.84, 3/3 | 2.70 / **10.60**, 3/3 |

- **A change is answered sooner at every provider speed**: 0.8 s, 0.65 s and 2.2 s, the race's
  doing (the hand-over finds the main model started). Its accuracy is unchanged: every change was
  made through the gate.
- **Everything else is inside the noise of a loaded machine.** Three runs a cell under a load of
  8 to 45 cannot show a second either way; the lookups' spread within a cell was up to 3.6 s. The
  race adds nothing to a turn the fast model answers (its request is ended), and the per-call
  numbers above say where a lookup's time is: two or three Haiku calls of 1.0 to 2.4 s each.
- This harness cannot see the live items below: the bench's proxy drops the system prompt's
  cache marker (below), its mock reads take 10 to 30 ms, and its two GitHub connections see the
  same issues.

## The live run of the same evening (main `ed64dd26`, OpenRouter)

From the operator's debug log and `latency.jsonl` (timings only):

1. **The cache read nothing on the call after a tool.** "Read me issue 413": the fast model's
   read (11,607 tokens read from the cache), then a call that read **0** and wrote 12,396 (first
   byte 2.90 s), then the answer reading 12,396. The middle call is the main model: the fast
   model's result was long, so the main model answered. The main model had not been asked since
   the fast model's tool list last moved (a tool used or found joins the list, and tools come
   first in the cached prefix), so nothing of its prefix was cached. **The race fixes this**: the
   main model is asked beside every fast call, so its prefix is written with the fast model's
   (in the harness, the main model's completed calls read from the cache 31 times of 32). In the
   count question the call after the tool search read 0 and wrote 21,708: the search added three
   tools to the list, which rewrites the prefix by design. That one is inherent to finding tools
   by search while the list is the prompt's first part. What would remove it is the Messages
   API's own tool search with deferred loading, where a found tool does not change the cached
   prefix; it is not built (Ghost's search is its own, and whether OpenRouter passes the server
   tool through is not known).
2. **Every session's first call wrote about 3,000 tokens** (read 8,497 to 11,938, written 2,985
   to 3,278) though the warm-up runs as a session opens. The warm-up was skipped whenever the same
   system prompt and tools had been sent in the last four minutes (`llm::prefix_key` covered the
   model, tools, system prompt and thinking, not the messages), and a session opens with the
   agent's carried turns after the system prompt's marker: the previous session's turns were new
   each time. The key now covers the messages, so a session whose carried history grew is warmed
   while the greeting plays and its first call reads it
   (`a_prefix_sent_lately_is_not_warmed_again`). Not seen on the bench, whose sessions carry no
   history; the operator's next session is the check (`cache_write` on a session's first call).
3. **Times heard**: a plain reply 2.1 to 2.5 s (first byte 1.0 to 1.7 s through OpenRouter),
   the question no app answers 2.87 s, an issue by number 9.9 s (three calls, one of them the cold
   main model), the count of open issues 12.2 s (four calls). The first sound came at 1.7 to
   2.3 s, not the bench's 1.3 s: on OpenRouter the first word takes about 1.6 s, so the first
   sound's wait is its 0.8 s (ADR 0050), and the close was 0.8 to 1.4 s (the read took 0.19 to
   0.67 s against the bench's 0.17 s). Neither is changed here.
4. **"Read me issue 413" was answered as not found.** The twin rule of PR #86 told the model to
   read through one of two GitHub connections, never both; it read through the first in two
   repositories, got 404 from both, and said it could not find the issue. The second connection
   is another account that could see it. Now a read whose error says the thing was not found
   (a fixed check on the error's text: not found, 404, does not exist) is read through the twin
   by the dispatcher in the same call, as a read under the operator's block rules, and the answer
   names the connection that found it (`a_read_one_connection_cannot_see_is_read_through_its_twin`,
   `the_twin_is_not_read_when_the_operator_blocks_it`). The twin line no longer says "never
   both". Proving two connections are the same identity was not taken: no tool says so for every
   service, and the fallback is right either way.
5. **The count took four rounds**: a tool search, the lists through the first connection, the
   lists through the second, the answer. The twin line now says to ask both connections at once
   to list or count across them, and the list tool, once used, stays offered by name (kept in
   SQLite), so the next count is two rounds: both lists, then the answer. Not measured live.

The slow-connection note spoken at every wake is the owner's fix (`crates/ghostd/src/model_check.rs`
and the wake digest); untouched here.

## The read at the VAD's first quiet (ADR 0054)

`ghostd perf hear --takes latest --set stt_early_read=speculate` and `ghostd perf turns` with and
without it, on the operator's 49 recorded takes. The turns came out identical (39 pauses
mid-turn, 4 closed early, last-end wait 0.85 s), and 34 early reads were thrown away when speech
came back (14.8 s of Parakeet in all, about 0.44 s a time). Neither harness counts how often the
early words differ from the stop's on the operator's own voice: ADR 0054's one in ten is from
Kokoro's takes. The saving is at most the stop's read (0.17 s at the p50, more on a busy machine:
0.67 s once live), and only when the model is the wait. **Left off**: there is no measure of its
cost in wrong starts on real speech, and it adds a read to every pause. Counting `early_same` and
`early_differs` in the turns harness is the next step.

## Connection reuse

A new TLS connection to OpenRouter or Anthropic took 30 to 40 ms from here (`curl`), and the
model client keeps its connections pooled across turns. Not worth a change.

## Costs counted (ADR 0018)

- **The race**: one main-model request per fast-model call. In the harness 106 were ended unread,
  65 of them before the main model had sent anything; those that reported usage read 20,400 tokens
  from the cache (p50) and wrote none. At Sonnet 5.5's cache-read price that is about $0.002 a
  fast call, so about $0.20 for a hundred spoken turns. It also writes the main model's cache that
  a later call reads. `GHOST_LLM_RACE=false` turns it off.
- **The twin read**: one more read, only when the first connection says it cannot find a thing.
- **The warm-up**: a cache write of the carried history per session when it grew, while the
  greeting plays; the first turn then reads it instead of writing it.
- **Every turn**: the twin line is about 20 tokens longer where there is a pair, read from the
  cache. Nothing else on a turn's path.

## Against the target, and what would reach it

The targets: a first sound in 1.0 to 1.3 s, and a lookup answered in about 3 s, on whatever
provider the operator configured.

What a lookup is made of, with this architecture, on the bench's provider: the close (0.6 s),
the fast model's call (first byte 0.5 s, thinking 0.5 to 1.1 s, writing the call 0.1 to 0.5 s),
the read (the operator's GitHub, 0.2 to 1.6 s), the fast model's answer (0.5 s plus its thinking,
0.2 to 1.1 s), and the voice's first clause (0.3 to 0.8 s): about 3.5 to 6 s. On OpenRouter add
about 0.5 s to each call's first byte. **About 3 s is not reachable with this architecture on a
mid-speed provider without giving up the fast model's thinking**, which this round measured as
costing accuracy on exactly the turns that matter (a change, a lookup's choice of what to read).
The first sound at 1.0 to 1.3 s is reachable on a quick connection (1.30 s on the bench) and not
on OpenRouter (1.7 to 2.3 s live), because it is a fixed word placed against the connection's
measured first word, after a close of 0.6 to 1.4 s.

What would reach both is **a realtime speech-to-speech model in front** (OpenAI's Realtime API,
Gemini Live, or a comparable one), as OpenClaw's voice does: it hears the operator's audio as it
comes, speaks within a few hundred milliseconds of the end of speech, handles small talk itself,
and hands every real request to Ghost's agent through one tool (a consult), saying something
natural while the agent works. What it would cost and change:

- **Privacy**: the operator's voice, not only the words, would leave the machine after the wake
  for the length of the session, to a second provider. Idle audio would still stay local (the
  wake stays Ghost's), but ADR 0010's line moves.
- **The gate**: unchanged if the realtime model has only the consult tool: every read and change
  is still the agent's, through the dispatcher and the gate. Its own words would be a new kind of
  text spoken as Ghost's, needing the same rules about app text it never sees.
- **Money**: realtime audio is priced per minute of audio in and out, several times a text turn's
  cost, and billed while the session is open.
- **What Ghost loses**: its own turn-taking (Smart Turn, the graces, barge-in rules), Parakeet and
  Kokoro on the machine, and the transcript as the turn's truth; the realtime model's own
  turn detection and voice replace them.

That is the owner's decision; it is not built.
