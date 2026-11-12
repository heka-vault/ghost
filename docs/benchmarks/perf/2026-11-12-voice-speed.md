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

## Second round: real models, first audio sooner, the fast model reads

Run the same day, on branch `feat/voice-speed-2`. The first section's numbers came from a
scripted model with assumed delays; this one starts from the real models and then goes
after what is left. Load averages are given beside each run; other sessions were building
on the workstation, and a run taken while a build of this branch was going says so.

### The answer to the owner's question

End of the operator's speech to Ghost's voice, the real models through the default
connection, the mock GitHub app, everything else as it ships on this branch:

| | plain spoken reply p50 / p95 | reply after one lookup p50 / p95 |
|---|---|---|
| (a) main model alone (Sonnet 5.5) | 2.62 / 2.86 s (real, n=8) | 5.67 / 8.07 s (real, n=10) |
| (b) fast model in front (Haiku 5.5), reading for itself, as it ships | 1.67 / 2.06 s (real, n=8) | 3.45 / 4.19 s (real, n=10) |
| (b') fast model in front, every call handed over (ADR 0050 as first written) | 2.05 / 2.33 s (real, n=8) | 6.94 / 10.53 s (real, n=10) |
| the bar: good pipelines | under 0.8 s | |
| the bar: speech-to-speech products | 1.0 to 1.3 s | |

Real means the real models, measured: `ghost-perf run --llm-url ... --llm-model
claude-sonnet-5-5 --llm-fast-model claude-haiku-5-5`, two passes of two runs a scenario
(load 1.5 to 5, then about 2). A plain reply is the greeting and small-talk scenarios; a
reply after one lookup is the lookup scenarios' answers. With the scripted model at the
measured times (`sonnet55`, `haiku55`, below; three runs a scenario) the same cells are
2.52 / 2.71 s and 6.01 / 6.25 s for (a), 1.38 / 1.51 s and 2.93 / 3.19 s for (b), and
1.29 / 1.64 s and 2.87 / 3.06 s for (b) with the early read on.

So a plain reply is about 1.7 s with a fast model in front and 2.6 s without one, and
neither meets either bar. A reply after a lookup is 3.5 s with the fast model reading,
down from 5.7 s with the main model alone.

### What was built

1. **The harness drives a real model** (`ghost-perf run --llm-url URL --llm-model M
   [--llm-fast-model M] [--llm-key-env NAME]`, `fast=real` in a variant). It reaches the
   address as Ghost's default connection does (`custom`, the Anthropic protocol), with no
   vault and no settings file; the key is read from the one variable named and never
   printed. The default connection, when no provider is set, is `ANTHROPIC_BASE_URL` with
   `ANTHROPIC_AUTH_TOKEN` (`ghost_core::connection::ModelConnection::from_env`); on this
   machine that is a proxy on the operator's network, which lists 39 models on
   `/v1/models`, Haiku 5.5 among them. The lines now also check the arguments of their
   calls against what they expect, from the run's own agent record. `sonnet55` and
   `haiku55` are scripted profiles at the measured times.
2. **The fast model is asked without the thinking setting.** Haiku 5.5 answers
   `between_tools`, Ghost's default, with HTTP 400 ("not supported for this model"), so with
   it as the fast model every turn failed in about 0.3 s and was handed over.
3. **A reply's first piece may be cut early** (`GHOST_TTS_FIRST_CLAUSE_WORDS`, 4): before
   a phrase's first word ("and", "but", "so", "because", ...) once four words are pending,
   never after a capitalized word or a number.
4. **The read at the VAD's first quiet** (`GHOST_STT_EARLY_READ`, off): built, measured,
   not turned on.
5. **The fast model reads for itself** (`GHOST_LLM_FAST_READS`, on whenever a fast model
   is set; `GHOST_LLM_FAST_READ_MAX_CHARS` 4,000, `GHOST_LLM_FAST_READ_ROUNDS` 2).
6. **The VAD's stop**: measured, not changed.

### Step 1: the real models

`ghost-perf llm --endpoint proxy --model claude-sonnet-5-5,claude-haiku-5-5 --n 6`
(Ghost's prompt and tools, six rounds, load 3.8):

| model | hello first token p50 / p95 | streamed | tool call complete p50 / p95 | answer after the result, first token | tool chosen |
|---|---|---|---|---|---|
| Sonnet 5.5 (thinking `between_tools`) | 1.73 / 1.91 s | all at once (88 tokens in 2 ms) | 3.12 / 3.12 s | 1.70 s | 1 of 6 |
| Haiku 5.5 (no thinking) | 0.52 / 0.54 s | about 290 tokens/s | 1.11 / 1.69 s | 0.51 s | 4 of 6 |

Through Ghost's own pipeline the main model's first token was 1.4 to 1.8 s (p50 by
scenario, 84% of the prompt from the cache), and a reply that opens with a call took 2.3
to 3.2 s to the call. The assumed `sonnet` profile (1.2 s, 60 tokens/s) was optimistic by
half a second on words and by a second and more on calls; the assumed `haiku` (0.45 s) was
close on words and optimistic on calls (1.1 s, not about 0.55).

Every scenario, real models, p50 / p95 over both passes:

| scenario | main | fast, hand-over | fast, reads |
|---|---|---|---|
| greeting | 2.50 / 2.76 (n=4) | 1.63 / 2.12 (n=4) | 1.61 / 1.68 (n=4) |
| small_talk | 2.72 / 2.86 (n=4) | 2.09 / 2.33 (n=4) | 1.77 / 2.06 (n=4) |
| lookup | 5.36 / 5.84 (n=4) | 6.64 / 7.18 (n=4) | 3.35 / 3.70 (n=4) |
| lookup_pause | 7.33 / 8.07 (n=4) | 8.41 / 10.53 (n=4) | 3.69 / 4.19 (n=4) |
| chain | 7.05 / 8.21 (n=4) | 8.47 / 9.78 (n=4) | 3.84 / 4.02 (n=4) |
| announce | 12.03 / 12.61 (n=2) | 11.59 / 13.43 (n=2) | 11.00 / 13.04 (n=2) |
| confirm_plain | 6.45 / 7.06 (n=2) | 6.04 / 6.31 (n=2) | 6.04 / 6.11 (n=2) |
| bye_model | 3.22 / 3.23 (n=2) | 4.01 / 4.21 (n=2) | 4.02 / 4.11 (n=2) |
| session (four turns) | 3.90 / 6.11 (n=8) | 4.67 / 7.08 (n=8) | 3.14 / 5.12 (n=8) |
| every voice turn | 5.15 / 8.21 (n=34) | 6.36 / 9.78 (n=34) | 3.45 / 6.11 (n=34) |

The acknowledgment before a lookup was heard at 3.2 s with the main model alone and 2.0 s
with a fast model in front. The change scenarios (`announce`, `confirm_plain`) are slow
for every configuration: the main model looks the issue up before it writes, two to four
calls of 1.5 to 2.5 s each. A hand-over took 1.35 s on average (22 of them), not the 0.67 s
assumed: the fast model writes a call before it can hand over.

The same, scripted at the measured times (`--profile sonnet55`, fast `haiku55`, three
runs, load 1 to 6, a debug build of this branch's tests running for the first minutes):

| scenario | main | fast, hand-over | fast, reads | fast, reads, early read |
|---|---|---|---|---|
| greeting | 2.36 / 2.40 | 1.26 / 1.32 | 1.16 / 1.26 | 1.16 / 1.18 |
| small_talk | 2.66 / 2.70 | 1.49 / 1.50 | 1.49 / 1.51 | 1.61 / 1.64 |
| lookup | 5.85 / 6.02 | 7.08 / 7.22 | 2.74 / 2.92 | 2.87 / 2.89 |
| lookup_pause | 6.02 / 6.23 | 7.26 / 7.43 | 3.05 / 3.18 | 2.88 / 3.04 |
| chain | 6.46 / 6.80 | 7.81 / 7.83 | 3.87 / 3.87 | 3.79 / 3.81 |
| confirm_plain | 5.63 / 5.75 | 6.86 / 6.99 | 6.87 / 6.99 | 6.78 / 6.78 |
| every voice turn | 5.80 / 6.51 | 7.00 / 7.82 | 2.93 / 6.89 | 2.87 / 6.78 |

| stage, p50 | main | fast, hand-over | fast, reads | fast, reads, early read |
|---|---|---|---|---|
| hear (end of speech to the turn released) | 0.62 | 0.61 | 0.62 | 0.62 |
| of it the read | 0.17 | 0.17 | 0.17 | 0.09 |
| of it the hold | 0.35 | 0.35 | 0.35 | 0.35 |
| model's first token | 3.12 | 3.12 | 1.21 | 1.21 |
| voice (Kokoro to first audio) | 0.57 | 0.56 | 0.54 | 0.57 |

### Step 2: first audio sooner

Kokoro on this machine (ONNX Runtime's default threads, 12 CPUs, load about 7), words to
audio:

| words | render p50 / p95 | audio |
|---|---|---|
| 2 | 258 / 299 ms | 0.94 s |
| 4 | 336 / 350 ms | 1.41 s |
| 8 | 647 / 657 ms | 2.65 s |
| 16 | 1,298 / 1,328 ms | 5.48 s |
| 32 | 2,632 / 2,667 ms | 11.11 s |

0.24 s of render per second of speech, nearly linear, and about 0.15 s of it whatever the
length. So the first piece's words are what the operator waits for, and a first sentence
with no comma (the usual rule's only cut) waited for its whole render.

On seven measured first sentences (`perf micro --only tts-len,tts-join`, cut at three
words in this run; the default of four cuts the same two sentences that have no comma),
cutting early started the first audio 0.57 s sooner than the whole sentence on average,
and 0.15 s sooner than the usual rule, because most already had a comma after four words; the two
with no comma ("I found three issues waiting | and two of them ...", "The build failed
last night | because the cache was cold ...") started 0.50 and 0.59 s sooner. The rest of
the sentence rendered in 0.40 to 0.65 s while the first piece played for 1.4 to 2.5 s,
never late. The join, measured with nobody listening:

| at the join, mean of 7 | two pieces | the whole render |
|---|---|---|
| silence | 161 ms | 366 ms (Kokoro's own pause there) |
| loudness of the 60 ms before, against the whole | -1.3 dB | |
| loudness of the 60 ms after | -1.0 dB | |
| pitch step across the join | 6.1 semitones | 1.8 semitones |
| voice, both pieces together | 3.7 s | 4.0 s |

The pieces leave less silence than the whole render does, so nothing is inserted; the
first piece ends a little lower and quieter, as the end of a sentence does, which is the
seam every clause cut already has; and the pieces speak about 7% faster. One sentence
showed where the rule must not cut: "once you confirm | which repository" put 130 ms of
silence where Kokoro leaves none, so "which" is not a word to cut before. Whether it
sounds rushed is not something a measurement says: `GHOST_TTS_FIRST_CLAUSE_WORDS=0` is the
old rule, and the voice rubric's D2 is the operator's check. A test holds that the pieces
meet at the speaker with no gap over 40 ms when the rest renders in time, and another that
the harness sees the gap when it does not. The harness scenarios' scripted replies all
have an early comma or a short first sentence, so this change does not show in the tables
above.

### Step 3: the read overlaps the silence, measured and left off

`make perf-hear` on Kokoro's rendered takes (three voices, 141 takes; load 3 to 7):

| | stage WER | turn WER | close p95 | early reads: same / differ / thrown away |
|---|---|---|---|---|
| read at the stop (today, the default) | 3.7% | 3.0% | 0.23 s | |
| read from the first quiet frame | 3.4% | 3.0% | 0.22 s | 159 / 17 / 136 |
| ... from 0.06 s of quiet | | | | 166 / 10 / 54 |
| ... from 0.10 s of quiet | | | | 162 / 14 / 21 |
| ... from 0.14 s of quiet | | | | 166 / 10 / 11 |

The early read passes the pass rule (no measure worse beyond noise, the close no slower),
but its words are not today's: in 10 to 17 of 176 segments they differ, because the last
0.1 to 0.2 s of room quiet is replaced by silence ("the wake were" for "the wake word",
"the latest book" for "the latest bug", a comma for a full stop; and the other way,
"Stop." where the stop's read heard "Stapen."). A full stop where there was a comma
changes how the turn closes. The requirement was identical words, so it stays off;
`verify` is there to measure it on the operator's voice (rubric D4). What it would buy:
the read at the close falls from 0.17 s to 0.09 s with a 0.1 s start, about 0.1 s off a
plain reply in the scripted runs. What it costs in model time is 3.8 s over the 141 takes
for the reads thrown away with a 0.1 s start, against 21 s from the first quiet frame,
where the VAD dips between words. With the branch's defaults the speech-in path is
unchanged, so `make perf-hear` scores the first row.

### Step 4: the fast model reads for itself

With the real models, the fast model made its reads in every lookup, chain and session
turn, and handed over every change and every goodbye (`hang_up`). Correctness, from the
harness's checks over both passes:

| check | main | fast, reads |
|---|---|---|
| lookup: the list tool called, both repositories | 4 of 4 | 4 of 4 |
| lookup: the triage label as the mock names it (`needs-triage`) | 0 of 8 (it called `triage`) | 0 of 8 (the same calls) |
| chain: issue 142 and its comments in the app repository | 4 of 4 | 4 of 4 |
| session: "the dark mode one" read by number | 0 of 2 (searched instead) | 0 of 2 (the same) |
| changes made through the gate, by the main model | 4 of 4 | 4 of 4 (all handed over) |
| mock state after the change scenarios | right in every run | right in every run |

Where the main model went wrong, the fast model went wrong the same way, with the same
calls; it did not go wrong anywhere the main model was right. That is a small sample (two
passes, mock data), and lookups are what a small model handles well, so the default rests
on it only because a fast model is off unless the operator sets one.

The results it answered from were 28 to 525 characters (mock data, ten reads). The
threshold is 4,000 characters, about a thousand tokens, eight times the largest seen, so a
real app's long list is what goes to the main model. With the real models the fast model's
reads took a lookup from 5.4 s (main alone) to 3.4 s and an issue with its comments from
7.1 s to 3.8 s. A change pays the hand-over: `confirm_plain` 5.6 s main alone against 6.9 s
scripted with a fast model in front (the real runs showed no difference in two runs).

### Step 5: the VAD's stop, measured and left

Smart Turn's verdict and the words both come at the VAD's stop, so a confident end closing
on less quiet is, in close time, a shorter short grace. The operator's recorded takes are in
his data folder and were not read for this round; Kokoro's rendered takes were (111 takes,
65 pauses mid-turn, `make perf-turns` and `turns-report`):

| total quiet before a confident close | closed early | still closed when the voice starts, 0.8 s after today's stop and moving with the close | ... 1.0 s |
|---|---|---|---|
| 0.55 s (stop 0.2 + short grace 0.35; today) | 12 / 65 | 8 | 2 |
| 0.45 s (short grace 0.25, or a stop of 0.1 for sure ends) | 15 / 65 | 9 | 4 |
| 0.35 s (short grace 0.15) | 17 / 65 | 9 | 8 |
| a VAD stop of 0.1 s for every turn, short grace 0.35 | 14 / 87 | 9 (0.8 s after its stop) | |

Each step closes more pauses early and leaves more of them closed when Ghost's voice would
start, so the rule (no more pauses closed early and not reopened) says no. The stop stays
0.2 s and the short grace 0.35 s. On the operator's takes: `make perf-turns TAKES=latest`,
then `ghostd perf turns-report <run>/turns.json --set turn_confident_grace_secs=0.25`.

### What the quick paths cost

| config, real models, first pass | answers started early | kept | withdrawn | clauses rendered ahead | played | dropped | reopened unheard | hand-overs, mean s |
|---|---|---|---|---|---|---|---|---|
| main | 24 | 18 | 6 | 22 | 14 | 8 | 2 | - |
| fast, hand-over | 24 | 18 | 6 | 25 | 14 | 11 | 2 | 12, 1.35 |
| fast, reads | 24 | 18 | 6 | 20 | 14 | 6 | 2 | 6, 1.39 |

The second pass's counts were alike. Clauses dropped are mostly narration before a call,
rendered and thrown away. With the fast model reading, half as many turns are handed over
(only changes and goodbyes), and fewer clauses are dropped.

The safety numbers: the cut-off count is unchanged, because no default of the close moved
(on Kokoro's takes 12 of 65 pauses close early, 8 still closed 0.8 s after the stop, none
at 1.2 s), and so is the hearing score (3.7% stage, 3.0% turn on Kokoro's takes; the early
read, off, would be 3.4% and 3.0%).

### What is left, and 0.8 s

A plain reply with the fast model in front, 1.7 s with the real models:

| | s | what would cut it |
|---|---|---|
| the VAD's stop | 0.20 | nothing safe: a shorter one closes more pauses early (step 5) |
| the read | 0.17 | the early read, if its words can be made the stop's (0.08 to 0.17) |
| the short grace, beyond the read | 0.18 | the model already starts inside it; it costs only when the model is quicker |
| the fast model's first token | 0.5 to 0.9 | a faster model; nothing in Ghost |
| writing the first clause | 0.1 to 0.2 | |
| Kokoro's first clause | 0.25 to 0.6 | the early cut where it applies; a voice that streams |

To reach 0.8 s at the p50, the parts Ghost owns would have to fit in about 0.3 s beside a
model that answers in 0.5 s, and today they take about 1.0 s (the stop, the read, the rest
of the grace, the render). Three things would each take a large step, and none is a
setting:

- **A streaming recogniser and turn detector**, so the words and the verdict are ready as
  the speech ends rather than 0.2 to 0.4 s after it: the stop, the read and most of the
  grace go together.
- **A streaming voice**, so the first audio leaves 50 to 100 ms after the first words
  instead of after a whole clause (Kokoro renders whole; the cloud voice streams, at a
  network hop and a price).
- **A model with a first token near 0.3 s**, local or a realtime API.

A speech-to-speech front end does all three at once, which is why those products answer in
1.0 to 1.3 s. It is not the only way, but the pipeline way needs all three, and Ghost has
none of them today; the good pipelines' 0.8 s is streaming recognition, streaming
synthesis and a fast first token together. Without them Ghost's floor with a fast model is
about 1.3 to 1.5 s: the scripted `instant` model's 1.15 s plus a fast model's half second.
