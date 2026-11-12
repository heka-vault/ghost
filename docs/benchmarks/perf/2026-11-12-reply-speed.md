# Perf: why real use was slower than the bench, and the first sound of a lookup (2026-11-12)

Branch `fix/reply-speed-2026-11-12`. The third round of the day
([2026-11-12-voice-speed.md](2026-11-12-voice-speed.md)) measured 1.69 s for a plain reply
and 3.57 s after a lookup, with a fast model in front, through the local proxy. That
morning's live run on the operator's setup (OpenRouter, Haiku 5.5 in front of Sonnet 5.5,
four apps connected) gave 4.6 to 6.3 s for the same kinds of question, and up to five
seconds with only the thinking sound. This round found why and fixed what Ghost owns.

## What the live run showed

From the debug log and `latency.jsonl` of the run (15:49 to 16:02; the operator's words
are not quoted here):

| turn | what happened | reply | model calls | cache read |
|---|---|---|---|---|
| a calendar question | the fast model searched the tools (result 8,071 characters, over its 4,000), the main model narrated (dropped), read Ghost's docs, then said no calendar is connected | 6.30 s | 3 | 37% |
| an issue by number | tool search (11,729 characters), the main model narrated (dropped) and read the issue; an acknowledgment came at 5.46 s | 10.29 s (cut off) | 3 | 37% |
| what changed yesterday | the fast model's own `ghost_recall`, no acknowledgment | 5.69 s | 2 | 91% |
| an issue by number, short repository name | three turns of "which repository?", then the same read on both GitHub connections | 5.51, 6.99 s | 2 each | 98 to 99% |

## Root causes

1. **No acknowledgment before most lookups.** It was said at a call to an *outside* app
   (`acks.rs`, `acknowledge`: Ghost's own tools gave `None`) or at a hand-over. Those turns
   went to Ghost's own tools (`ghost_tools_search`, `ghost_recall`, `ghost_docs_search`) or
   were the fast model's own reads (ADR 0050's `GHOST_LLM_FAST_READS`), which are not a
   hand-over. The one acknowledged came at the main model's call, 5.46 s in. The model's
   own words before its call were always dropped (`narration.rs`).
2. **A tool search on most lookups, and a cold cache after it.** Past the 40-tool limit
   (the operator's four apps have 106 tools) the read tools are hidden. What the model used
   or found was kept in memory only, so every restart searched again. Each search offered
   up to eight more tools by name ("19 to 27 tools", "27 to 31"): the tool list and the
   apps' paragraphs of the prompt changed mid-session, which rewrites the cached prompt
   from its start (tools come first). That is the 37%.
3. **A cold cache at the start of a session**, when the daemon was new, when a search had
   changed the tool list, or when the main model had not been called for five minutes
   because the fast model answered the plain turns.
4. **The fast model asked instead of looking**: the prompt said to ask a short question
   when unsure, and nothing said to look first.
5. **Three calls to say no calendar is connected**: Ghost's own paragraph said to check the
   tool search and the docs "before saying Ghost can't do something".
6. **Why the bench did not see it**: the harness connected the mock GitHub app alone and
   never Ghost's own app, so nothing was ever hidden and no turn searched. And the proxy
   keeps cache entries an hour (it answered `ephemeral_1h_input_tokens` to a 5-minute
   marker), so a bench rarely starts cold.

## What was changed

- **A call announced as it starts** (`ghost_llm::Event::ToolStarting`, at the tool block's
  start, before its arguments are written), passed on as `Frame::ToolStarting`, with the
  fast model's held words.
- **The acknowledgment at that frame, for any tool that sends the model round again**
  (Ghost's own lookups too). It says the model's own words before its first call when it
  wrote eight or fewer, rendered then, or else one of the fixed lines. As before it waits
  until the turn's grace has run from the end of the operator's speech and is dropped if
  they go on, plays as an acknowledgment (not a turn, not in the transcript), stops on
  barge-in, is never said for a goodbye or a hold, and never once any of the reply has
  been spoken. Every later round's narration is still dropped.
- **The prompt cache written as a session opens** (`Frame::WarmCache`, `GHOST_LLM_WARM`,
  on): each model's first call's prefix (tools, system prompt, history, thinking setting)
  with `max_tokens: 0`, not streamed, so the provider writes the cache and answers
  nothing, unless the same prefix was sent in the last four minutes. A provider that
  refuses no output is asked for one token. Checked through the proxy: `content: []`,
  `stop_reason: max_tokens`, the cache written, and the first real call reading it
  (the harness log: "warmed claude-haiku-5-5's prompt cache in 0.53s (1798 tokens
  written, 3134 read)").
- **The tools used or found kept in SQLite** (`recent_tools`, `Apps::remember`) and
  restored at start; **a search offers its best three by name**, not all eight (the rest
  stay callable by the name it gave).
- **The prompt**: look first when a lookup can settle it, finding the tool when it is not
  listed, and ask only when the results leave more than one answer; read through one
  connection when two reach the same repository; when what is asked needs a kind of
  service none of the connected apps is, say so at once; the words before a turn's first
  lookup may show the operator was heard. Ghost's own paragraph now says to check the
  search and the docs before saying Ghost *itself* can't do something.
- **The harness**: `--apps deferred` (two GitHub connections, the mock twice with its own
  state each, Ghost's own app, a live tool list, the tool limit set under the mocks' ten
  so they are hidden as the operator's 106 are) and `--apps deferred-kept` (the lookups'
  reads kept, as a daemon started again finds them); `apps=` in a variant. Two scenarios
  outside the default set: `no_app` (a question no connected app answers) and `by_number`
  (an issue asked for by its number alone).

## Before and after

`ghostd perf run --no-jev --runs 3 --scenario small_talk,lookup,chain,by_number,no_app
--llm-url $ANTHROPIC_BASE_URL --llm-model claude-sonnet-5-5 --llm-fast-model
claude-haiku-5-5 --llm-key-env ANTHROPIC_AUTH_TOKEN --fast real --variant
NAME:apps=deferred[-kept]`: the real models through the local proxy, Haiku 5.5 in front,
with its own reads (as live). *Before* is main (`3c89efeb`) with only the harness's profile
added, built separately; *after* is this branch. Load average 1 to 3 for the before run
and 2 to 10 for the after run (other agents' builds). Seconds from the end of the
operator's speech, p50 of 3; *first sound* is the acknowledgment or the reply, whichever
came first.

| scenario | before: first sound / reply / calls | after, nothing kept | after, the reads kept (a restart) |
|---|---|---|---|
| small talk | 1.52 / 1.52 / 1 | 1.51 / 1.51 / 1 | 1.67 / 1.67 / 1 |
| what's waiting for triage | 3.86 / 5.35 / 3 to 4 | 1.96 / 5.02 / 3 | 2.74 / 5.11 / 2 to 3 |
| an issue and its comments | 3.39 / 4.99 / 3 | 2.22 / 5.04 / 3 | 2.16 / 4.01 / 2 |
| an issue by its number alone | 2.58 / 2.58 / 1, **read in 0 of 3** (it asked) | 1.98 / 6.20 / 3, read in 3 of 3 | 2.56 / 4.68 / 2, read in 3 of 3 |
| a question no app answers | 3.77 / 3.77 / 3 | 2.36 / 2.36 / 1 | 2.41 / 2.41 / 1 |

- The first sound of a lookup came 1.4 to 2.0 s sooner, and was there in every lookup
  run (before: none on the no-app question and on the reads by number).
- A read the model had used before saves a model call (the search) and about a second
  of reply: an issue and its comments 5.04 to 4.01 s, by number 6.20 to 4.68 s.
- The no-app question: one call instead of three, 3.77 to 2.36 s.
- An issue by its number alone is now read instead of asked about. It costs the read: the
  reply comes later than the question did, and it is the answer.
- The triage lookup's label (`triage` against the mock's `needs-triage`) is wrong before and
  after, for both models, as the earlier round found; and in 2 of 6 runs of the by-number
  read the model read through both GitHub connections at once despite the new line.
- The cache: every turn's first call after the opening read the system prompt and tools
  from the cache (58 to 100% of its input; the rest is the turn itself). Through the proxy
  that was true before too, because the proxy keeps entries an hour.

### Still above the target

A lookup's first sound is about 2 s from the end of speech; the target was 1.0 to 1.3 s.
Where it goes, from the records: the close, 0.6 s (the VAD's stop, the read, the short
grace); then the acknowledgment waits until the full grace (1.2 s) has run from the end of
speech before it may be said, because the operator may only be pausing (ADR 0049); then
the fast model's first sign of a call. Haiku 5.5 began its tool calls 1.0 to 1.8 s after the
request in these runs (through the proxy, under load). So the floor of this design is the 1.2 s grace, and it is met
when the model is quick; what is left beyond it is the model. Lowering the wait for a
confident close would bring the word to about 1 s; it is the same question as ADR 0049's
call wait and is left to that ADR.

### The fast model's thinking

Haiku 5.5 thinks adaptively when the request leaves the field out, as Ghost's did, unseen
(the latency record says `thinking: false`). Through the proxy, nine requests that needed a
tool: left out, the tool block began at 1.17 s (p50), and with `thinking: disabled` at
0.84 s, after a few words ("I'll read issue 142 from the app repo."). In the harness,
three runs each: with it off, the triage lookup read Ghost's own status and stopped in 3 of
3 runs and an issue by number was missed in 1 of 3 (a lookup's first sound 1.95 s, its
reply 3.4 s); with its thinking it went on to the issues in 3 of 3 and read the issue in 3
of 3 (2.1 to 2.7 s, 7.3 to 7.9 s). Right later beats wrong sooner, so it is left out by
default; `GHOST_LLM_FAST_THINKING=disabled` is there for the operator to choose.

## OpenRouter against the proxy

From the operator's `latency.jsonl` (every turn through OpenRouter), the first call's first
token when the prompt was read from the cache: Haiku 5.5 1.53 s p50 (n=15, plain replies),
Sonnet 5.5 1.21 s (n=293). Through the proxy, the same day: Haiku 5.5 0.47 to 0.65 s (the
harness's small-talk replies, n=12), Sonnet 5.5 1.7 s in the second round's `perf llm`. So
OpenRouter adds about a second to Haiku 5.5's first token and nothing to Sonnet 5.5's.
Which provider OpenRouter routes Haiku 5.5 to was not checked: the key is in the vault,
which this round did not read. A first token of about 1.5 s on every fast-model turn is
the largest single gap left between the bench and real use; the warm-up does not change
it.

## Costs counted

- **The warm-up**: one request per model per session opening, unless the same prefix was
  sent in the last four minutes. Unused (the operator says nothing), it is a cache write of
  the whole prefix: about 15,000 tokens on the operator's setup, at 1.25 times the input
  price for the five-minute cache: about $0.04 for Sonnet 5.5 and $0.002 for Haiku 5.5. A
  proxy that keeps entries an hour bills them at twice. Used, it is the write the first
  turn would have made, and the first turn reads.
- **The acknowledgment in the model's words**: one Kokoro render of up to eight words
  (0.25 to 0.4 s on this machine) off the turn's path, while the call is being written.
- **Every turn**: the system prompt is 550 characters longer (2,574 to 3,124), about 135
  tokens, read from the cache. The tools offered per turn are the same at most (the
  pinned and twelve kept); a search now adds three, not eight.
- **SQLite**: a write of the kept list (twelve names) when it changes, on a call.

## What is not measured here

- OpenRouter itself, and the operator's real apps (their schemas are larger than the mock's
  and the cache prefix is about 15,000 tokens against the harness's 4,000).
- The operator's ear: whether the model's own words before a lookup sound better than
  "Sure." (rubric part D).
