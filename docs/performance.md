# Performance is a feature

Ghost is a voice. A reply that starts two seconds late feels broken, however good its
words are. Talking to Ghost, from the desk, the phone or the desktop app, should be as quick
as the work allows, and every release makes it quicker or says why it couldn't
([ADR 0018](adr/0018-performance-is-a-feature.md)).

Ghost is pre-1.0. Most changes today are fixes to make the proof of concept work. That is
why the budgets below are measured from the start, not added at the end.

## What we measure

One user turn, from the moment the operator stops speaking to the moment Ghost's voice
starts, broken into stages:

| Stage | From → to |
|---|---|
| hear | end of speech → final transcript (VAD stop, STT) |
| hold | final transcript → turn closed (Smart Turn, the turn grace) |
| intercept | quick intents, the gate's request check, Jev or regex on a yes |
| model | each model call: request → first token, and → last token; tokens in, out, cached |
| tools | each tool call by name, including the app behind it (a Mobbin search through the fleet proxy is 3 to 9 s, all of it the app's). A call may take up to its timeout, 300 s by default (ADR 0046); the operator can talk over it or hang up, and it is stopped |
| write | the gate: the level, the call, the audit |
| voice | text to Ghost → first audio out → Ghost heard (bot started speaking) |
| transport | phone only: WebRTC each way |

Outside a turn: Start on the phone → greeting heard; pairing; the phone's `/hello` and
`/audit`; control API calls from the desktop app; daemon start → models ready.
Apps connect at daemon start (ADR 0034), so a transfer starts nothing. What an app costs on
every turn is its tools in the prompt: measure `make perf` after connecting one, and
narrow or block what it costs too much ([apps.md](apps.md#performance)).

Every live turn writes its timeline, timings only and never text, to
`~/.local/share/ghost/latency.jsonl`, with one line in the daemon log. `make latency`
summarises it. `make perf` (`ghostd perf run`, `crates/ghost-voice/src/perf/`) drives scripted
conversations through the real pipeline without a person, the models real and the model's
replies scripted, and reports each stage's p50 and p95; `make perf-micro` times the models one
by one and `make perf-llm` the real model connection with Ghost's prompt and tools.
`make perf-providers` runs the provider check the product runs (Settings › Model's Test,
`ghostd llm test`; ADR 0055) against every provider named that the environment has a key for,
keys from the variables named on the command line and never the vault, and writes a dated
comparison here (`docs/benchmarks/perf/<date>-providers.md`): `PROVIDERS="proxy
anthropic:ANTHROPIC_API_KEY openrouter:OPENROUTER_API_KEY openrouter-openai"`. The model's
first word is the one wait Ghost can't hide: past 1.5 s, by the latest check or by the
operator's last ten spoken turns, the waiting list says so and what would help. The
fixtures the models are checked against come from the oracle (`tools/oracle`,
`scripts/fixtures/record.py`; `crates/ghost-infer/tests/parity.rs`), the Python wrappers the
Rust was measured against (ADR 0039).

How well Ghost hears is measured the same way: `make perf-hear` runs the operator's
recorded takes, or Kokoro's, through the real speech-in stage and scores the words
([benchmarks/hear-harness.md](benchmarks/hear-harness.md)). A change made to hear better
passes its rule; a change made for speed shows there that it heard no worse.

## Budgets

These are the starting targets for p50 / p95 on the workstation over home Wi-Fi. The first
`make perf` baseline replaces them with measured numbers, and from then on they only move down.

| Path | p50 | p95 |
|---|---|---|
| Answer with no tool, end of speech → voice | 1.5 s | 2.5 s |
| Answer after one lookup | 2.5 s | 4.0 s |
| Quick intent (hang up, cancel) → voice | 0.8 s | 1.2 s |
| A write asked for plainly → done and spoken | 2.5 s | 4.0 s |
| Phone transport, one way | 0.15 s | 0.3 s |
| Phone Start → greeting heard | 2.0 s | 3.0 s |
| Phone `/hello`, control API reads | 0.1 s | 0.3 s |

Where the harness stands against the first two (2026-11-12, the scripted model taking a
real model's assumed time, [the report](benchmarks/perf/2026-11-12-voice-speed.md)): a
plain reply is 2.4 s at the p50 with the main model's time alone, where the model
and no longer the turn's grace is the wait, and 1.66 s with a fast model in front
(ADR 0050) or 1.48 s when the one model is itself that quick; an answer after one
lookup is 4.18 s, 4.93 s with the fast model's hand-over in front of it. So
the plain reply's budget is met only with a fast model, and the lookup's by neither. Both
budgets stay; the next target for a plain reply is 1.0 s, and what stands between is
listed in that report.

With the real models (2026-11-12, second round, the same report's second section; the
harness driving Sonnet 5.5 and Haiku 5.5 through the default connection): a plain reply
is 2.5 s at the p50 with the main model alone, whose first words take 1.6 to 1.8 s
through Ghost's pipeline, and 1.5 to 2.0 s with the fast model in front; an answer after
one lookup is 5.1 s with the main model alone and 3.2 s when the fast model makes the read
itself (`GHOST_LLM_FAST_READS`). The plain reply's 1.5 s budget is met at the p50 only by
the greeting; the lookup's 2.5 s by nothing yet.

Delay added on purpose counts against the budget like any other. Today that is the
turn grace, so a pause doesn't end the operator's turn: 0.35 s when Smart Turn is sure
and the words end a sentence, 1.2 s otherwise (ADR 0049, `GHOST_TURN_CONFIDENT_*`; the
model answers during it since 2026-10-22 and the reply's first clause is rendered during
it, so it costs time only when it outlasts both or the operator goes on), the longer silences
after words that don't end a sentence (3 s, `GHOST_TURN_UNFINISHED_SECS`) or that Smart
Turn calls incomplete (1.6 s, `GHOST_TURN_STOP_SECS`), and holding a short
reply until it is complete, so narration before a lookup is never spoken. With a fast
model set (ADR 0050) two more: all of the fast model's reply is held until it ends, so
nothing of it is spoken before a hand-over, and a turn that needs the main model waits
for the fast one to say so first (about 1.5 s with the real Haiku 5.5, which takes 1.1 s
to write a call; covered by the acknowledgment, and no longer paid by a read the fast model
makes itself). Each one is a setting, and the harness shows what it
costs. One more is conditional: a reply that hits
the model's output cap (`GHOST_LLM_MAX_TOKENS`, 2000 tokens, tool calls included) is
redone in smaller steps, one extra model call, because the alternative is losing the calls
it held (`ghost_llm`; the turn is flagged `cut_off` in `latency.jsonl`).

## How we work on it

- **Measure first.** A change made for speed comes with before and after numbers from
  `make perf` or `make latency`. Without numbers it is a guess.
- **Every release runs `make perf`.** The report goes in `docs/benchmarks/perf/`, named
  by date and version. When a stage goes over its budget, the release fixes it or the
  release notes say why it didn't.
- **Changes on the voice path say what they cost.** Anything that adds a step to a turn
  (a processor, a model call, a network hop) states its effect on latency in its PR or
  commit message.
- **The big levers get a regular look:** which model and thinking mode, prompt caching
  (does the cache hit on every call after the first?), how long the prompt and the tool
  schemas are, streaming text into speech, the turn grace and VAD timings, and where each
  network hop goes.
- **Speed never buys a broken rule.** Idle audio stays on the machine, the gate still
  bounds every write, and secrets still never appear in logs. A faster path that breaks one
  of these isn't a path.

## Follow-ups

- **No model run on an intercepted turn (2026-10-20).** A goodbye that STT handed over in pieces ("All right. Good." then "I will talk to you later.") used to start a model run on the first piece, cancelled a moment later: wasted tokens and a request. The interceptor now judges the turn's pieces together and takes the earlier ones back from the aggregator when a quick phrase handles the turn (`on_handled`). Check `latency.jsonl`: an intercepted turn should carry no `llm` entry.
- **End-of-turn fallback (2026-10-20).** When the turn model says "incomplete" the stop strategy waits its full fallback before closing the turn. Seen on 4 of 10 phone turns in the morning and again on a goodbye in the afternoon ("I will talk to you later." judged incomplete; the hang-up waited 2.8 s for nothing). Shortened the same day: `GHOST_TURN_STOP_SECS` (Smart Turn's `stop_secs`) defaults to 1.6 s instead of 3 s. Watch for cut-off mid-sentence pauses; if they appear, raise it or make the fallback depend on how the utterance ended.
- **Answer during the turn grace (2026-10-22).** The model used to start only after the 0.8 s grace. It now starts at Smart Turn's verdict, the speculative turn, its answer held in the model stage until the grace ends (`GHOST_TURN_SPECULATE`; the patient stop in `ghost_voice::turns`). The answer is to the user aggregator's whole turn, not the analyzer's latest transcript, which alone answered "what's waiting for triage ... in the app repo" as "in the app repo". `make perf ARGS="--ablate none,speculate"` through the proxy: small talk p50 3.22 to 2.48 s, a goodbye through the model 3.38 to 2.60 s (tool turns there were lost in the proxy's bad minutes). Through OpenRouter: small talk 4.00 to 3.38 s, a lookup 6.76 to 6.33 s, "close infra 17" 5.58 to 5.16 s; a tool turn gains less because the turn is confirmed before the model reaches its call, which then runs as usual. `hear.speculation` in `latency.jsonl` counts answers started, kept and withdrawn; each withdrawal is a discarded model call.
- **A pause mid-sentence ended the turn (2026-10-29).** Smart Turn called "can you count, um" and "can you tell me about" finished, and 0.2 s of VAD plus the 0.8 s grace later the turn was handed to the model; going on threw the answer away and sent the rest as a second turn, so one sentence became two or three turns. And "can you" alone, which Smart Turn called incomplete, still went to the model after the 1.6 s fallback. Changes in `ghost_voice::turns`. A turn whose words stop mid-thought (a filler, a conjunction, a preposition or an article, with no full stop or question mark after it, or a trailing comma) is held for `GHOST_TURN_UNFINISHED_SECS` (3 s) instead of the grace (`sounds_unfinished`, read at the end of the grace). A turn Smart Turn calls incomplete waits the same 3 s, unless the words that then arrive end a sentence: a full stop Parakeet wrote ("I will talk to you later.") is a better sign than the model's doubt, and the turn closes after the 1.6 s `GHOST_TURN_STOP_SECS` counted from the same silence (`ends_a_sentence`); in the day's logs every unpunctuated transcript was an unfinished sentence and every punctuated one a finished one. Going on before any of the answer was heard or done reopens the turn: the model's stage takes the turn and the unheard answer out of the context, the aggregator hands it the whole turn, and a call of the half turn's answer doesn't run (`TurnControl`). The grace is 1.2 s instead of 0.8 s. `GHOST_DEBUG_AUDIO=1` logs each verdict (`turn: Smart Turn says done (0.98, 230 ms)`), each unfinished end, each answer started while the close waits and each reopened turn.
- **The answer started during the grace was never the answer (2026-10-29).** Since the Rust daemon, `hear.speculation` was `missed` on every turn and `kept` on none (the Python daemon kept 120 of 121): the answer started at Smart Turn's verdict, and the segment's transcript reaches the aggregator after the verdict, so the answer was to the turn without its last words, thrown away at the end of the grace, and the model started over then, so the whole grace was paid on every turn (`hold 0.80`, then `hold 1.20`). The answer now starts when the words arrive, a moment after the verdict, starts again when later words change the turn, and doesn't start on words that stop mid-thought; it also starts during the shortened silence of an incomplete turn whose words end a sentence, where before the model started only after the fallback. The expected cost of the 1.2 s grace is then the ~0.2 s it exceeds the model's first token (1.1–1.5 s p50), on turns the operator was done with; a turn the operator went on with costs the model call thrown away, as before. Measured on the next sessions with `make latency` (`hold` and `hear.speculation`).
- **A pause mid-sentence cost the words at the cut (2026-11-06).** Each VAD segment was read alone and the pieces joined, so Parakeet read "Remind me tomorrow morning ... to review the Phonon notes" as two sentences, each without the other's context: 24 of the operator's 49 recorded takes reached the stage in pieces (#52 item 1, #60). The user aggregator now keeps the turn's audio (`ghost_voice::turns::TurnTape`, a 1 s pre-roll then everything to the last VAD stop) and, when a second or later segment's transcript arrives, reads it once and hands the model that text instead of the pieces (ADR 0044; `GHOST_TURN_REREAD=false` for the pieces). On the recorded session the pieces scored 8.4% word error rate, the whole turn 4.2% (`make perf-hear`, `stage` against `turn`). The read happens inside the grace, about 30 ms per second of the turn with fp32 (0.24 s for the `lookup_pause` scenario's 4.5 s), so `hear` didn't move: `make perf` p50 1.45 s → 1.46 s, reply 2.05 s → 2.09 s. It needed the full-precision model (ADR 0043): the int8 one read nothing for 2 of 49 such turns.
- **A confident turn closes sooner, its first clause is ready, and a fast model may speak first (2026-11-12).** Every reply waited the 0.2 s of the VAD and the 1.2 s grace before anything else counted: a floor near 1.8 s with Kokoro's first clause, whatever the model. Three changes (ADR 0049, ADR 0050; [the report](benchmarks/perf/2026-11-12-voice-speed.md)). The grace is 0.35 s when Smart Turn is at least 0.9 sure and the words end a sentence that doesn't stop mid-thought or on a number (`ghost_voice::turns::TurnTiming`, `GHOST_TURN_CONFIDENT_GRACE_SECS`, `GHOST_TURN_CONFIDENT_PROB`); every other wait is what it was. On the operator's recorded takes (`make perf-turns TAKES=latest`) that closes 3 of 39 pauses mid-turn early against 1, the two new ones over before any voice could start and so reopened whole, and a turn's real end waits 0.85 s on average against 1.37 s (p50 0.35 s against 1.20 s). Going on still reopens the turn, now read whole across the pause, and words that began within 0.15 s of Ghost's first sample reopen it too (`GHOST_TURN_REOPEN_STARTED_SECS`; `hear.reopened` counts both). The reply's first clause is rendered as the model writes it, while the reply is held (`ghost_voice::tts::Prerender`, `GHOST_TTS_PRERENDER`, Kokoro only). And the connection may name a fast model that takes a turn's first call and hands anything with a tool call to the main one (`GHOST_LLM_FAST_MODEL`). `make perf` with the scripted model taking assumed times, p50, before to after: with a small model's time (0.45 s to a first token) a plain reply 2.08 to 1.48 s and every voice turn 3.13 to 2.57 s, the grace being what it waited for; with the main model's time (1.2 s) a plain reply 2.42 to 2.4 s, because the model already was the wait, and 1.66 s with a fast model in front, which costs a turn that goes to an app 0.67 s (4.18 to 4.93 s for a lookup, its acknowledgment at 1.48 s instead of 2.23 s). With no model time at all a plain reply is 2.24 to 1.15 s: the new floor is the VAD's 0.2 s, the read, and Kokoro's first clause. What a turn does still waits the 1.2 s: a tool call of a turn that closed on the short grace runs only once the grace would have run out, so a quick model does nothing for half a sentence. Costs counted in the same runs: no more answers withdrawn than before, a turn reopened in every run of the pause scenario and none anywhere else, no rendered clause dropped. None of this is a real-model number: run the voice rubric (parts B, C, D and the new B5) and `make latency` on the next sessions, and watch `hear.close`, `hear.reopened`, `prerender` and `handover`. Two older faults showed up while measuring on a loaded machine and are fixed: words that arrived after their turn had closed (a read slower than the grace) reopened it with nothing to close it again, and a close that came as the words did could confirm a held answer before its start reached the model's stage, so the turn got no answer.
- **The real models, a shorter first piece, and the fast model's own reads (2026-11-12, second round).** [The report's second section](benchmarks/perf/2026-11-12-voice-speed.md#second-round-real-models-first-audio-sooner-the-fast-model-reads); ADR 0049 and ADR 0050 amended. The harness drives the real models through the default connection now (`ghost-perf run --llm-url ... --llm-key-env NAME`, no vault, no settings file), and they are slower than assumed: Sonnet 5.5 1.6 to 1.8 s to its first words through Ghost's pipeline and 2.5 to 3.1 s to a tool call, Haiku 5.5 0.5 s and 1.1 s. Haiku 5.5 refused the session's thinking setting with a 400, so the fast model of ADR 0050 had handed every turn over; it is now asked without it. Three changes. The fast model makes reads itself and answers from their results while they are short (`GHOST_LLM_FAST_READS`, on whenever a fast model is set; a change, `think_harder` or a long result still go to the main model): a lookup with the real models 5.1 s with the main model alone, 6.6 s handed over, 3.3 s read by the fast model, which made the same calls as the main model in every run. A reply's first piece may end before a phrase's first word once four words are pending (`GHOST_TTS_FIRST_CLAUSE_WORDS`): a first sentence with no comma starts about 0.5 s sooner, the rest renders while it plays, and the join is shorter than the pause Kokoro leaves there in a whole render; the operator's ear decides (rubric D2). The read at the VAD's first quiet (`GHOST_STT_EARLY_READ`) is built and off: it would save 0.08 to 0.17 s a turn, but its words differ from the stop's in about one segment in ten on Kokoro's takes. A shorter VAD stop for a confident end is not taken: it closes more pauses early and leaves more of them closed when Ghost's voice starts. Costs counted: the fast model's tokens on every spoken turn, and on a turn that changes something the fast model's 1 to 1.2 s before the main model starts (`handover`).
- **A voice that starts as it is made, and words read before the stop (2026-11-12, third round).** [The report's third section](benchmarks/perf/2026-11-12-voice-speed.md#third-round-a-voice-that-starts-as-it-is-made-and-words-read-before-the-stop); ADR 0053, ADR 0054. Kokoro's render is 92 to 97 % vocoder, 0.16 to 0.21 s per second of speech, and the session's threads were already the fastest. Its graph is cut in two (`scripts/models.sh kokoro-windowed`) and with `GHOST_TTS_STREAM=windowed` the vocoder renders a window of frames at a time, the harmonic phase carried across: first audio 180 to 240 ms for 4 to 32 words, against 340 ms to 2.6 s whole. Off by default: a window is normalized over its own frames, 1 to 3 dB quieter than the whole render there, and the operator's ear decides (rubric D5, D6). With the real models and the fast model in front, a plain reply went from 1.69 s to 1.46 s at the p50 and a reply after a lookup from 3.57 s to 2.95 s. ElevenLabs now connects at the turn's first frame and cuts its first piece early: Ghost's share of its first audio fell from 219 ms on a fresh socket and 62 ms on an open one to about 25 ms, against a local stand-in. Kyutai Pocket TTS was measured and not taken (slower than Kokoro on this CPU once started, a port to build). `GHOST_STT_EARLY_READ=speculate` lets the early read's words start the speculative answer while the stop's read still makes the transcript: 0.07 to 0.14 s off a plain reply when the model is the wait, identical words and word error rate, 45 ms later to the stop's words, one more Parakeet read a segment; off by default (rubric D7). Watch `prerender`, `hear.speculation` (`early`, `early_same`, `early_differs`) and the latency line's `tts`.
