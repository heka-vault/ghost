# Perf: a confident close, a ready voice, a fast model (2026-11-12)

Three changes to how soon Ghost answers, measured one at a time and together with
`make perf`: a turn whose end is sure closes after 0.35 s instead of 1.2 s and is reopened
if the operator goes on, the reply's first clause is rendered while the reply is still
held ([ADR 0049](../../adr/0049-a-confident-turn-closes-sooner.md)), and a fast model may
answer a spoken turn first and hand real work to the main one
([ADR 0050](../../adr/0050-a-fast-model-speaks-first.md)).

**Read this first.** Every latency below is from the harness's scripted model, which
answers with the scenario's reply after an assumed delay. The delays stand in for real
models and were not measured in these runs:

| profile | first token | then | stands for |
|---|---|---|---|
| `sonnet` | 1.2 s | 60 tokens/s | the main model as Ghost runs it (1.1 to 1.5 s to a first token, 2026-10-19 to 2026-10-29) |
| `haiku` | 0.45 s | 150 tokens/s | a small model on the same connection (0.6 s to "hello" on 2026-10-19) |
| `instant` | 0 | at once | no model at all: the stages alone |

A word is 1.3 tokens and a tool call 12 tokens and its arguments. The speech in, the
turn's close, the gate, the mock app and Kokoro are real; the microphone, the room, the
echo canceller and the model are not. A run by the operator with the real model and the
real microphone ([the voice rubric](../../voice-rubric.md), parts B, C and D) is still
needed before any of these numbers is believed about a conversation, and before the
fast model is turned on.

## What was run

`ghost-perf run --no-jev --runs 3` on the workstation (12 CPUs), every scenario, the
operator's lines rendered by Kokoro and played in at real-time pace, Parakeet at full
precision (ADR 0043), the turn read whole (ADR 0044). `ghost-perf` is `ghostd perf`
without the vault: the harness was run with the code's default settings, a cache of its
own and no key, so nothing of the operator's was read but the model files.

- **Baseline:** `origin/feat/apps-parity` at dd016bf (since merged to main) with one
  change, the scripted model's delays, under each profile, three runs.
- **Before and after:** the branch, with each change switched by its settings in one
  process, every variant running a scenario before any runs it again:
  `before` (`GHOST_TURN_CONFIDENT_GRACE_SECS=1.2`, `GHOST_TURN_REOPEN_STARTED_SECS=0`,
  `GHOST_TTS_PRERENDER=false`: the one grace and nothing rendered ahead), `1 grace` (the
  short grace and the wider reopen), `2 prerender` (the first clause rendered ahead),
  `1+2` (both: the branch as it ships), `3 fast` (`before` with a fast model at the
  `haiku` profile in front), `all` (everything). The fast model is scripted to answer what
  the scenario's model would have opened with: plain speech is its reply, a tool call is
  its hand-over.

The `sonnet` tables are two passes of three runs, six runs a scenario, made before the
branch was rebased onto main; the `haiku` and `instant` tables are one pass of three on the
rebased branch. Nothing in the rebase touches a spoken turn, and the one later change that
does (a quick model's tool call waits out the grace, below) cannot act under the `sonnet`
profile, whose calls come after the grace has run.

The workstation was not quiet. Other sessions were building on it through the night
(load averages of 7 to 22), and Parakeet, Smart Turn and Kokoro slow down under that: in
the baseline the read of a turn took over 0.4 s on 23 of 135 turns against a p50 of 0.20 s,
and Kokoro's first clause up to 2.2 s against 0.6 s. The p50s hold from pass to pass
within about 0.1 s; the p95s are mostly that noise and are not worth reading closely. The
first `sonnet` pass ran beside the `haiku` one and the second alone; where the two
disagree the cell says so below. Four of the 396 `sonnet` runs got no reply at all, each at a
load spike with the read of the turn taking 1.5 s or more; they are counted in the last
row of the table and are not in the percentiles.

Three faults showed in those runs and were fixed along the way, each with a test. Two are
older than these changes and need the read of a turn to take as long as the grace: words
that arrived after their turn had closed reopened it with nothing left to judge its end
by, and it hung until the idle timer; and a close that came the instant the words did
could confirm a held answer whose start had not reached the model's stage, so the turn
got no answer. The third was this branch's: with a quick model, a turn closed on the short
grace made its tool call inside the pause it should have waited out. A tool call of such
a turn now waits until the grace itself would have run out, and the `haiku` and `instant`
tables are from after that. The harness had one of its own: its scripted model, asked
again for a turn that had been reopened, answered with the line's next reply and skipped
the tool call a real model would make again; it now starts the turn's answer again, and
the pause scenario under the quick profiles was run again with that.

## Baseline

Reply, seconds from the end of the operator's speech to Ghost's voice, p50 / p95:

| scenario | scripted model local sonnet regex | scripted model local haiku regex | scripted model local instant regex |
|---|---|---|---|
| greeting | 1.99 / 2.21 | 1.84 / 1.84 | 1.75 / 1.89 |
| small_talk | 2.46 / 2.52 | 2.05 / 2.13 | 2.04 / 2.07 |
| lookup | 4.33 / 4.33 | 3.48 / 4.85 | 3.71 / 3.82 |
| lookup_pause | 6.49 / 6.76 | 3.51 / 3.56 | 2.29 / 3.85 |
| chain | 6.24 / 9.49 | 3.88 / 4.27 | 2.56 / 2.83 |
| announce | 4.38 / 4.73 | 3.34 / 3.44 | 2.14 / 2.16 |
| confirm_plain | 4.07 / 4.08 | 3.66 / 3.75 | 2.60 / 2.61 |
| confirm_yes | 3.07 / 5.73 | 2.48 / 4.07 | 2.01 / 2.17 |
| bye_shortcut | 0.76 / 0.85 | 0.71 / 0.96 | 0.89 / 1.83 |
| bye_model | 2.66 / 4.38 | 2.42 / 2.74 | 1.89 / 2.01 |
| session | 4.19 / 6.06 | 3.29 / 4.42 | 2.11 / 2.89 |
| every voice turn | 4.08 / 6.46 (n=45) | 3.17 / 4.36 (n=45) | 2.14 / 3.58 (n=45) |
| runs with a problem | 0/33 | 0/33 | 0/33 |

| stage, p50 / p95 | scripted model local sonnet regex | scripted model local haiku regex | scripted model local instant regex |
|---|---|---|---|
| hear (end of speech to the turn released) | 1.47 / 1.97 | 1.47 / 1.97 | 1.47 / 1.97 |
| of it the hold | 1.20 / 1.70 | 1.20 / 1.70 | 1.20 / 1.70 |
| of it the read | 0.20 / 0.58 | 0.19 / 0.59 | 0.20 / 0.67 |
| model, all calls on the path | 2.08 / 3.13 | 1.15 / 1.23 | 0.00 / 0.00 |
| voice (last model event to Ghost heard) | 0.62 / 2.21 | 0.56 / 1.64 | 0.60 / 2.05 |
| acknowledgment heard, turns that had one | 2.24 / 3.25 | 2.19 / 2.67 | 1.48 / 1.99 |
| reply | 4.08 / 6.46 | 3.17 / 4.36 | 2.14 / 3.58 |

A plain reply at the baseline is the 0.2 s the VAD waits, the 1.2 s grace with the read
inside it, whatever of the model's time is left when the grace ends, and Kokoro's first
clause: 2.05 s with the small model's time, 2.46 s with the main model's. With a quick
model most held answers reach their tool call inside the grace, are withdrawn, and are
made again at the close (29 of 50 under `haiku`), which is why a lookup takes 3.5 s
there.

## Before and after, the main model's time (`sonnet`)

Reply, p50 / p95:

| scenario | before sonnet | 1 grace sonnet | 2 prerender sonnet | 1+2 sonnet | 3 fast sonnet | all sonnet |
|---|---|---|---|---|---|---|
| greeting | 2.26 / 2.95 | 2.26 / 2.66 | 2.08 / 2.56 | 1.99 / 2.14 | 1.86 / 2.07 | 1.28 / 1.40 |
| small_talk | 2.42 / 3.47 | 2.45 / 4.48 | 2.46 / 2.82 | 2.71 / 2.95 | 2.16 / 2.46 | 1.66 / 1.97 |
| lookup | 4.32 / 5.15 | 4.36 / 4.49 | 4.24 / 4.26 | 4.18 / 5.10 | 5.05 / 6.42 | 4.93 / 6.83 |
| lookup_pause | 4.53 / 5.36 | 4.38 / 4.62 | 4.42 / 4.61 | 4.28 / 5.33 | 5.10 / 6.15 | 4.93 / 6.02 |
| chain | 5.70 / 7.97 | 5.85 / 5.93 | 5.54 / 6.00 | 5.61 / 6.62 | 6.64 / 6.95 | 6.41 / 8.28 |
| announce | 4.15 / 5.67 | 4.25 / 4.38 | 4.22 / 4.63 | 4.21 / 4.30 | 4.88 / 5.48 | 4.94 / 6.68 |
| confirm_plain | 3.91 / 4.75 | 3.93 / 4.07 | 4.01 / 4.72 | 3.92 / 4.35 | 4.77 / 4.82 | 4.53 / 4.97 |
| confirm_yes | 3.21 / 5.14 | 3.41 / 4.60 | 3.98 / 5.08 | 3.73 / 5.77 | 3.61 / 6.99 | 4.12 / 6.57 |
| bye_shortcut | 0.81 / 3.23 | 0.83 / 2.97 | 1.04 / 1.69 | 0.89 / 1.38 | 1.11 / 1.52 | 0.85 / 1.66 |
| bye_model | 2.70 / 5.34 | 2.87 / 4.22 | 2.41 / 4.80 | 2.68 / 3.01 | 3.21 / 3.54 | 2.98 / 3.88 |
| session | 4.05 / 5.35 | 4.09 / 5.55 | 4.00 / 4.49 | 3.96 / 4.48 | 4.81 / 6.25 | 4.59 / 8.12 |
| every voice turn | 4.05 / 5.75 (n=90) | 4.05 / 5.79 (n=89) | 4.02 / 5.58 (n=88) | 3.96 / 5.51 (n=90) | 4.74 / 6.64 (n=89) | 4.56 / 6.96 (n=90) |
| runs with a problem | 0/66 | 1/66 | 2/66 | 0/66 | 1/66 | 0/66 |

| stage, p50 / p95 | before sonnet | 1 grace sonnet | 2 prerender sonnet | 1+2 sonnet | 3 fast sonnet | all sonnet |
|---|---|---|---|---|---|---|
| hear (end of speech to the turn released) | 1.47 / 1.97 | 0.62 / 1.97 | 1.46 / 1.97 | 0.62 / 1.96 | 1.47 / 1.98 | 0.63 / 1.97 |
| of it the hold | 1.20 / 1.70 | 0.35 / 1.70 | 1.20 / 1.70 | 0.35 / 1.70 | 1.20 / 1.70 | 0.35 / 1.70 |
| of it the read | 0.20 / 0.62 | 0.19 / 0.67 | 0.19 / 0.45 | 0.20 / 0.48 | 0.20 / 0.87 | 0.19 / 0.92 |
| model, all calls on the path | 1.95 / 2.97 | 2.12 / 3.77 | 2.00 / 2.98 | 2.10 / 3.78 | 2.67 / 3.84 | 2.84 / 4.63 |
| voice (last model event to Ghost heard) | 0.58 / 1.92 | 0.58 / 1.65 | 0.59 / 1.49 | 0.53 / 1.43 | 0.62 / 1.32 | 0.51 / 2.41 |
| acknowledgment heard, turns that had one | 2.24 / 3.01 | 2.25 / 2.96 | 2.23 / 2.94 | 2.23 / 2.92 | 1.50 / 2.19 | 1.48 / 2.05 |
| reply | 4.05 / 5.75 | 4.05 / 5.79 | 4.02 / 5.58 | 3.96 / 5.51 | 4.74 / 6.64 | 4.56 / 6.96 |

With the main model's time the first two changes buy almost nothing at the p50, and the
table says why: `hear` falls from 1.47 s to 0.62 s and the model's share rises by the
same amount. The model's first token, 1.2 s after the words, was already later than the
grace; the turn now waits for it in the open instead of inside the hold. A reply whose
first clause is short gains the render (the greeting, 2.26 s to 1.99 s: "Doing well." is
rendered while the rest is written). The small-talk cell for `1+2` (2.71 s) is load:
twelve runs of the two plain scenarios alone on a quieter machine gave

| scenario | before | 1 grace | 1+2 |
|---|---|---|---|
| small_talk | 2.42 / 2.58 | 2.37 / 2.60 | 2.38 / 2.58 |
| greeting | 2.06 / 2.56 | 2.10 / 2.82 | 1.95 / 2.03 |

The fast model is what moves a plain reply: 2.42 s to 1.66 s for small talk, 2.26 s to
1.28 s for the greeting, with everything on (`all`). On its own behind the 1.2 s grace
(`3 fast`) it buys only 0.3 s: the three changes need each other. It costs every turn
that goes to an app the fast model's call first, 0.67 s on average: a lookup goes from
4.18 s to 4.93 s, and the goodbye only the model recognizes from 2.68 s to 2.98 s. What
the operator hears first on those turns is the acknowledgment, at 1.48 s instead of
2.23 s.

## Before and after, a small model's time (`haiku`)

| scenario | before haiku | 1 grace haiku | 2 prerender haiku | 1+2 haiku |
|---|---|---|---|---|
| greeting | 1.83 / 1.83 | 1.28 / 1.36 | 1.47 / 1.47 | 1.21 / 1.22 |
| small_talk | 2.08 / 2.24 | 1.69 / 1.75 | 1.47 / 1.72 | 1.48 / 1.49 |
| lookup | 3.47 / 3.48 | 2.63 / 2.83 | 3.20 / 3.39 | 2.58 / 2.61 |
| chain | 4.27 / 4.39 | 3.12 / 4.05 | 4.28 / 4.28 | 4.45 / 5.33 |
| announce | 3.19 / 4.43 | 2.92 / 3.05 | 3.23 / 3.29 | 2.60 / 2.84 |
| confirm_plain | 3.52 / 3.73 | 3.55 / 4.35 | 3.71 / 3.91 | 3.71 / 3.94 |
| confirm_yes | 2.46 / 3.39 | 2.21 / 3.33 | 2.33 / 3.21 | 2.12 / 3.15 |
| bye_shortcut | 0.76 / 0.84 | 0.77 / 0.91 | 0.79 / 0.94 | 0.72 / 0.79 |
| bye_model | 2.51 / 2.57 | 1.96 / 1.98 | 2.37 / 2.42 | 1.81 / 2.16 |
| session | 3.24 / 4.24 | 2.44 / 4.07 | 3.29 / 3.74 | 2.91 / 5.20 |
| lookup_pause | 3.47 / 3.47 | 2.84 / 3.16 | 3.42 / 3.43 | 2.77 / 2.96 |
| every voice turn | 3.13 / 4.38 (n=45) | 2.60 / 4.11 (n=45) | 3.19 / 3.99 (n=45) | 2.57 / 5.01 (n=45) |
| runs with a problem | 0/33 | 0/33 | 0/33 | 0/33 |

| stage, p50 / p95 | before haiku | 1 grace haiku | 2 prerender haiku | 1+2 haiku |
|---|---|---|---|---|
| hear (end of speech to the turn released) | 1.46 / 1.97 | 0.62 / 1.96 | 1.47 / 1.96 | 0.62 / 1.98 |
| of it the hold | 1.20 / 1.70 | 0.35 / 1.70 | 1.20 / 1.70 | 0.35 / 1.70 |
| of it the read | 0.18 / 0.59 | 0.18 / 0.31 | 0.18 / 0.49 | 0.19 / 0.66 |
| model, all calls on the path | 1.18 / 1.47 | 1.20 / 1.46 | 1.18 / 1.46 | 1.19 / 1.46 |
| voice (last model event to Ghost heard) | 0.54 / 1.45 | 0.53 / 1.27 | 0.53 / 1.09 | 0.54 / 2.17 |
| acknowledgment heard, turns that had one | 2.17 / 2.66 | 1.49 / 2.65 | 2.17 / 2.66 | 1.58 / 2.69 |
| reply | 3.13 / 4.38 | 2.60 / 4.11 | 3.19 / 3.99 | 2.57 / 5.01 |

This is where the grace was the wait, and where shortening it shows: a plain reply 2.08 s
to 1.48 s, a lookup 3.47 s to 2.58 s, every voice turn 3.13 s to 2.57 s. The lookup's
gain is the held answer no longer being thrown away: 32 of 51 were made again at a tool
call before, 9 after (the turns that end on a number or that Smart Turn was less sure
of). Its tool call still waits until 1.2 s after the VAD's stop, as it did. Rendering
ahead alone (`2 prerender`) takes a plain reply from 2.08 s to 1.47 s too, because with
a 1.2 s grace the clause is rendered inside it; with the short grace there is little
hold left to render in, and the two together are no faster than either. `confirm_plain`
("Close infra 17.") does not move: it ends on a number and keeps its 1.7 s.

## Before and after, no model time (`instant`)

| scenario | before instant | 1+2 instant |
|---|---|---|
| greeting | 1.85 / 2.47 | 0.67 / 0.74 |
| small_talk | 2.24 / 2.25 | 1.15 / 1.21 |
| lookup | 2.33 / 2.34 | 2.09 / 2.15 |
| chain | 2.81 / 2.89 | 3.02 / 3.32 |
| announce | 2.08 / 2.08 | 2.23 / 2.38 |
| confirm_plain | 2.70 / 2.84 | 2.60 / 2.64 |
| confirm_yes | 1.86 / 2.22 | 1.39 / 2.13 |
| bye_shortcut | 0.78 / 0.87 | 0.69 / 0.76 |
| bye_model | 1.81 / 1.94 | 1.81 / 1.84 |
| session | 2.12 / 2.62 | 2.11 / 2.67 |
| lookup_pause | 2.18 / 2.32 | 2.34 / 2.78 |
| every voice turn | 2.13 / 2.79 (n=45) | 2.09 / 2.90 (n=45) |
| runs with a problem | 0/33 | 0/33 |

| stage, p50 / p95 | before instant | 1+2 instant |
|---|---|---|
| hear (end of speech to the turn released) | 1.46 / 1.97 | 0.62 / 1.97 |
| of it the hold | 1.20 / 1.70 | 0.35 / 1.70 |
| of it the read | 0.17 / 0.26 | 0.18 / 0.28 |
| model, all calls on the path | 0.00 / 0.00 | 0.33 / 0.85 |
| voice (last model event to Ghost heard) | 0.61 / 1.06 | 0.61 / 1.42 |
| acknowledgment heard, turns that had one | 1.47 / 1.98 | 1.47 / 1.98 |
| reply | 2.13 / 2.79 | 2.09 / 2.90 |

With no model time a plain reply goes from 2.24 s to 1.15 s, and the greeting's answer
from 1.85 s to 0.67 s. That is the new floor under any model: the VAD's 0.2 s, the read
and the short grace (0.62 s to the turn's release), and Kokoro's first clause, 0.6 s for
an eight-word sentence and far less for a short opener like "Doing well." rendered ahead. Turns that go to an
app do not move here: their call waits out the 1.2 s either way.

## What the quick paths cost

Counted over every turn of the runs above, spoken or not (a turn that was reopened is a
record too):

| config | turns spoken | answers started early | kept | withdrawn | redone at a tool | clauses rendered ahead | played | dropped | reopened unheard | reopened after Ghost began | hand-overs | mean s to hand over | closes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| before sonnet | 90 | 101 | 77 | 24 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | - | grace 67, number 12 |
| 1 grace sonnet | 89 | 101 | 83 | 18 | 0 | 0 | 0 | 0 | 6 | 0 | 0 | - | confident 63, grace 9, number 12 |
| 2 prerender sonnet | 88 | 98 | 75 | 23 | 0 | 71 | 71 | 0 | 0 | 0 | 0 | - | grace 64, number 12 |
| 1+2 sonnet | 90 | 101 | 84 | 17 | 0 | 72 | 72 | 0 | 6 | 0 | 0 | - | confident 65, grace 7, number 12 |
| 3 fast sonnet | 89 | 101 | 76 | 25 | 0 | 0 | 0 | 0 | 0 | 0 | 59 | 0.67 | grace 66, number 12 |
| all sonnet | 90 | 99 | 82 | 17 | 0 | 72 | 72 | 0 | 6 | 0 | 65 | 0.67 | confident 63, grace 9, number 12 |
| before haiku | 45 | 51 | 10 | 9 | 32 | 0 | 0 | 0 | 0 | 0 | 0 | - | grace 32, number 7 |
| 1 grace haiku | 45 | 51 | 33 | 9 | 9 | 0 | 0 | 0 | 3 | 0 | 0 | - | confident 33, grace 3, number 6 |
| 2 prerender haiku | 45 | 51 | 10 | 9 | 32 | 36 | 36 | 0 | 0 | 0 | 0 | - | grace 33, number 6 |
| 1+2 haiku | 45 | 51 | 33 | 9 | 9 | 36 | 36 | 0 | 3 | 0 | 0 | - | confident 33, grace 3, number 6 |
| before instant | 45 | 51 | 9 | 2 | 40 | 0 | 0 | 0 | 0 | 0 | 0 | - | grace 33, number 6 |
| 1+2 instant | 45 | 51 | 9 | 3 | 39 | 36 | 36 | 0 | 3 | 0 | 0 | - | confident 33, grace 3, number 6 |

- **Answers started early and withdrawn** did not rise: 24 before and 17 after under
  `sonnet`, 9 and 9 under `haiku`. Each is a model call thrown away, as before.
- **Turns reopened** are the pause scenario's and nothing else's: its first half ("What's
  waiting for triage?") closes on the short grace in every run, the operator goes on
  0.7 s later, and the turn is reopened with nothing heard, read whole, and answered
  once. Before, the grace held it open. No turn was reopened after Ghost's voice had
  begun: the harness's operator never talks over Ghost.
- **Clauses rendered ahead** were all played. None was dropped in these scenarios: the
  reopened half's answer was a tool call with no words. A test holds the drop.
- **Hand-overs:** 65 of 90 spoken turns with a fast model in front, 0.67 s each to ask.
  No run spoke a word of the fast model's on a hand-over.

## The cut-off safety number

The shorter grace is only as good as what it does to a pause mid-sentence. `make
perf-turns` ([the hear harness](../hear-harness.md)) runs the operator's recorded takes of
2026-11-06 through the real VAD, Parakeet and Smart Turn and applies the close's rule to
every segment's end: 49 spoken takes, each one turn as he meant it, 24 of them in pieces,
39 pauses mid-turn between the pieces (p50 0.68 s, longest 1.40 s).

| | one grace, 1.2 s | short grace 0.35 s at 0.9 or surer |
|---|---|---|
| Pauses mid-turn the wait was shorter than | 1 of 39 | 3 of 39 |
| ... of them, closed by the short grace | 0 | 3 |
| ... still closed 0.8 s after the VAD's stop | 1 | 2 |
| ... still closed 1.2 s after the VAD's stop | 1 | 1 |
| Takes with a pause closed early | 1 of 49 | 3 of 49 |
| Wait at a turn's real end, mean / p50 | 1.37 / 1.20 s | 0.85 / 0.35 s |
| Real ends that got the short grace | 0 | 30 of 49 |

So the shorter grace closes two more pauses in 39 too soon than the one grace did (0.80 s
and 0.90 s; the third, 1.32 s, the 1.2 s grace also closed). A turn closed too soon is not
yet a turn cut off: if the operator goes on before Ghost's voice starts, it is reopened
whole. With the main model alone the voice starts 2 s or more after the VAD's stop, after
every one of these pauses. With a fast model in front it starts about 1.2 s after: one
pause still open then, the same one as before. Of the 39 pauses, Smart Turn called 20 not
done; of the 19 it called done, the words stopped mid-thought at 3, ended on a number at 7
and had no stop written at 3, and all of those keep their long waits. Six were left, four
at 0.9 or surer.

The same rule on Kokoro's rendered takes (three voices, 111 takes, 65 pauses) closes 12
pauses early against none, none of them still closed at 1.2 s. Kokoro speaks each half of
a paused line as a finished sentence, which the operator's voice does not: this is the
harsh case, and it is the one `make perf`'s `lookup_pause` scenario plays.

The sweep in the harness's report says what the two thresholds trade: at 0.5 or surer
the short grace reaches 36 of 49 real ends and closes 4 pauses early; at 0.98 it reaches
17 and closes 2; a short grace of 0.2 s closes 4 at 0.9, and one of 0.8 s closes 2 and
buys a quarter of a second.

## How Ghost hears

`make perf-hear` before and after, the speech-in stage against its baseline (the stage
itself is not changed):

| | before | after |
|---|---|---|
| Recorded takes, `stage` word error rate | 8.4% | 8.4% |
| Recorded takes, `turn` (what the model gets for a split turn) | 4.2% | 4.2% |
| Recorded takes, empty reads, `stage` / `turn` | 1 / 0 of 49 | 1 / 0 of 49 |
| Kokoro's takes, `stage` / `turn` | 3.0% / 2.1% | 3.0% / 2.1% |
| Pass rule | | PASS, both |

What could have changed what the model hears is the reopened turn: closed after its
first half, its second half used to be read alone. The turn's tape now runs on across the
close, so a reopened turn is read whole like any turn with a pause. A test in `turns`
holds it, and the pause scenario run with the debug log shows it in the real pipeline:
the first half closes after the short grace, the operator goes on, `turn: reopened,
nothing of the answer heard`, then `turn: read whole (4.5s)` with both halves in one
sentence. The same change gave every turn after a session's first its pre-roll back: the
tape was left open after a close, so the next turn's whole read began at the VAD's start
and not the second before it.

## What is not measured here

- The real model: its first token, how it streams, and whether a small model answers
  plainly when it should and hands over when it should. The harness's fast model always
  does the right thing. `make perf-llm` measures the connection; the rubric measures the
  conversation.
- The real microphone: the echo canceller, the room, music. The rule that words soon
  after Ghost's voice began reopen the turn (`hear.reopened: started`) never fires in
  the harness, because its operator never talks over Ghost; tests hold it, and the
  rubric's new line B5 is where it is seen.
- The cloud voice. Rendering ahead is Kokoro's only, and off by default for a paid voice.
- The phone.

## Where this leaves a reply

With the main model alone a plain reply is where it was, about 2.4 s in the harness and
2.0 to 2.5 s for the real model's 1.1 to 1.5 s to a first token: the grace no longer
hides the model, and the model is the wait. With a fast model in front, or a quick model
as the only one, the harness says 1.5 to 1.7 s, and a real small model at 0.6 s to its
first token should land near 1.6 s, later when its answer opens with a long clause. That
is inside the 1.5 s budget only on a good day, and it is still half a second from the
1.0 to 1.3 s of the speech-to-speech models and twice the 0.8 s the best pipelines
publish.

What is left of a 1.5 s plain reply, in the order it is spent:

| | s | what would cut it |
|---|---|---|
| the VAD's stop | 0.20 | Smart Turn deciding on less silence |
| the read | 0.18 | a streaming read, so the model starts as the speech ends and not after it |
| the model's first token | 0.45 to 0.6 | the model; nothing in Ghost |
| writing the first clause | 0.1 | |
| Kokoro's first clause | 0.3 to 0.6 | a first chunk shorter than a clause, or a voice that streams |

The turn's grace is no longer on the list. Kokoro's render is now the largest part Ghost
owns: it renders a whole clause before any of it plays, 0.23 s per second of speech.

A turn that goes to an app is further off: two calls of the main model, and with a fast
model its call in front of them. Letting the fast model's own read run (ADR 0050's first
rejected option) would take a main-model call off a lookup instead of adding a fast one,
and is the next thing worth measuring with a real model. A held answer that reaches a
tool call inside the grace is still made again at the close (9 of 51 with a quick model);
keeping it instead would save that call.
