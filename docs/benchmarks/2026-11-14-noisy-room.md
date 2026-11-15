# 2026-11-14: a session in a noisy room

Reported while testing 2026.11.1 with other people around a laptop: "once they respond,
they are picking up all the background noise." Ghost answered things nobody said to it,
couldn't tell when the person had finished, and the session didn't end. This is the
measurement behind [ADR 0072](../adr/0072-a-desk-session-tells-the-person-from-the-room-by-level.md):
a baseline on main, where each false turn came from, and the fix.

A laptop's built-in microphone is not the hardware Ghost is tested with (the reSpeaker
XVF3800 array, with the speakers on its output). The fix is scoped to what is small and
safe, leaves the array exactly as it was, and the numbers here are also meant to size a
later decision on voice recognition, single-microphone noise handling, push-to-talk and
recommended hardware.

## Method

[The room harness](room-harness.md) (`make perf-room`): 126 scenes, each a whole session
through `build_session` with the real Silero, Parakeet, Smart Turn, echo filter, turn rule,
idle clock and Kokoro, in real time, four at a time. Six rooms (quiet; steady noise; one
talker across the room; four at once; one who mentions Ghost; music with a singer) at
20, 12, 6 and 3 dB between the person's active speech level and the room's, Ghost's
voice through headphones or back through a canceller's residual, three scripts (a
follow-up right away and a quieter one 9 s later; an interruption and a one-word "Stop.";
woken with the request in the same breath and nothing more). Every voice is Kokoro's,
every room sound synthetic and seeded; nothing is a recording.

Both runs were made on the operator's workstation (12 cores) while other agents built
and tested beside them; at times the scripted microphone fell 1–6 s behind real time
(`perf: scripted mic … behind`), which makes turn timing worse in both runs alike. A
scene run twice can differ by a few turns; read the rows, not single cells.

- Before: main at `c6c0dacd`, `.scratch/perf/room-baseline/`.
- After: this branch, `--set room_gate=on` (as a laptop's session has it), `.scratch/perf/room-after/`.

`python3 docs/benchmarks/scripts/room_compare.py BEFORE/results.json AFTER/results.jsonl`
renders the comparison below.

## Baseline (main)

Every room, at every ratio, both paths and all three scripts per row (from `report.md`):

| room | SNR | path | false turns | of them act | well below / as loud | opened for nothing | false cuts | lines lost | WER | stops and interruptions cut | session ended after |
|---|---|---|---|---|---|---|---|---|---|---|---|
| quiet | - | headphones | 1 | 0 | 0/1 | 0 | 0 | 0/6 | 5.0% | 2/2 | 45s, 45s |
| quiet | - | speakers | 3 | 0 | 3/0 | 2 | 7 | 0/6 | 50.0% | 1/2 | 51s, 45s |
| noise | 20 dB | headphones | 0 | 0 | 0/0 | 0 | 0 | 0/6 | 4.9% | 2/2 | 45s, 45s |
| noise | 20 dB | speakers | 4 | 2 | 2/2 | 0 | 3 | 0/6 | 5.0% | 1/2 | 45s, 48s |
| noise | 12 dB | headphones | 0 | 0 | 0/0 | 0 | 0 | 0/6 | 7.3% | 2/2 | 45s, 45s |
| noise | 12 dB | speakers | 0 | 0 | 0/0 | 1 | 2 | 0/6 | 7.3% | 1/2 | 45s, 45s |
| noise | 6 dB | headphones | 0 | 0 | 0/0 | 0 | 0 | 0/6 | 7.3% | 2/2 | 45s, 45s |
| noise | 6 dB | speakers | 0 | 0 | 0/0 | 1 | 1 | 0/6 | 9.8% | 1/2 | 45s, 45s |
| noise | 3 dB | headphones | 0 | 0 | 0/0 | 2 | 0 | 1/6 | 29.3% | 2/2 | 34s, 45s |
| noise | 3 dB | speakers | 1 | 0 | 0/1 | 0 | 0 | 2/6 | 46.3% | 2/2 | 45s, 45s |
| talker | 20 dB | headphones | 53 | 45 | 49/4 | 2 | 29 | 1/6 | 600.0% | 0/2 | never, never |
| talker | 20 dB | speakers | 56 | 55 | 49/7 | 3 | 6 | 0/6 | 650.0% | 0/2 | never, never |
| talker | 12 dB | headphones | 80 | 72 | 59/21 | 3 | 5 | 0/6 | 892.5% | 0/2 | never, never |
| talker | 12 dB | speakers | 57 | 48 | 37/20 | 2 | 12 | 0/6 | 1139.0% | 0/2 | never, never |
| talker | 6 dB | headphones | 62 | 55 | 0/62 | 2 | 12 | 3/6 | 968.3% | 0/2 | never, never |
| talker | 6 dB | speakers | 62 | 60 | 0/62 | 2 | 5 | 1/6 | 650.0% | 0/2 | never, never |
| talker | 3 dB | headphones | 89 | 81 | 0/89 | 3 | 4 | 3/6 | 970.0% | 0/2 | never, never |
| talker | 3 dB | speakers | 83 | 80 | 0/83 | 4 | 2 | 4/6 | 447.5% | 0/2 | never, never |
| babble | 20 dB | headphones | 42 | 37 | 30/12 | 5 | 4 | 4/6 | 222.5% | 1/2 | never, never |
| babble | 20 dB | speakers | 27 | 24 | 21/6 | 6 | 5 | 4/6 | 317.1% | 0/2 | never, 56s |
| babble | 12 dB | headphones | 23 | 20 | 6/17 | 6 | 2 | 5/6 | 132.5% | 0/2 | 20s, never |
| babble | 12 dB | speakers | 35 | 35 | 11/24 | 5 | 3 | 3/6 | 1270.0% | 0/2 | 38s, never |
| babble | 6 dB | headphones | 19 | 19 | 0/19 | 3 | 1 | 5/6 | 255.0% | 0/2 | 20s, never |
| babble | 6 dB | speakers | 5 | 5 | 0/5 | 3 | 2 | 6/6 | 100.0% | 0/2 | 20s, never |
| babble | 3 dB | headphones | 5 | 3 | 0/5 | 3 | 2 | 6/6 | 100.0% | 0/2 | never, 47s |
| babble | 3 dB | speakers | 4 | 0 | 0/4 | 2 | 2 | 6/6 | 100.0% | 0/2 | 20s, 45s |
| name | 20 dB | headphones | 50 | 21 | 43/7 | 1 | 17 | 0/6 | 348.8% | 0/2 | never, 21s |
| name | 20 dB | speakers | 64 | 36 | 57/7 | 2 | 14 | 1/6 | 610.0% | 0/2 | never, never |
| name | 12 dB | headphones | 62 | 23 | 42/20 | 4 | 5 | 1/6 | 217.5% | 0/2 | never, never |
| name | 12 dB | speakers | 43 | 35 | 35/8 | 2 | 2 | 0/6 | 1324.4% | 0/2 | never, never |
| name | 6 dB | headphones | 69 | 55 | 0/69 | 3 | 2 | 0/6 | 2150.0% | 0/2 | never, never |
| name | 6 dB | speakers | 72 | 59 | 0/72 | 5 | 0 | 0/6 | 2005.0% | 0/2 | never, 68s |
| name | 3 dB | headphones | 91 | 82 | 0/91 | 4 | 0 | 0/6 | 2472.5% | 0/2 | never, never |
| name | 3 dB | speakers | 91 | 48 | 1/90 | 3 | 9 | 1/6 | 1117.5% | 0/2 | never, never |
| music | 20 dB | headphones | 96 | 4 | 86/10 | 3 | 8 | 2/6 | 645.0% | 0/2 | never, never |
| music | 20 dB | speakers | 85 | 18 | 75/10 | 3 | 5 | 2/6 | 1300.0% | 0/2 | never, never |
| music | 12 dB | headphones | 104 | 19 | 47/57 | 4 | 1 | 3/6 | 856.1% | 0/2 | never, never |
| music | 12 dB | speakers | 146 | 36 | 76/70 | 6 | 0 | 2/6 | 2137.5% | 0/2 | never, never |
| music | 6 dB | headphones | 122 | 5 | 0/122 | 4 | 2 | 1/6 | 2282.9% | 0/2 | never, never |
| music | 6 dB | speakers | 128 | 6 | 0/128 | 4 | 6 | 4/6 | 987.5% | 1/2 | never, never |
| music | 3 dB | headphones | 90 | 17 | 0/90 | 4 | 5 | 2/6 | 612.5% | 0/2 | never, never |
| music | 3 dB | speakers | 92 | 0 | 0/92 | 3 | 5 | 4/6 | 415.0% | 0/2 | never, never |
| **all** | | | **2116** | **1105** | **729/1387** | **115** | **190** | **77/252** | **676.9%** | **18/84** | **54 of 84 never ended** |

## Where the false turns came from

Read from the pipeline's own debug decisions (`GHOST_DEBUG_AUDIO=1` on the harness,
`--set debug_audio=1`; synthetic voices only), in order of how much they mattered:

1. **A turn opens on any voice the VAD hears** (`turns::BargeInUserTurnStartStrategy`:
   `VadStarted` while Ghost is quiet opens the turn, interrupts a reply still being made,
   and Parakeet then reads the room's words into it). Silero's volume gate is a fixed
   −50 LUFS (`GHOST_VAD_MIN_VOLUME`, 0.6), which a talker across the room clears at every
   ratio here. Over Ghost's voice the room's interims cut it off in two words: 190 false
   cuts, and only 18 of 84 stops and interruptions that were the person's landed.
2. **The room goes on under the person**, so the VAD does not stop when they do: the
   person's line and the room's speech become one segment, the turn is read whole with the
   room's words in it (ADR 0044), and the room reopens it before its answer is heard
   (`turn: reopened, nothing of the answer heard`). Hence WERs over 100% (inserted words)
   and lines lost whole.
3. **The idle clock** (`turns.rs`, the aggregator's `arm_idle`): a turn's start stops it
   and only a reply starts it again. A turn that closed with no words left it stopped for
   good. In these scenes the room's segments mostly had words, were answered, and the
   reply restarted the clock legitimately, so this mattered less here than its unit tests
   show; it is the whole story wherever room sound reads as nothing (fans, music without
   words, clatter).
4. **Parakeet reading babble into words** is real (`noise` rows: almost nothing, since
   steady noise rarely opens the VAD and reads as nothing when it does), but it is a
   consequence of 1, not a separate cause.
5. **The echo path** adds a few turns and cuts (speakers against headphones in the quiet
   and noise rows), as before; nothing here changes it.

The split the owner asked for, false turns from sound **well below** the person (the
loudest segment read for the turn 10 dB or more under the person's) against sound **as
loud**: 729 and 1,387 at baseline. Per room, the "well below" ones are almost all at 20 and
12 dB, the "as loud" ones at 6 and 3 dB (and music's at 12, whose beats peak high).

## After

What changed (ADR 0072): the idle clock no longer stops for good on a wordless turn; on a
microphone other than the tested array, the speech-in stage holds back a segment under
the person's level and no louder than the room, the person's level seeded from the
segment that woke Ghost; the person's segment ends when they go quiet under the room; a
held segment goes on if it begins with Ghost's name. Before → after, every row both paths:

| room | SNR | false turns | of them act | well below | as loud | opened for nothing | false cuts | lines lost | WER | stops and interruptions cut | never ended |
|---|---|---|---|---|---|---|---|---|---|---|---|
| quiet | - | 4 → 4 | 0 → 0 | 3 → 4 | 1 → 0 | 2 → 1 | 7 → 6 | 0/12 → 0/12 | 28% → 5% | 3/4 → 2/4 | 0/4 → 0/4 |
| noise | 20 dB | 4 → 2 | 2 → 0 | 2 → 0 | 2 → 2 | 0 → 0 | 3 → 0 | 0/12 → 0/12 | 5% → 5% | 3/4 → 4/4 | 0/4 → 0/4 |
| noise | 12 dB | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 | 1 → 0 | 2 → 0 | 0/12 → 0/12 | 7% → 7% | 3/4 → 3/4 | 0/4 → 0/4 |
| noise | 6 dB | 0 → 2 | 0 → 1 | 0 → 0 | 0 → 2 | 1 → 0 | 1 → 1 | 0/12 → 1/12 | 9% → 17% | 3/4 → 2/4 | 0/4 → 0/4 |
| noise | 3 dB | 1 → 1 | 0 → 1 | 0 → 0 | 1 → 1 | 2 → 0 | 0 → 0 | 3/12 → 0/12 | 38% → 15% | 4/4 → 4/4 | 0/4 → 0/4 |
| talker | 20 dB | 109 → 10 | 100 → 7 | 98 → 9 | 11 → 1 | 5 → 1 | 35 → 6 | 1/12 → 0/12 | 625% → 73% | 0/4 → 2/4 | 4/4 → 2/4 |
| talker | 12 dB | 137 → 47 | 120 → 32 | 96 → 25 | 41 → 22 | 5 → 6 | 17 → 17 | 0/12 → 2/12 | 1017% → 69% | 0/4 → 1/4 | 4/4 → 4/4 |
| talker | 6 dB | 124 → 115 | 115 → 97 | 0 → 0 | 124 → 115 | 4 → 4 | 17 → 37 | 4/12 → 4/12 | 811% → 439% | 0/4 → 0/4 | 4/4 → 4/4 |
| talker | 3 dB | 172 → 125 | 161 → 114 | 0 → 0 | 172 → 125 | 7 → 5 | 6 → 34 | 7/12 → 5/12 | 709% → 877% | 0/4 → 0/4 | 4/4 → 4/4 |
| babble | 20 dB | 69 → 3 | 61 → 2 | 51 → 1 | 18 → 2 | 11 → 4 | 9 → 1 | 8/12 → 3/12 | 270% → 167% | 1/4 → 4/4 | 3/4 → 0/4 |
| babble | 12 dB | 58 → 4 | 55 → 2 | 17 → 0 | 41 → 4 | 11 → 8 | 5 → 1 | 8/12 → 8/12 | 701% → 204% | 0/4 → 1/4 | 2/4 → 1/4 |
| babble | 6 dB | 24 → 18 | 24 → 18 | 0 → 0 | 24 → 18 | 6 → 3 | 3 → 5 | 11/12 → 11/12 | 178% → 140% | 0/4 → 0/4 | 2/4 → 2/4 |
| babble | 3 dB | 9 → 25 | 3 → 16 | 0 → 0 | 9 → 25 | 5 → 3 | 4 → 1 | 12/12 → 11/12 | 100% → 226% | 0/4 → 0/4 | 1/4 → 2/4 |
| name | 20 dB | 114 → 6 | 57 → 1 | 100 → 4 | 14 → 2 | 3 → 4 | 31 → 1 | 1/12 → 2/12 | 478% → 64% | 0/4 → 3/4 | 3/4 → 0/4 |
| name | 12 dB | 105 → 21 | 58 → 3 | 77 → 14 | 28 → 7 | 6 → 3 | 7 → 6 | 1/12 → 0/12 | 778% → 32% | 0/4 → 4/4 | 4/4 → 3/4 |
| name | 6 dB | 141 → 101 | 114 → 28 | 0 → 0 | 141 → 101 | 8 → 4 | 2 → 71 | 0/12 → 1/12 | 2078% → 591% | 0/4 → 1/4 | 3/4 → 4/4 |
| name | 3 dB | 182 → 99 | 130 → 42 | 1 → 0 | 181 → 99 | 7 → 5 | 9 → 46 | 1/12 → 3/12 | 1795% → 821% | 0/4 → 0/4 | 4/4 → 3/4 |
| music | 20 dB | 181 → 7 | 22 → 0 | 161 → 5 | 20 → 2 | 6 → 1 | 13 → 3 | 4/12 → 1/12 | 977% → 60% | 0/4 → 4/4 | 4/4 → 1/4 |
| music | 12 dB | 250 → 35 | 55 → 0 | 123 → 3 | 127 → 32 | 10 → 2 | 1 → 6 | 5/12 → 0/12 | 1489% → 151% | 0/4 → 3/4 | 4/4 → 2/4 |
| music | 6 dB | 250 → 115 | 11 → 13 | 0 → 0 | 250 → 115 | 8 → 4 | 8 → 18 | 5/12 → 1/12 | 1643% → 1139% | 1/4 → 0/4 | 4/4 → 4/4 |
| music | 3 dB | 182 → 154 | 17 → 39 | 0 → 0 | 182 → 154 | 7 → 4 | 10 → 15 | 6/12 → 7/12 | 514% → 637% | 0/4 → 0/4 | 4/4 → 4/4 |
| **all** |  | 2116 → 894 | 1105 → 416 | 729 → 65 | 1387 → 829 | 115 → 62 | 190 → 275 | 77/252 → 60/252 | 677% → 272% | 18/84 → 38/84 | 54/84 → 40/84 |

**Totals:** false turns 2,116 → 894; of them with words a model would act on 1,105 → 416;
from sound well below the person 729 → 65 (−91%); as loud 1,387 → 829; turns opened for
nothing 115 → 62; the person's lines lost 77 → 60 of 252; stops and interruptions that
cut Ghost off 18 → 38 of 84; sessions that never ended 54 → 40 of 84.

**Where it does not help, and why:** at 6 and 3 dB the room is as loud as the person, and
a level can't tell them apart: false turns stay about where they were (talker 124 → 115
and 172 → 125, music 250 → 115 and 182 → 154). False cuts rise in those rows (talker
17 → 37, name 2 → 71), because turns now close and Ghost gets to speak, so the room has
more replies to cut; and babble at 3 dB counts more false turns (9 → 25) because at
baseline it merged everything into a few endless turns (12 of 12 lines lost). These rows
are what voice recognition or push-to-talk would have to answer.

### What each part bought

The same 16 noisy scenes (talker, babble, name, music; 20 and 6 dB; headphones; `follow`
and `first`):

| | 20 dB false turns | 20 dB never ended | 6 dB false turns | 6 dB never ended |
|---|---|---|---|---|
| Baseline | 148 | 7/8 | 176 | 7/8 |
| Idle clock alone | 124 | 8/8 | 109 | 6/8 |
| Idle clock and the room gate | 4 | 1/8 | 145 | 7/8 |

The gate does the work where the room is well below the person; nothing here does it where
the room is as loud. (6 dB rows move within the run-to-run noise.)

### The quiet room, the tested setup, the rest

- **Quiet room, gate on and off**, three repeats each, sequential, headphones, `barge` and
  `follow`: 5/6 stops cut, no false turns, no lost lines, every session ended 45 s after
  Ghost's last words, both ways (WER 6.6% and 5.7%: one word). The quiet rows of the big
  table moved within the load's noise.
- **The tested reSpeaker XVF3800** runs exactly as before: the gate is off on the on-chip
  case (`microphone::Plan::room_gate`, tested), and the idle-clock change only stops room
  sound that came to nothing from keeping a session open.
- **Cost:** 1.9 µs per 20 ms frame (`room::tests::what_the_room_gate_costs`, release);
  nothing waits on a turn at the person's level; a held segment of 0.3 s or more costs one
  Parakeet read for the name.
- **Unchanged and passing:** every workspace test (1,921), the session tests (barge-in
  with one-word stops, hold and resume, the echo filter, the fast model), the Hearing
  test's scorer and plans, the wake's tests. The wake path is untouched except that the
  transcript wake's stage now remembers the last segment's level, as a number, for the
  session it opens.

## What was tried and dropped

- **Judging a segment on its first 0.2 s** (the VAD's start): the room's loudest syllables
  let a talker 12 dB under the person through about one segment in three. Judged on 0.4 s,
  or on all of it when shorter, so "Stop." still counts; a segment clearly at the person's
  level still opens at once.
- **The 90th percentile as a segment's level** (to read past the VAD's pre-roll in the
  wake's segment): too peaky on 0.4 s of the room. The 75th, with the pre-roll left out of
  the wake's segment instead.
- **A line at the person's level less a margin, alone:** in a quiet room a quieter
  follow-up nine seconds later measured 9 dB under the person and was held. The line is now
  never over the room's own loud level plus 3 dB, so a quiet room holds nothing.
- **Learning the person's level from every segment let in:** the room, let in on the wide
  margin, took it down 5 dB in a minute. It follows only segments within 3 dB of it, never
  more than 6 dB under the loudest.
- **Seeding nothing, learning in the session only:** the room talked over the greeting
  before the person had said a word and became the person's level.

## Left out on purpose

- **Noise suppression** (RNNoise, DeepFilterNet, a spectral gate): steady noise caused
  almost nothing here (the `noise` rows, 9 false turns at baseline across 32 scenes), and
  suppression keeps speech by design. It would add a model to the audio path for nothing
  these numbers show.
- **Voice recognition:** what it could buy is the "as loud" column after the fix: 829
  false turns, 752 of them at 6 and 3 dB.
- **Seeding the person's level from the trained detectors' wake** (`GHOST_WAKE_MODE=onnx`):
  that path is the tested array's, where the gate is off.
- **Gating what the realtime voice's provider hears** (ADR 0063): its uplink sits before
  this stage; the provider still hears the room. Parakeet's transcript, the intents, the yes
  and the agent are gated.

## What needs a live check

[The voice rubric's Part I](../voice-rubric.md): five minutes at a laptop with two people
talking across the room, `GHOST_DEBUG_AUDIO=1` in a terminal. The synthetic room has no
microphone gain control, no Lombard effect and nobody moving; the real one does.

