# Jev vs rules for classifying short operator turns

2026-10-17, workstation, Jev `typesafe/jev-1.13-20260917` through OpenRouter.
Scripts: [`scripts/jev_make_utterances.py`](scripts/jev_make_utterances.py),
[`scripts/jev_eval.py`](scripts/jev_eval.py). Data stays local (`.scratch/jev/`).

## Question

Should Jev decide "did the operator say yes" for `confirm`-level writes (ADR 0011), and
is it worth short-circuiting sleep / cancel / repeat before the LLM?

## Data

560 utterances, 80 per intent (affirm, deny, sleep, cancel, repeat, ask, noise). Kokoro
spoke 168 phrases in two voices; a third of the clips were degraded (white noise at
8 dB SNR, or 1.35x speed); Parakeet transcribed them. So the classifier sees Parakeet's
real mis-hearings ("Thanks goes" for "thanks ghost", "Market approved" for "mark it
approved"), not clean text. Not yet validated on room audio from the operator.

## Results (second question design)

| | rules (regex) | Jev raw | Jev ≥ 0.9, else rules |
|---|---|---|---|
| accuracy, 7-way | 77.1% | 88.8% | 82.9% |
| false affirm (would execute a write) | 0 | 10 | **0** |
| false sleep (would end the session) | 1 | 1 | **0** |
| affirm recall | 0.79 | 0.99 | **0.95** |
| deny recall | 0.94 | 0.89 | 0.94 |
| sleep recall | 0.91 | 0.97 | 0.95 |
| cancel recall | 0.91 | 0.94 | 0.94 |
| repeat recall | 0.72 | 0.85 | 0.78 |
| Jev answers used | | 100% | 70% |

Latency from this workstation via OpenRouter, 8 in flight: p50 152 ms, p95 263 ms.
Cost: 476k input tokens for 560 requests, $0.02. One request is about 850 tokens;
the criteria are deliberately wordy because Jev reads literally.

The first question design (terse criteria) reached 85.9% raw but produced 27 false
affirms on wrap-ups like "that's it" and "all done" said while a close was pending.
Spelling out "a wrap-up is sleep, not affirm" and "anything starting with no is deny"
removed all of them above 0.8 confidence.

## Decision

- **Confirmation: Jev on, regex fallback.** Default `GHOST_JEV=auto`, threshold 0.9,
  deadline 0.5 s. At that threshold Jev never said yes when the operator didn't, and
  it recovers the natural yeses the regex misses ("yeah that's fine", "sounds good,
  do it", "mm hm, yes"). The negation guard runs first in both paths. The 30% of
  turns below threshold go to the regex, which is exactly today's behaviour.
- **Intent router: not now.** The plan's bar was 15 points of recall on
  sleep/cancel/repeat; the gain at the safe threshold is 4, 3, and 6 points. A regex
  extension for those three intents is nearly as good and has no network on the
  path. Revisit with real-session transcripts.
- **Next data:** a few days of `GHOST_DEBUG_AUDIO=1` sessions, hand-labeled, run through
  the same script. That set decides whether the threshold moves.

## 2026-10-19: looser yes, threshold 0.95

The operator wants Ghost to hear a yes the way a person does (ADR 0017). On a real phone
session "Okay." answering "Okay to close infra 17?" came back as noise at 0.45, and
"Alright." and "Sounds good." as sleep. The affirm criteria and the instructions now say a
short acknowledgement right after the question is affirm; wrap-ups stay sleep, and any "no"
or "wait" stays deny. The rules fallback also takes "okay", "sure", "alright" and
"sounds good" on their own.

Same 560 utterances:

| threshold | jev used | accuracy | false affirm | affirm recall | deny recall |
|---|---|---|---|---|---|
| 0.90 | 65% | 83.2% | 3 | 1.00 | 0.99 |
| **0.95** | 57% | 82.5% | **0** | **1.00** | 0.99 |

All three false affirms at 0.90 are Parakeet's "Market approved." for "mark it approved"
said while another change was pending. The default threshold moves to 0.95: no false
affirms on this set, and every real yes is still heard. Raw Jev accuracy is unchanged at
88.8%; affirm recall went from 0.99 to 1.00 and affirm precision fell from 0.89 to 0.81,
which is what the fallback threshold absorbs.
