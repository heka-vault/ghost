# Perf: the Rust pipeline's first `make perf` (2026-10-29)

The first run of the latency harness on `crates/ghost-voice` (ADR 0039, W7 of
heka-vault/ghost#39): Ghost's own pipeline in Rust, Pipecat's ideas and none of its code.
The harness is the Python one ported (`ghost-perf`, `make perf`): the operator's lines
rendered once by Kokoro in another voice (the same cached utterances the Python rig
used), played in at real-time pace through the real Silero, Parakeet, Smart Turn and
Kokoro, the real gate and the mock GitHub app in a temp dir. One difference: the model is
scripted (`ghost_voice::llm::ScriptedModel`, each scenario's replies and tool calls, first
token at 0 ms), so the stages around the model are what is measured; the model's own time
is the connection's and is reported by `ghostd perf llm` on the Python side.

Against the Python baseline of [2026-10-19](../2026-10-19-latency.md) (the same lines,
the same models, the real model through the proxy), per stage, p50 of 45 voice turns:

| stage | Python 2026-10-19 | Rust 2026-10-29 | note |
|---|---|---|---|
| hear (VAD silence, STT, Smart Turn, the grace) | 1.16 s | 1.04 s | the grace is 0.80 s of it in both |
| VAD stop silence | 0.20 s | 0.20 s | the floor, `GHOST_VAD_STOP_SECS` |
| STT | 0.16 s | 0.10 s | Parakeet int8 on the same CPU, `ghost-infer` |
| Smart Turn on the path | 0.01 s | 0.00 s | the verdict is awaited off the runtime |
| voice (last model event to Ghost heard) | 0.46 s | 0.38 s | Kokoro a clause at a time, `ghost-infer`; p95 0.76 s against 3.43 s |
| tools on the mock | 0.01 s | 0.01 s | the gate's submit and the app's call |
| pipeline hops (other) | 0.02 s | 0.00 s | bounded channels, one task a stage |
| reply, end of speech to Ghost's voice | 5.00 s | 1.39 s | the Python's 3.60 s of model time is 0 here: not a comparison |

Every scenario ran 3/3 with no drift and no failure (the scripted model cannot drift;
what the check sees is that each tool went through the gate and the mock, and that the
closes and the label landed). STT read 30 of 45 scripted lines word-perfect, mean word
match 94% (the rest are Parakeet's spellings of the rendered voice: "Close in for 17").

**Barge-in.** The end-to-end test (`crates/ghost-voice/tests/session.rs`,
`a_barge_in_stops_playback_within_100ms`) times a two-word interim transcript arriving
while Ghost speaks to the device cleared: 0.7 ms. The turn's token is cancelled by the
user aggregator the moment the barge-in rule fires, the output stage selects on it inside
its write loop, and every queued audio frame of the cancelled turn is dropped unplayed.

**Hold and the pause.** `lookup_pause` (a 0.7 s pause mid-sentence) still splits at the
0.8 s grace, as it did for the Python daemon 3 runs of 3 (the VAD needs its 0.2 s
`start_secs` before the hold hears the operator went on). The first half's answer is
interrupted by the second half, which runs as a turn of its own and is answered; its
`reply` (1.60 s) is timed from the second half, as the Python rig's was. Lines split at a
comma (0.3 s) were kept whole by the hold: 6 of 6.

What the harness does not measure here, and why: the real model (scripted); ElevenLabs
(the ADR 0026 engine runs in the pipeline with Kokoro behind it, tested against a fake
server, not timed); the phone transport (the media crate's, W9); Jev (`--no-jev`; the
regex classifier judged the one yes).

The harness's report follows.

# Ghost latency harness, 2026-10-29 00:13

- host: the workstation (12 CPUs)
- endpoints: the scripted model (no network)
- scenarios: greeting, small_talk, lookup, lookup_pause, chain, announce, confirm_plain, confirm_yes, bye_shortcut, bye_model, session
- runs per scenario: 3
- settings overrides: none
- history seeded: 0
- confirmation: regex only
- daemon: ghost-voice (Rust)
- wall: 369s

## scripted model local regex

### Where a voice turn's time goes (45 turns, mean reply 1.35s)

| contributor | mean s | share |
|---|---|---|
| turn hold (grace) | 0.69 | 51% |
| TTS to first audio + output | 0.40 | 30% |
| VAD stop silence | 0.20 | 15% |
| STT | 0.11 | 8% |
| tools beyond the model | 0.01 | 1% |
| LLM (all calls on the path) | 0.00 | 0% |
| pre-LLM processors (interceptors, Jev) | 0.00 | 0% |
| Smart Turn wait | 0.00 | 0% |
| pipeline hops (other) | 0.00 | 0% |

### End to end per scenario (operator stops talking -> Ghost's voice starts)

reply: from the VAD's end of speech to BotStartedSpeaking, as the live log reports it. audible: from the line's last audio frame to Ghost's voice loud at the scripted speaker or the WebRTC peer (device or network buffering included; the thinking sound can count when it is loud).

| scenario | runs ok | turns | reply p50 | reply p95 | audible p50 | hear p50 | llm p50 | llm ttfb p50 | tools p50 | tts p50 | other p50 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| greeting | 3/3 | 3 | 1.33 | 1.34 | 1.18 | 1.04 | 0.00 | 0.00 | - | 0.28 | 0.00 |
| small_talk | 3/3 | 3 | 1.66 | 1.67 | 1.51 | 1.05 | 0.00 | 0.00 | - | 0.61 | 0.00 |
| lookup | 3/3 | 3 | 1.51 | 1.54 | 1.56 | 1.05 | 0.00 | 0.00 | 0.01 | 0.46 | 0.00 |
| lookup_pause | 3/3 | 3 | 1.60 | 1.61 | 1.42 | 1.04 | 0.00 | 0.00 | - | 0.55 | 0.00 |
| chain | 3/3 | 3 | 1.85 | 1.94 | 1.75 | 1.04 | 0.00 | 0.00 | 0.01 | 0.80 | 0.00 |
| announce | 3/3 | 3 | 1.39 | 1.40 | 1.26 | 1.04 | 0.00 | 0.00 | 0.01 | 0.33 | 0.00 |
| confirm_plain | 3/3 | 3 | 1.35 | 1.37 | 1.25 | 1.05 | 0.00 | 0.00 | 0.01 | 0.29 | 0.00 |
| confirm_yes | 3/3 | 6 | 1.39 | 1.54 | 1.19 | 1.05 | 0.00 | 0.00 | 0.01 | 0.34 | 0.00 |
| bye_shortcut | 3/3 | 3 | 0.54 | 0.68 | - | 0.30 | - | - | - | 0.24 | 0.00 |
| bye_model | 3/3 | 3 | 1.31 | 1.34 | 1.27 | 1.04 | 0.00 | 0.00 | 0.00 | 0.27 | 0.00 |
| session | 3/3 | 12 | 1.37 | 1.61 | 1.23 | 1.04 | 0.00 | 0.00 | 0.01 | 0.38 | 0.00 |

### Stages across voice turns

| stage | n | p50 | p95 | max |
|---|---|---|---|---|
| reply | 45 | 1.39 | 1.82 | 1.95 |
| hear | 45 | 1.04 | 1.05 | 1.05 |
| stt | 45 | 0.10 | 0.15 | 0.17 |
| pre | 45 | 0.00 | 0.00 | 0.01 |
| turn | 45 | 0.00 | 0.00 | 0.00 |
| hold | 45 | 0.80 | 0.80 | 0.80 |
| llm | 39 | 0.00 | 0.00 | 0.00 |
| llm_ttfb | 39 | 0.00 | 0.00 | 0.00 |
| tools | 27 | 0.01 | 0.02 | 0.02 |
| tts | 45 | 0.38 | 0.76 | 0.90 |
| other | 45 | 0.00 | 0.00 | 0.00 |
| tts_synth | 45 | 0.37 | 0.76 | 0.90 |

Turns split by a VAD pause: 9/45; kept open by the hold: 6; Smart Turn said incomplete 3/54 times (each costs up to its silence timeout).

### Model calls

| scenario | calls | calls/turn | ttfb p50 | ttfb p95 | dur p50 | dur p95 | first tool p50 | in (uncached) | cache read | cache write | out | cache hit |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| greeting | 3 | 1.0 | 0.00 | 0.00 | 0.00 | 0.00 | - | 100 | 0 | 0 | 20 | 0% |
| small_talk | 3 | 1.0 | 0.00 | 0.00 | 0.00 | 0.00 | - | 100 | 0 | 0 | 20 | 0% |
| lookup | 6 | 2.0 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |
| lookup_pause | 9 | 1.5 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |
| chain | 6 | 2.0 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |
| announce | 6 | 2.0 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |
| confirm_plain | 6 | 2.0 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |
| confirm_yes | 9 | 1.5 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |
| bye_model | 3 | 1.0 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |
| session | 18 | 2.0 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 100 | 0 | 0 | 20 | 0% |

### Prompt cache, call by call (first run of each scenario)

- **lookup**: voice 2.1: in 100, read 0, write 0, ttfb 0.00; voice 2.2: in 100, read 0, write 0, ttfb 0.00
- **lookup_pause**: voice 2.1: in 100, read 0, write 0, ttfb 0.00; voice 2.2: in 100, read 0, write 0, ttfb 0.00; voice 3.1: in 100, read 0, write 0, ttfb 0.00
- **chain**: voice 2.1: in 100, read 0, write 0, ttfb 0.00; voice 2.2: in 100, read 0, write 0, ttfb 0.00
- **announce**: voice 2.1: in 100, read 0, write 0, ttfb 0.00; voice 2.2: in 100, read 0, write 0, ttfb 0.00
- **confirm_plain**: voice 2.1: in 100, read 0, write 0, ttfb 0.00; voice 2.2: in 100, read 0, write 0, ttfb 0.00
- **confirm_yes**: voice 2.1: in 100, read 0, write 0, ttfb 0.00; voice 2.2: in 100, read 0, write 0, ttfb 0.00; voice 3.1: in 100, read 0, write 0, ttfb 0.00
- **session**: voice 2.1: in 100, read 0, write 0, ttfb 0.00; voice 2.2: in 100, read 0, write 0, ttfb 0.00; voice 3.1: in 100, read 0, write 0, ttfb 0.00; voice 3.2: in 100, read 0, write 0, ttfb 0.00; voice 4.1: in 100, read 0, write 0, ttfb 0.00; voice 4.2: in 100, read 0, write 0, ttfb 0.00

### Tools (function call start -> result; read tools call the app behind them)

| tool | n | p50 | p95 | steps p50 |
|---|---|---|---|---|
| github.issue_write | 12 | 0.014 | 0.018 | prepare 0.000, submit 0.006, request 0.003 |
| github_issue_read | 6 | 0.006 | 0.007 | read 0.000 |
| github_list_issues | 6 | 0.005 | 0.005 | read 0.000 |
| hang_up | 3 | 0.000 | 0.000 | - |

### Confirmation

3 spoken answers: regex x3 classify p50 0.000s; whole step incl. the write p50 0.006s.

### STT accuracy on the scripted lines

30/45 lines word-perfect; mean word match 94%.

### Opening (session build -> greeting voice)

build p50 0.02s, pipeline start -> greeting voice p50 0.20s (p95 0.27), together p50 0.22s (the desk also plays the chime and ducks music before this).

