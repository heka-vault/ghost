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
| tools | each tool call by name, including the app behind it (a Mobbin search through the fleet proxy is 3 to 9 s, all of it the app's) |
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
by one and `make perf-llm` the real model connection with Ghost's prompt and tools. The
fixtures the models are checked against come from the oracle (`tools/oracle`,
`scripts/fixtures/record.py`; `crates/ghost-infer/tests/parity.rs`), the Python wrappers the
Rust was measured against (ADR 0039).

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

Delay added on purpose counts against the budget like any other. Today that is the
1.2 s turn grace, so a pause doesn't end the operator's turn (the model answers during it
since 2026-10-22, so it costs time only when the operator goes on), the longer silences
after words that don't end a sentence (3 s, `GHOST_TURN_UNFINISHED_SECS`) or that Smart
Turn calls incomplete (1.6 s, `GHOST_TURN_STOP_SECS`), and holding a short
reply until it is complete, so narration before a lookup is never spoken. Each one is a
setting, and the harness shows what it costs. One more is conditional: a reply that hits
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
