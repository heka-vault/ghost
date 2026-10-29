# Latency: the per-turn timeline and the harness

Every Ghost turn is timed, live, and a harness replays scripted conversations through the
real pipeline to find slow parts without a human. Results from the first runs are in
[2026-10-19-latency.md](2026-10-19-latency.md).

## The live timeline (`ghost_voice::latency`)

The timeline is a sink (`ghost_voice::latency::Sink`) that `build_session` attaches to every
session, desk and phone. It sees each frame once as it leaves its stage, keeps the moments
of a turn on the pipeline clock, and takes a few durations straight from Ghost's own stages (the turn
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
| `hear` | operator silent -> turn released | VAD silence (`vad`), STT (`stt`, and `stt_model` for the model alone), interceptors and the confirmation classifier (`pre`), Smart Turn and waiting for the transcript (`turn`), Ghost's grace hold (`hold`; `speculation` counts answers started during it and how each ended: `kept`, `withdrawn`, `missed`, `tool`) |
| `llm` | each model call, start -> end | `ttfb`, `first_tool`, `first_text`, `dur`, tokens `in`/`out`/`cache_read`/`cache_write`, tools called |
| `tools` | function call -> result | the part not overlapping a model call; per call the name, `op` for `change`, `status` (`ok`, `done:announce`, `done:request`, `needs_confirmation`, `error`), and `steps` (`read` = the ability's client, an external API for GitHub; `prepare`, `submit` = gate snapshot plus the write below `confirm`; `request` = ADR 0017's check that may execute) |
| `tts` | last model call ends -> Ghost's voice | narration release (`wait`), Kokoro to first audio (`synth`), transport (`out`) |
| `other` | the rest | frame hops: release -> model, tool result -> next model call |

Other kinds: `opening` (pipeline start, or the phone connecting, to the greeting; `build` is
`build_session` to pipeline start), `text` (a typed turn), `system` (a model run nobody
spoke for, such as a desktop Confirm). Flags: `narration_dropped`, `interrupted` (the
operator spoke again before Ghost answered), `barged_in`. A barged-in reply carries
`barge_in`: from when the operator began talking over it (VAD start minus its `start_secs`)
to the interruption; `docs/benchmarks/scripts/barge_in_bench.py` measures it headless, with
interim transcripts off and on. A confirmation turn carries
`confirm: {classifier: jev | guard | regex-fallback | regex-unsure | regex, classify, verdict, total}`.

Records hold numbers, counts, model ids, tool and operation names and classifier verdicts.
Never transcripts, model text, tool arguments or results (tested in
`crates/ghost-voice/src/latency.rs`).

`make latency` (`ghostd latency [--last N] [--origin desk|phone] [--json]`) prints p50/p95
per stage, overall and per origin, and the last turns.

## The harness (`ghostd perf`)

`crates/ghost-voice/src/perf/` (ADR 0039). Scripted conversations through `build_session`,
headless:

- **In:** the operator's lines rendered once by Kokoro in another voice (`am_michael`),
  16 kHz, cached in `.scratch/perf/utterances/`; a line with ` ... ` has a 0.7 s pause in the
  middle. A scripted input transport sends 20 ms frames at real-time pace, silence between
  lines. Real Silero VAD, Parakeet, Smart Turn.
- **Out:** an output transport that takes Ghost's audio at sound-card pace and drops it, so
  Ghost started and stopped speaking behave as on the desk. It notes when the output turns
  loud: `audible` is the operator's last frame to Ghost's voice at the speaker.
- **Model:** scripted (`ghost_voice::llm::ScriptedModel`) with the scenario's replies, so
  every tool call reaches the real gate and the real mock app while the model's own time is
  a setting (`--ttfb-ms`); no tokens are spent. The real model connection is measured by
  `ghostd perf llm` (below), with Ghost's real system prompt and tool schemas.
- **Before and after:** `--ablate` turns the 2026-10-19 changes off one by one, so a change
  can be measured against Ghost without it in the same run: `tts_clauses` (whole sentences
  to Kokoro), `speculate` (the model waits out the turn grace, 2026-10-22); `prompt_lines`,
  `system_cache` and `shared_client` are the model connection's and change nothing with the
  scripted model, accepted for the label's sake. `all` is Ghost before them, `none` after.
- **Interleaved:** every config runs a scenario before any runs it again.
- **State:** a fresh temp dir per run: its own SQLite, conversations, mock GitHub
  (`ghost-mock-github`, the app `infra/example/apps.json` names; the harness's policy asks
  before a close so the scenarios that measure the yes have one to measure), Jev when a key
  is set (`--no-jev` for the regex alone). The operator's data is never read or written. No
  mic, no speakers.

Scenarios (`crates/ghost-voice/src/perf/scenarios.rs`): `greeting`, `small_talk`, `lookup`
(github_list_issues on the mock GitHub app), `lookup_pause` (the same with a pause mid-sentence), `chain`
(an issue and what people said: github_issue_read for the issue and its comments), `announce` (a label), `confirm_plain` ("close infra 17": the
request is the yes, ADR 0017), `confirm_yes` (a hedge, Ghost asks, "yes, go ahead"),
`bye_shortcut` (quick intents), `bye_model` (go_to_sleep), `session` (four turns, cache growth).

```sh
make perf                                        # every scenario, 3 runs
make perf SCENARIO=lookup,chain RUNS=5
make perf ARGS="--set turn_grace_secs=0.4"       # any VoiceSettings field
make perf ARGS="--ablate all,none"               # before vs after
make perf ARGS="--ttfb-ms 800"                   # the scripted model's time to its first token
make perf ARGS="--history 8"                     # seed earlier turns into the record
make perf-micro                                  # STT by length, Kokoro first audio, Smart Turn, Jev
make perf-llm                                    # the model connection as configured: hello TTFB, the tool call, the answer
make perf-llm ARGS="--endpoint proxy,openrouter --model claude-sonnet-5-5,claude-haiku-5-5 --n 4"
make perf-llm ARGS="--openrouter-models google/gemini-3.8-flash,openai/gpt-5.6-luna"
```

`perf llm`: `--endpoint current` (the default) is the model connection as configured (ADR
0037), on whichever protocol it speaks; a provider id (`anthropic`, `openrouter`, `ollama`,
…) is that provider as it comes, with its own key; `proxy` and `openrouter` are the older
`GHOST_LLM_ENDPOINT` switch; `+sysbp` puts the cache marker on the system prompt. Keys are
read from the environment (the vault's, through `ghostd`) and never printed. Spend is small:
a few thousand input tokens a call, mostly cached.

Each run writes `.scratch/perf/<timestamp>/report.md` (ranked contributors, per-scenario and
per-stage p50/p95, model calls with cache reads and writes call by call, tools,
confirmation, STT accuracy, opening), `records.json` (re-render with
`ghostd perf report records.json`) and `latency.jsonl`; `perf micro` writes `micro.md` and
`micro.json`, `perf llm` `llm.md` and `llm.json`. Checks are soft: a run where the scripted
reply drifts from what the scenario expects is listed under Problems, not failed.

Wall time is dominated by Ghost speaking in real time: about 30 s a run.
