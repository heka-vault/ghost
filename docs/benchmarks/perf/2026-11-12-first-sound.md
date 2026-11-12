# Perf: the first sound, on a real-size prompt and a slow provider (2026-11-12)

Branch `fix/first-sound`, after [the reply-speed round](2026-11-12-reply-speed.md) (PR #79), which
brought a lookup's first sound to about 2 s on the bench. The target was 1.0 to 1.3 s from the end of
the operator's speech on a good connection, and never a long silence on a slow one. The owner's rule
for this round: Ghost works as well as it can on whatever provider the operator configured; a fast
model whose first token takes 1.5 s (Haiku 5.5 on OpenRouter, from the operator's records) is a fact
to design for.

## What the bench did not see

The harness's prompt is about 4,000 tokens of system and tools. The operator's turns read about
18,000 from the cache (`cache_read` in his `latency.jsonl`). And his provider is slower than the
proxy the bench uses: the first model call's first byte on his last turns, p50 (p90), was Haiku 5.5
1.93 s (2.69 s, n=14) and Sonnet 5.5 1.53 s (2.49 s, n=17) this month. Two harness options now stand
for both (`ghost_voice::perf::shaped`):

- `--pad-tokens N` (`pad=` in a variant): about N tokens of fixed synthetic reference appended to
  the system prompt, inside the cached prefix, the same on every call. 11,000 here, for about 15,000
  in all.
- `--first-byte-ms N` (`delay=`): every streamed model call waits N ms before it is sent, both models.

The padding alone moved the picture: with it, the fast model's first call took 1.47 to 1.86 s to its
first byte through the proxy (p50 of 15), against 1.1 to 1.2 s without it and the 0.5 to 0.65 s
measured earlier for small talk on the bare prompt. So the bench's "good connection" with a real-size
prompt is already the slow case.

## Root causes

1. **The acknowledgment waited out the full grace whatever the close was**
   (`crates/ghost-voice/src/acks.rs`, `settle_wait`, on main: `settle - segment_ended.elapsed()`
   with `settle` = `GHOST_TURN_GRACE_SECS`). A held answer's frames reach the acknowledgment stage
   only once the turn has closed (`LlmStage::consume` drains a speculation's events at
   `SpeculationConfirmed`), so after a confident close at 0.35 s a call that started 0.5 s after
   the end of speech still waited until 1.2 s: up to 0.85 s of nothing, protecting nothing the close
   had not.
2. **Nothing was said while the model was silent.** The acknowledgment fires on a call's start or
   a hand-over; a plain answer, or a model slow to begin anything, left silence. The thinking sound
   was turned off on 2026-10-30 (it leaks through the echo canceller and the operator found it
   unpleasant), and ADR 0049 recorded that "nothing fills it yet".
3. **The wrong label on the triage lookup.** In every run, before, both models called
   `github_list_issues(labels=["triage"])`, got an empty list and said nothing was waiting (the
   app's label is `needs-triage`). The model had never seen the label names and guessed one; an
   empty result under a guessed filter looked like an answer. Not the fixture: a real GitHub answers
   the same way.
4. **Two GitHub connections.** The prompt said in general to read "through one of them" when two
   apps reach the same repository; it named neither app, and the model read through both in 1 of 9
   runs (2 of 6 in the last round).

## What was changed

- **The acknowledgment follows the close the turn got.** Once the turn has closed it is said at
  once; only a frame seen before the close (none today) still waits out the grace. Going on still
  cuts the turn and the word (a cut turn says nothing, even mid-render), barge-in stops it, a goodbye
  or a hold gets none, and nothing is said once any of the answer has been. The tool call itself
  still waits the full 1.2 s (ADR 0049), unchanged.
- **The first sound** (`ghost_voice::acks::FirstSound`). When a turn's reply has begun and, a set
  time after, the model has written no word and started no call, Ghost says "One sec." or "Let's
  see.", once, from the clips already rendered for the acknowledgment. The model's stage now says
  when it has written its first word (`Frame::ModelWriting`), even while the fast model's reply is
  held whole, so an answer on its way is never preceded by it, and a call's own acknowledgment is
  not doubled.
- **When, per connection.** The daemon gives the session the connection's usual first word (the
  median first-call time of the operator's last ten spoken turns, read from the last 64 KB of
  `latency.jsonl`, else the latest model check of the route that answers first). On a quick
  connection (0.75 s or less) the word waits 0.25 s past it, so it is heard only on a turn slow for
  that connection. On a slow one it is said 0.8 s ahead of it: early, so it lands well before the
  answer rather than a moment before it. Always between 0.7 and 1.4 s after the reply begins.
  `GHOST_FIRST_SOUND_SECS` sets it; 0 turns it off.
- **The label:** the prompt says never to filter on a name it has not seen, but to list without it
  and pick out what matches.
- **Two connections** (`ghost_core::apps::twins`): two connected apps that reach the same service
  (a URL's host, or the program a local app runs) with the same tools by the app's own names are
  twins, and the later one's paragraph of the prompt says so, naming both: read one thing through
  one of them, the one that reached it already, else the first. The general sentence is gone.

## How the first sound's wait was chosen

The first round used "the connection's first word plus 0.25 s, from 0.7 to 1.4 s", and set the
waits for the bench's assumed first word (0.55 s): 0.8, 1.3 and 1.4 s at 0, +0.5 and +1.0 s of
added delay. The word fired on every turn, small talk included, because the padded prompt's real
first word was 1.5 to 2.4 s. At 0.8 s after the reply began it came 0.94 s before a plain answer on
small talk (p50: the word at 1.41 s, the reply at 2.35 s). Placed by the first rule at that
connection's own first word it would have come about 0.35 s before the answer: "One sec. Doing
well." So on a slow connection the word belongs early, and only on a quick one late. The rule was
changed to that, and the second round measured it.

How the leaders do it: OpenClaw's realtime voice speaks one brief acknowledgment when its consult
tool starts (`session-config.ts`, "speak one brief acknowledgement such as 'Let me check that for
you'"), which Ghost already does at a call's start; Hermes delegates every real request the same
way. Neither has a timed first sound; both front a realtime model whose first audio is quick. Ghost's
front model is a text model on whatever connection the operator chose, which is why the wait is
measured per connection here.

## Before and after

`ghostd perf run --no-jev --runs 3 --scenario small_talk,lookup,chain,by_number,no_app --llm-url
$ANTHROPIC_BASE_URL --llm-model claude-sonnet-5-5 --llm-fast-model claude-haiku-5-5 --llm-key-env
ANTHROPIC_AUTH_TOKEN --fast real`, variants `apps=deferred[+pad=11000][+delay=500|1000]`; *after*
adds `first_sound_secs=` at what the rule gives for the first word measured at each setting (light
1.12 s → 0.85, d0 1.47 s → 0.7, d500 1.88 s → 1.08, d1000 2.37 s → 1.4; light's 0.85 was set
before its measure, which gives 0.7). *Before* is main (`0f1f8274`) with only the harness options
added, built separately (`9e09ede9`). Release builds; real models through the local proxy, Haiku 5.5
in front with its own reads. Seconds from the end of the operator's speech, p50 of 3: first sound
(the acknowledgment, the first sound or the reply, whichever came first) / reply.

| scenario | light: before | light: after | +pad: before | +pad: after | +pad +0.5 s: before | after | +pad +1.0 s: before | after |
|---|---|---|---|---|---|---|---|---|
| small talk | 1.50 / 1.50 | 2.08 / 2.08 | 2.52 / 2.52 | **1.32** / 2.07 | 3.69 / 3.69 | **1.69** / 2.56 | 3.57 / 3.57 | **2.01** / 3.08 |
| what's waiting for triage | 1.95 / 5.83 | **1.46** / 5.21 | 3.03 / 6.54 | **1.31** / 5.07 | 2.98 / 10.45 | **1.70** / 12.52 | 3.45 / 11.08 | **2.02** / 11.87 |
| an issue and its comments | 2.02 / 5.07 | **1.51** / 4.95 | 2.99 / 6.90 | **1.33** / 5.63 | 5.23 / 9.01 | **1.74** / 6.58 | 3.80 / 8.60 | **2.06** / 8.18 |
| an issue by its number | 1.98 / 7.11 | 2.65 / 6.27 | 2.44 / 7.14 | 2.45 / 6.90 | 2.70 / 8.35 | 2.73 / 15.92 | 3.36 / 9.92 | 3.05 / 9.54 |
| a question no app answers | 2.55 / 2.55 | **1.45** / 2.44 | 3.10 / 3.10 | **1.30** / 2.76 | 3.38 / 3.38 | **1.68** / 3.48 | 3.40 / 3.40 | **2.01** / 5.08 |

Load average: about 20 falling to 6 during the padded *before* runs (other agents' builds), 8 to 5
for light *before*, 1.5 to 3.4 for *after*. The padded *before* replies are slower partly for that
(the read and the voice share the machine). The first sound is not: it is the wait above plus the
close, and holds whatever the load.

- **A first sound in 1.3 to 1.5 s on every turn but the issue by number, at every setting, and
  never later than 2.06 s with a full second added to every first byte** (before: 2.4 to 5.2 s).
  With the prompt the operator's size and no delay it is 1.30 to 1.33 s.
- **Plain replies:** the word was said before small talk in 2 of 3 runs at d0 and d500 and in 3 of
  3 at d1000, 0.75 s or more ahead of the answer (d0: the word at 1.32 s, the reply at 2.07 s); not
  at all on the light setting, where the model was quick enough. The reply itself was no later for
  it.
- **Light small talk** went from 1.50 to 2.08 s with no word said: the fast model's first byte was
  0.49 to 0.57 s in the *before* runs and 0.73 to 0.97 s in the *after* ones, the close (0.35 s) and
  the read (0.18 s) the same. That is the provider, not this change, which adds no step to a plain
  turn.
- **The issue by number** closes on the number rule (ADR 0049: 1.7 s after a turn that ends on a
  number), so its first sound comes at the close plus the wait: 2.0 to 3.1 s. Unchanged here; it is
  ADR 0049's number wait.
- **The triage lookup** now reads the right issues: 0 of 12 runs filtered on a guessed label
  (before: 12 of 12 used `triage` and answered "nothing is waiting"), 9 of 12 listed each
  repository and answered with the three `needs-triage` issues, and 3 of 12 read Ghost's own waiting
  list only and asked "Do you mean GitHub issues?" (the same reading of "waiting" appeared before,
  hidden by the wrong label). The harness's check still counts all 12 as a miss: it looks for a
  `needs-triage` filter, and the model now lists and picks out instead. The first wording ("an empty
  filtered list is not an answer: look again") fixed the answer a round later (lookup reply 6.5 to
  10.6 s at d0); the second, "never filter on a name you have not seen", saves that round: 5.07 s.
- **Two connections:** 0 of 21 by-number runs after read through the second connection, 1 of 12
  before. In 12 of 21 the model read issue 142 in both repositories through one connection, at once:
  a number with no repository, read where it might be, which is "look first" working.
- **Full replies at +0.5 and +1.0 s** vary by several seconds run to run (4 to 5 model calls in some
  lookups, before and after): three runs are too few to call those a change either way.

## Against the target

On a good connection with a real-size prompt (the proxy, no delay), the first sound is **1.30 to
1.33 s**: at the top of the 1.0 to 1.3 s target for everything but the issue by number. On a slow one
it is at most 2.06 s with a second added to every first byte, against 3.4 to 3.8 s before. What is
left, on the padded d0 runs:

- the close: the VAD's 0.2 s, the read (0.17 s), and the short grace (0.35 s), about 0.6 s;
- the first sound's floor, 0.7 s after the reply begins.

Lowering the floor to bring the word to 1.0 s would say it before answers that come at 1.1 to
1.5 s on a quick connection, the "One sec." before a one-word answer this round set out to avoid.
What would remove the time is the model: the fast model's first byte with the real-size prompt is
1.1 to 1.9 s through the proxy, and a word is only a placeholder for it. Three things would help:
the read starting at the VAD's first quiet (`GHOST_STT_EARLY_READ=speculate`, ADR 0054, off; 0.17
s), a fast model on a faster route for the operator's connection (the slow-model note already says
so, ADR 0056), and a smaller prompt for the fast model's first call (its tools and the apps'
paragraphs are most of the 15,000 tokens, and it reads all of them before its first byte).

The acknowledgment following the close made no difference in these runs: the model's first call
never started before the 1.2 s grace had run (its first byte was 1.1 s or more after the request).
It removes up to 0.85 s when a quick model begins a call in under a second after a confident close,
which the bench's scripted profiles and a quick provider do; it is tested at the stage
(`acks::tests::after_a_close_the_acknowledgment_waits_no_more_grace`), not measured here.

## Costs counted (ADR 0018)

- **Every turn:** the system prompt template is 28 bytes longer (3,124 to 3,152, about 7 tokens),
  read from the cache. With two connections to the same service, the second's paragraph is 284
  characters longer (about 70 tokens; the operator's setup has one such pair), also cached.
- **Every reply:** one frame (`ModelWriting`) when the model writes its first word. No render, no
  I/O.
- **A turn that hears the first sound:** a cached clip played, as an acknowledgment is; no
  synthesis on the turn's path. It is not a turn: not in the transcript or the context.
- **A session opening:** the last 64 KB of `latency.jsonl` read and parsed, 0.09 ms in release on
  a 5 MB file (`latency::tests::reading_the_tail_costs_the_same_however_long_the_file`), and when
  there are fewer than five spoken turns, the latest model check from SQLite.
- **Tokens:** none for the first sound or the acknowledgment. The label line saves a model call on
  a lookup that would have guessed.

## What is not measured here

- OpenRouter itself; the delay option stands for it, and the operator's own records say how far.
- The operator's ear: whether "One sec." 0.75 s before a plain answer on a slow connection sounds
  better than the silence (rubric part D). `GHOST_FIRST_SOUND_SECS=0` turns it off.
- A goodbye only the model recognizes on a slow connection: if its call starts after the wait,
  the word is said before the goodbye. The quick intents catch most goodbyes before the model.
