# Perf: GitHub through an MCP server (2026-10-20)

The first `make perf` after ADR 0022: the GitHub ability is gone, the harness runs on
`ghost-mock-github` (the official server's tool shape) declared in `infra/policy.example.toml`,
and the model reaches it through `github_issue_read` / `github_list_issues` and the declared
`github.close` / `github.set_labels` writes.

Against the 2026-10-19 baseline (the ability's own client, same model through the same proxy):

| scenario | 2026-10-19 reply p50 | 2026-10-20 reply p50 | note |
|---|---|---|---|
| lookup | 8.3 s | 7.5 s | one read; the server hop is 7 ms, the model is 5.8 s of it |
| chain | about 8 s | 7.9 s | `issue_read` twice (the issue, then its comments) |
| confirm_plain | n/a | 5.3 s | `github.close` through the gate, the request as the yes: 21 ms in the gate |
| announce | n/a | 7.4 s, 8.2 s (2 runs) | `set_labels` replaces the whole list, so the model reads first, then writes |
| session | n/a | 7.1, 6.0, 5.8, 0.7 s | two list reads, an issue read, a label at announce, a goodbye; no problems |

The server hop costs nothing a voice turn can feel (`tools p50 0.01 s`); every second is
still the model's. The first `announce` run produced no reply: after the issue read the
model's second call spent 510 output tokens thinking and ended with neither text nor a tool
call (`narration_dropped`); three reruns answered in 7.4 to 8.2 s and labelled the issue.
That is the `between_tools` thinking path, not the server, and it is the next thing to look
at. The harness's label check now reads "ready for agent" and "ready-for-agent" as one
label: the check is that the write went through the gate, not how the model hyphenates.

The harness's report follows.

- host: the workstation (12 CPUs)
- endpoints: claude-sonnet-5-5 via the LAN proxy
- scenarios: lookup, chain, announce, confirm_plain
- runs per scenario: 1
- settings overrides: none
- history seeded: 0
- confirmation: Jev when a key is set, else regex
- wall: 123s

## claude-sonnet-5-5 proxy

### Where a voice turn's time goes (3 turns, mean reply 6.90s)

| contributor | mean s | share |
|---|---|---|
| LLM (all calls on the path) | 4.99 | 72% |
| turn hold (grace) | 0.80 | 12% |
| TTS to first audio + output | 0.66 | 10% |
| VAD stop silence | 0.20 | 3% |
| STT | 0.17 | 3% |
| pipeline hops (other) | 0.06 | 1% |
| tools beyond the model | 0.02 | 0% |
| Smart Turn wait | 0.01 | 0% |
| pre-LLM processors (interceptors, Jev) | 0.00 | 0% |

### End to end per scenario (operator stops talking -> Ghost's voice starts)

reply: from the VAD's end of speech to BotStartedSpeaking, as the live log reports it. audible: from the line's last audio frame to Ghost's voice loud at the scripted speaker or the WebRTC peer (device or network buffering included; the thinking sound can count when it is loud).

| scenario | runs ok | turns | reply p50 | reply p95 | audible p50 | hear p50 | llm p50 | llm ttfb p50 | tools p50 | tts p50 | other p50 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| lookup | 1/1 | 1 | 7.54 | 7.54 | 7.52 | 1.20 | 5.80 | 1.57 | 0.01 | 0.46 | 0.07 |
| chain | 1/1 | 1 | 7.90 | 7.90 | 7.84 | 1.17 | 5.90 | 1.10 | 0.01 | 0.76 | 0.06 |
| announce | 0/1 | 0 | - | - | - | - | - | - | - | - | - |
| confirm_plain | 1/1 | 1 | 5.26 | 5.26 | 5.18 | 1.17 | 3.25 | 1.27 | 0.02 | 0.77 | 0.04 |

### Stages across voice turns

| stage | n | p50 | p95 | max |
|---|---|---|---|---|
| reply | 3 | 7.54 | 7.86 | 7.90 |
| hear | 3 | 1.17 | 1.19 | 1.20 |
| stt | 3 | 0.17 | 0.19 | 0.19 |
| pre | 3 | 0.00 | 0.00 | 0.00 |
| turn | 3 | 0.01 | 0.01 | 0.01 |
| hold | 3 | 0.80 | 0.80 | 0.80 |
| llm | 3 | 5.80 | 5.89 | 5.90 |
| llm_ttfb | 3 | 1.27 | 1.54 | 1.57 |
| tools | 3 | 0.01 | 0.02 | 0.02 |
| tts | 3 | 0.76 | 0.77 | 0.77 |
| other | 3 | 0.06 | 0.07 | 0.07 |
| tts_synth | 3 | 0.73 | 0.73 | 0.73 |

Turns split by a VAD pause: 1/3; kept open by the hold: 1; Smart Turn said incomplete 0/4 times (each costs up to its 3 s silence timeout).

### Model calls

| scenario | calls | calls/turn | ttfb p50 | ttfb p95 | dur p50 | dur p95 | first tool p50 | in (uncached) | cache read | cache write | out | cache hit |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| lookup | 3 | 3.0 | 1.57 | 1.59 | 1.84 | 2.69 | 2.31 | 4 | 2701 | 1420 | 164 | 65% |
| chain | 3 | 3.0 | 1.39 | 1.50 | 1.69 | 2.59 | 2.19 | 4 | 3665 | 573 | 230 | 86% |
| announce | 2 | 2.0 | 2.35 | 3.33 | 2.71 | 3.37 | 1.99 | 4 | 3398 | 744 | 336 | 82% |
| confirm_plain | 2 | 2.0 | 1.21 | 1.26 | 1.63 | 2.06 | 2.11 | 4 | 3391 | 722 | 116 | 82% |

### Prompt cache, call by call (first run of each scenario)

- **lookup**: voice 2.1: in 4, read 0, write 3982, ttfb 1.57; voice 2.2: in 4, read 3982, write 139, ttfb 1.59; voice 2.3: in 4, read 4121, write 138, ttfb 1.17
- **chain**: voice 2.1: in 4, read 2804, write 1185, ttfb 1.10; voice 2.2: in 4, read 3989, write 213, ttfb 1.39; voice 2.3: in 4, read 4202, write 321, ttfb 1.51
- **announce**: voice 2.1: in 4, read 2804, write 1188, ttfb 1.27; voice 2.2: in 4, read 3992, write 301, ttfb 3.44
- **confirm_plain**: voice 2.1: in 4, read 2804, write 1174, ttfb 1.27; voice 2.2: in 4, read 3978, write 269, ttfb 1.14

### Tools (function call start -> result; read tools call the GitHub client)

| tool | n | p50 | p95 | steps p50 |
|---|---|---|---|---|
| github.close | 1 | 0.021 | 0.021 | prepare 0.000, submit 0.005, request 0.012 |
| github_issue_read | 2 | 0.007 | 0.008 | read 0.002 |
| github_list_issues | 2 | 0.007 | 0.007 | read 0.002 |

### STT accuracy on the scripted lines

4/4 lines word-perfect; mean word match 100%.

### Opening (session build -> greeting voice)

build p50 0.04s, pipeline start -> greeting voice p50 0.38s (p95 0.40), together p50 0.42s (the desk also plays the chime and ducks music before this).

### Problems (model drift or failures)

- announce run 1: line 1: no spoken reply
- announce run 1: line 1: expected ['change'], got no tools
- announce run 1: line 1: expected status done:announce, got none
- announce run 1: ghost-mock/app#151 lacks 'ready-for-agent'

