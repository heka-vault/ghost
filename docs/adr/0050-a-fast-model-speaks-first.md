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

## Amended 2026-11-12: the fast model reads for itself, measured with the real models

The first numbers above came from a scripted model. The second round measured the real
ones, through the default connection with Ghost's prompt and tools
([the benchmark, second section](../benchmarks/perf/2026-11-12-voice-speed.md#second-round-real-models-first-audio-sooner-the-fast-model-reads)),
and found two things.

**The fast model never answered.** Haiku 5.5 refuses the session's thinking setting
(`between_tools`) with HTTP 400, so with it as the fast model every turn failed in 0.3 s
and was handed over. The fast model is now asked without the thinking setting
(`ghost_voice::llm::fast_options`); a small model answering small talk needs none.

**The hand-over costs more than assumed.** A real Haiku 5.5 takes 1.1 s to write a tool
call, against 0.5 s to its first words, so a turn that goes to an app paid about 1.5 s for
the fast model to ask, not 0.67 s: a lookup 5.1 s with the main model alone, 6.6 s with a
fast model handing it over.

So the first rejected option below is taken, behind a setting:

**The fast model may make reads itself** (`GHOST_LLM_FAST_READS`). A reply of the fast
model's whose calls are all reads (an app's read tools, Ghost's own reads;
`ToolDispatcher::is_read`) is the turn's reply, as the main model's would be: its calls
run through the dispatcher as any read does, after the turn's grace has run
(`TurnControl::settles`, ADR 0049), and the answer after their results is the fast
model's too while the results are no longer than `GHOST_LLM_FAST_READ_MAX_CHARS` (4,000
characters) and it has made no more than `GHOST_LLM_FAST_READ_ROUNDS` (2) rounds of reads.
It hands over, as before, for a change of any kind, for `think_harder`, for `hang_up` and
the session's other tools, for anything after a longer result, and for a read past its
rounds. A held answer keeps the fast model's reads for the confirmed turn instead of
withdrawing and asking again. What reaches the gate is still always the main model's:
a change is never the fast model's to make.

Measured with the real models, two runs a scenario on the mock app, p50 end of speech to
Ghost's voice: a lookup 5.08 s main alone, 6.59 s handed over, 3.19 s read by the fast
model; an issue and its comments 7.53, 8.47 and 3.93 s; plain speech 2.5, 1.5 to 2.0 and
1.6 to 1.9 s. The fast model made the same calls as the main model in every lookup run
(the same tools, the same count, the same repository and issue numbers where the harness
checks them; the label check failed for both models alike), and every change still went
through the main model. With so few runs that is no proof of equal judgement; the setting
is on by default only because the fast model itself is off by default, and the voice
rubric's part D is what confirms it.

## Amended 2026-11-12: something is said within the grace of a lookup, and the cache is warm

Live, on the operator's setup (OpenRouter, four apps past the tool limit), four turns that
looked something up went four to six seconds with only the thinking sound: the
acknowledgment above fired at a hand-over or at a call to an outside app, and those turns
went to Ghost's own tools or were the fast model's own reads, neither of which is a
hand-over. [The benchmark](../benchmarks/perf/2026-11-12-reply-speed.md) has the evidence
and the numbers.

- **A call is acknowledged as it starts** (`ghost_llm::Event::ToolStarting`), for any tool
  that sends the model round again, Ghost's own included, after the turn's grace has run
  from the end of the operator's speech as before. A hand-over is still acknowledged the
  same way, once.
- **The fast model's words before its first call may be what is said.** "Nothing it wrote
  is spoken" holds for its reply's text: what it writes before a turn's first call, eight
  words or fewer and not before `think_harder`, is rendered and played as the
  acknowledgment (not a turn, not in the transcript, not in the context when it handed
  over). The same holds for the main model's words before its first call.
- **Each model's prompt cache is written as a session opens** (`GHOST_LLM_WARM`), while the
  greeting plays, unless the same prefix was sent in the last four minutes.
- **The fast model's thinking is its own** (`GHOST_LLM_FAST_THINKING`). The amendment
  above said it is asked "without the thinking setting"; left out, Haiku 5.5 thinks
  adaptively. Turning it off began its calls 0.3 s sooner and made it look less far (a
  triage question answered from Ghost's own status in 3 of 3 runs), so it is left out by
  default and `disabled` is the operator's choice.

Measured with the real models on the harness's deferred apps: a lookup's first sound
3.4 to 3.9 s before, 2.0 to 2.7 s after; a question no app answers 3.8 s in three calls
before, 2.4 s in one after. The floor is the grace the acknowledgment waits (1.2 s from
the end of speech), then the model's first sign of a call.

## Amended 2026-11-12: a first sound per connection, and the acknowledgment follows the close

The bench now pads the prompt to the operator's size and can add a slow provider's first byte
([the benchmark](../benchmarks/perf/2026-11-12-first-sound.md); `perf::shaped`). With about 15,000
tokens of prompt the fast model's first byte was 1.1 to 1.9 s through the proxy, and 1.5 to 1.9 s on
the operator's OpenRouter records. A turn that answered in words, or a model slow to begin anything,
left that silence, and the acknowledgment of a call waited out the full grace after any close.

- **The acknowledgment follows the close the turn got** (ADR 0049's amendment): "waits until the
  turn's grace has run from the end of their speech" above now holds only for a frame seen before
  the turn closed.
- **A first sound** (`ghost_voice::acks::FirstSound`): when a turn's reply has begun and the model
  has written no word (`Frame::ModelWriting`, sent even while the fast model's reply is held) and
  started no call within a wait, Ghost says "One sec." or "Let's see." once, from the
  acknowledgment's rendered clips. Not before an answer whose first word is written, not for a
  goodbye or a hold, never twice with a call's acknowledgment, never once any of the answer was
  said, and not if the turn is cut.
- **The wait is the connection's** (`FirstSound::for_connection`), from its usual first word (the
  median of the operator's last ten spoken turns, else its latest check, ADR 0056): on a quick
  connection (0.75 s or less) 0.25 s past it, so only a slow turn hears it; on a slow one 0.8 s
  ahead of it, so it lands well before the answer and never a moment before it; between 0.7 and
  1.4 s after the reply begins. Measured first with a later word on slow connections, it would have
  come about 0.35 s before a plain answer. `GHOST_FIRST_SOUND_SECS` sets it, 0 turns it off.
- **Words, not the thinking sound.** The thinking sound is off for leaking through the echo
  canceller (2026-10-30). The words are two of the acknowledgment's own, which the echo filter
  already knows.

Measured with the real models, the prompt padded to the operator's size: the first sound of every
turn but an issue asked for by number came at 1.30 to 1.33 s, against 2.4 to 3.1 s before, and at
1.68 to 2.06 s with 0.5 or 1.0 s added to every first byte, against 2.7 to 5.2 s. The word came
before small talk in most padded runs, 0.75 s or more ahead of the answer, and not when the model
was quick. Whether that sounds better than silence is the voice rubric's part D.

## Amended 2026-11-12: the main model is asked beside the fast one

[ADR 0060](0060-the-main-model-is-asked-beside-the-fast-one.md): every call the fast model
takes also goes to the main model (`GHOST_LLM_RACE`, on), whose reply waits and is the
turn's when the fast model hands over, and is ended unread otherwise. "A turn that needs a
tool pays for the fast model's call before the main model starts" no longer holds: a change
through the real models went from 9.26 to 7.20 s at the p50. The fast model's thinking stays
left out by default, measured again ([the benchmark](../benchmarks/perf/2026-11-12-reply-time.md)).

## Considered options

- **The fast model's own tool calls run.** A read is safe to run (it is called directly,
  ADR 0034) and the main model would still write the answer, which would take a model
  call off every lookup instead of adding one. But a wrong repository or a wrong number
  from the small model becomes the main model's premise, and for a change the small model
  would be what reaches the gate. Hand-over keeps one rule: the main model does the work.
  Worth measuring with a real model before deciding again. Taken for reads on 2026-11-12
  (above); a change still goes to the main model.
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
