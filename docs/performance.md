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
| tools | each tool call by name, including the external API behind it (GitHub, an MCP server: a Mobbin search through the fleet proxy is 3 to 9 s, all of it the server's) |
| write | the gate: snapshot, write, read back, audit |
| voice | text to Ghost → first audio out → Ghost heard (bot started speaking) |
| transport | phone only: WebRTC each way |

Outside a turn: Start on the phone → greeting heard; pairing; the phone's `/hello` and
`/audit`; control API calls from the desktop app; daemon start → models ready.

Every live turn writes its timeline, timings only and never text, to
`~/.local/share/ghost/latency.jsonl`, with one line in the daemon log. `make latency`
summarises it. `make perf` drives scripted conversations through the real pipeline without a
person and reports each stage's p50 and p95.

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
0.8 s turn grace, so a pause doesn't end the operator's turn, and holding a short reply
until it is complete, so narration before a lookup is never spoken. Each one is a setting,
and the harness shows what it costs.

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
