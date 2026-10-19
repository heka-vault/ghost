# Latency: the per-turn timeline and the harness

Every Ghost turn is timed, live, and a harness replays scripted conversations through the
real pipeline to find slow parts without a human. Results from the first runs are in
[2026-10-19-latency.md](2026-10-19-latency.md).

## The live timeline (`ghost_voice.latency`)

`LatencyObserver` is a Pipecat observer that `build_session` attaches to every session, desk
and phone. It sees each frame once (on its first push), keeps the moments of a turn on the
pipeline clock, and takes a few durations straight from Ghost's own processors (the turn
hold, the interceptors, the confirmation classifier, the steps inside a tool). When a turn
is over it writes one INFO line to ghostd's log and one JSON record to
`~/.local/share/ghost/latency.jsonl` (`GHOST_LATENCY_LOG` to move it; rotated to `.1` at
`GHOST_LATENCY_MAX_MB`, default 5; `GHOST_LATENCY=0` turns it off).

```
latency: turn 2 desk voice reply 4.78s = hear 1.18 (stt 0.17, turn 0.01, hold 0.80) + llm 3.13 (ttfb 1.53, 2 calls, cache 79%) + tools 0.00 (list_awaiting_signoff 0.00) + tts 0.45 + other 0.01
```

A voice turn is timed from the moment the operator stopped talking (the VAD's stop minus
its `stop_secs`) to `BotStartedSpeaking`, and the parts tile that interval:

| part | from -> to | what is in it |
|---|---|---|
| `hear` | operator silent -> turn released | VAD silence (`vad`), STT (`stt`, and `stt_model` for the model alone), interceptors and the confirmation classifier (`pre`), Smart Turn and waiting for the transcript (`turn`), Ghost's grace hold (`hold`) |
| `llm` | each model call, start -> end | `ttfb`, `first_tool`, `first_text`, `dur`, tokens `in`/`out`/`cache_read`/`cache_write`, tools called |
| `tools` | function call -> result | the part not overlapping a model call; per call the name, `op` for `change`, `status` (`ok`, `done:announce`, `done:request`, `needs_confirmation`, `error`), and `steps` (`read` = the ability's client, an external API for GitHub; `prepare`, `submit` = gate snapshot plus the write below `confirm`; `request` = ADR 0017's check that may execute) |
| `tts` | last model call ends -> Ghost's voice | narration release (`wait`), Kokoro to first audio (`synth`), transport (`out`) |
| `other` | the rest | frame hops: release -> model, tool result -> next model call |

Other kinds: `opening` (pipeline start, or the phone connecting, to the greeting; `build` is
`build_session` to pipeline start), `text` (a typed turn), `system` (a model run nobody
spoke for, such as a desktop Confirm). Flags: `narration_dropped`, `interrupted` (the
operator spoke again before Ghost answered), `barged_in`. A confirmation turn carries
`confirm: {classifier: jev | guard | regex-fallback | regex-unsure | regex, classify, verdict, total}`.

Records hold numbers, counts, model ids, tool and operation names and classifier verdicts.
Never transcripts, model text, tool arguments or results (tested in
`packages/voice/tests/test_latency.py`).

`make latency` (`ghostd latency [--last N] [--origin desk|phone] [--json]`) prints p50/p95
per stage, overall and per origin, and the last turns.

## The harness (`ghostd perf`)

Scripted conversations through `build_session`, headless:

- **In:** the operator's lines rendered once by Kokoro in another voice (`am_michael`),
  16 kHz, cached in `.scratch/perf/utterances/`; a line with ` ... ` has a 0.7 s pause in the
  middle. A scripted input transport sends 20 ms frames at real-time pace, silence between
  lines. Real Silero VAD, Parakeet, Smart Turn.
- **Out:** an output transport that takes Ghost's audio at sound-card pace and drops it, so
  `BotStarted/StoppedSpeaking` behave as on the desk. It notes when the output turns loud:
  `audible` is the operator's last frame to Ghost's voice at the speaker.
- **Model:** `--endpoint proxy` or `openrouter`, the same switch as `GHOST_LLM_ENDPOINT`
  (OpenRouter's Anthropic Messages API with `OPENROUTER_API_KEY`). Keys are read from the
  environment and never printed.
- **Before and after:** `--ablate` turns the 2026-10-19 changes off one by one, so a change
  can be measured against Ghost without it in the same run: `tts_clauses` (whole sentences
  to Kokoro), `issue_comments` (get_issue without comments), `prompt_lines` (no "lead with a
  few words" and "parallel lookups" lines), `system_cache` (no cache marker on the system
  prompt), `shared_client` (a client per session). `all` is Ghost before them, `none` after.
- **Interleaved:** every config runs a scenario before any runs it again, so a slow minute
  at the endpoint lands on all of them.
- **State:** a fresh temp dir per run: its own SQLite, conversations, mock GitHub
  (`GHOST_GITHUB_MOCK=1`), the example policy (`approve` announces, `close` confirms), Jev
  when a key is set. The operator's data is never read or written. No mic, no speakers.
- **Phone:** `--transport webrtc` runs the same scenario through `SmallWebRTCTransport` with an
  aiortc peer on loopback sending Opus; `audible` is then measured at the peer, so it includes
  both jitter buffers. Use `--set thinking_sound=false` so loud means Ghost's voice.

Scenarios (`ghostd/perf/scenarios.py`): `greeting`, `small_talk`, `lookup`
(list_awaiting_signoff), `lookup_pause` (the same with a pause mid-sentence), `chain`
(an issue and what people said: one get_issue with its comments), `announce` (approve), `confirm_plain` ("close infra 17": the
request is the yes, ADR 0017), `confirm_yes` (a hedge, Ghost asks, "yes, go ahead"),
`bye_shortcut` (quick intents), `bye_model` (go_to_sleep), `session` (four turns, cache growth).

```sh
make perf                                        # every scenario, 3 runs, Ghost's model
make perf SCENARIO=lookup,chain RUNS=5
make perf MODEL=claude-sonnet-5-5,claude-haiku-5-5 ENDPOINT=proxy,openrouter RUNS=2
make perf ARGS="--thinking between_tools,adaptive"   # Sonnet 5.5 rejects disabled; Haiku rejects between_tools
make perf ARGS="--set turn_grace_secs=0.4"       # any VoiceSettings field
make perf ENDPOINT=proxy,openrouter ARGS="--ablate all,none"   # before vs after, both endpoints
make perf ARGS="--transport webrtc --set thinking_sound=false"
make perf ARGS="--history 8"                     # seed earlier turns into the record
make perf-micro                                  # STT by length, Kokoro first audio, Smart Turn, Jev
make perf-llm ARGS="--openrouter-models google/gemini-3.8-flash,openai/gpt-5.6-luna"
```

Each run writes `.scratch/perf/<timestamp>/report.md` (ranked contributors, per-scenario and
per-stage p50/p95, model calls with cache reads and writes call by call, tools,
confirmation, STT accuracy, opening), `records.json` (re-render with
`ghostd perf report records.json`) and `latency.jsonl`. Checks are soft: a run where the
model drifts (asks instead of acting) is listed under Problems, not failed.

Cost: a scenario run is one to four model calls of about 3.5k input tokens, mostly cache
reads. Wall time is dominated by Ghost speaking in real time: about 30 s a run.
