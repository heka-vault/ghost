# 0055: The desktop sees Ghost's voice and the session's work, never its words early

- **Status:** accepted
- **Date:** 2026-11-12
- **Amends:** ADR 0013 (the event stream carries two more kinds, both live-session only)
- **Amended by:** ADR 0058 (the turn's final words, the words being heard exactly, a call's result on request, activity kept beside the transcript)

The conversation screen was a chat transcript with a microphone beside it: the operator could
not see that Ghost was heard, what it was doing between two turns, how far into a reply it was,
or where they cut it off. A voice-first product needs the screen to show the conversation as it
happens, without ever showing words as they are spoken (#73) and without the microphone's
signal reaching the apps.

## Decision

**Ghost's own voice level stays as it is.** `Event::Voice { level }` (shipped with the
desktop's waveform on 2026-11-05, recorded here): the RMS of each 50 ms of what is playing,
-50 to 0 dBFS mapped to 0 to 1, about twenty a second while Ghost speaks, timed to when it is
heard, then one `0.0`. Ghost's output only, never the microphone, never audio samples. That is
enough for a waveform that follows the voice and for counting how long the voice has sounded;
a finer envelope (several values per window) was considered and not added, since syllables
are 4 to 8 a second and 20 Hz already draws them.

**A new `Event::Activity`** reports what the live session does, by name and never in words. A
pipeline observer (`ghost_voice::activity`, beside the latency timeline on the same
`Observer` seam) maps a handful of frames:

| `kind` | from | carries |
|---|---|---|
| `you_started`, `you_stopped` | `UserStarted`, `UserStopped` | nothing: someone is speaking, the turn closed |
| `call`, `result` | `FunctionCallInProgress`, `FunctionCallResult` | the tool's name as offered to the model, the call id |
| `handover` | `HandOver` (ADR 0050) | the calls' names, seconds to decide |
| `cut` | `Interruption` while Ghost is heard | nothing |
| `error` | `Error` | nothing (the message stays in the log) |

with the session id and the time. It never carries a transcript, a reply, a tool's arguments
or result, or audio, and nothing outside a session exists to observe. `GhostState.activity`
holds the session's list in the apps.

**The screen estimates which words have been said** (`@ghost/ui` `said.ts`): the reply is
recorded whole when Ghost's voice starts on it (`ghost_voice::tap`), and each voice window with
a level is 50 ms of voice, so voiced time × 16 characters a second (Kokoro's voiced pace at
speed 1.0) × the agent's speed is about where the voice is. It moves by whole words, never
claims the last word while the voice is going, is exact only at the ends, and a `cut` freezes
it. An exact word clock would need the TTS stage to stamp audio with its text; not worth it
until the estimate is seen to be wrong. (ADR 0058 added that stamp; the estimate stays for a
voice that does not stamp.)

**The operator's own speaking is a sign, not a level**: the stage shows "Listening to you" and
a slow breathing ring from `you_started` (or pieces #73 holds), the same whatever the volume.
The microphone's level stays off the event stream outside the Hearing test (ADR 0042).

**Not on the phone.** The phone's data channel drops `activity` as it drops `voice`
(`ghostd::phone::routes::forward_events`); the phone's live stream (ADR 0051) never had either.

## What it costs (ADR 0018)

Measured in release on the workstation (`ghost_voice::activity`, a throwaway timing test,
2026-11-12): a frame the observer passes over costs **2 ns** (one `match` on its kind, in the
pushing stage's task, on every frame of a session including the 50 a second of audio in); a
frame it reports costs **37 ns** to the callback, then the daemon's JSON encode of the event,
**0.8 µs** for its 198 bytes, and a broadcast send. That happens about four times a spoken turn
and twice per tool call: under 10 µs a turn. The voice level remains about 20 × 30 bytes a
second while Ghost speaks. Nothing waits on either: the broadcast drops for a slow client. No step is added to a turn's path to
the speaker; `make perf` is not affected.

## Consequences

- The record shows the work between turns (one row per run of calls, opened to each), a cut
  where it came, and a failed turn, live. (ADR 0058 keeps them beside the transcript, so a
  reload and history show them too.)
- What came back from a read is not shown: the result is untrusted app text that reaches the
  model within ADR 0046's bounds, and putting it on screen needs its own bounded, redacted
  path. (ADR 0058 is that path: `GET /calls/{id}`, on request.)
- A new kind of activity is a new variant in `ghost_voice::activity::Activity`, `ActivityKind`
  in `ghost_api::models` and the TS client; it must carry no words.
