# 0072: A desk session tells the person from the room by their level, and the room never keeps it open

- **Status:** accepted
- **Date:** 2026-11-14
- **Measured in:** [2026-11-14-noisy-room.md](../benchmarks/2026-11-14-noisy-room.md), with [the room harness](../benchmarks/room-harness.md)

Tested with other people in the room on a laptop's built-in microphone, Ghost took the room for the person: once it had answered, other people's talk opened turns, was answered, interrupted its replies, and the session never ended. The room harness reproduced it: one talker across the room 12 dB under the person, on headphones, gave 21 to 35 turns nobody said to Ghost in each session, and none of the sessions ended. Across six rooms and four ratios, 2,116 false turns and 54 of 84 sessions that never ended; with this decision, 894 and 40, and 729 → 65 of the false turns from sound well under the person ([the report](../benchmarks/2026-11-14-noisy-room.md)).

Where they came from, in the order they matter:

1. **A turn opens on any voice the VAD hears.** While Ghost is quiet, the VAD's start opens a turn (`turns::BargeInUserTurnStartStrategy`); it interrupts a reply still being made, Parakeet reads the room's words, and the turn closes on them. Silero's volume gate is a fixed −50 LUFS: a talker across the room clears it easily. Over Ghost's voice, two words from the room cut it off.
2. **The room goes on under the person.** The VAD stays on while the room talks, so the person's segment does not end when they stop; the turn is held open, read whole with the room's words in it (ADR 0044), or reopened by the room before its answer is heard.
3. **The idle clock.** A turn's start stops the session's idle clock, and only a reply starts it again. A turn that closed with no words (room sound Parakeet read as nothing) left it stopped: one such turn kept a session open for good. A turn with the room's words in it is answered, and the answer's end starts the clock afresh.
4. **The echo path** adds little once the canceller has converged; the speakers rows of the report and the headphones rows differ by a few turns.

The tested setup is not this one. Ghost is tested with, and works best with, the reSpeaker XVF3800 array with the speakers on its output: it aims at the talker and cancels its own output on its chip. A laptop's built-in microphone is untested hardware. This decision fixes what is small and safe, leaves the array exactly as it was, and measures the rest for a later decision.

## Decision

**The idle clock is not the room's** (`turns::IdleClock`, every session, every microphone). It runs from when Ghost last stopped speaking; a turn stops it while the person talks; a turn that closes with words is answered and the answer's end starts it afresh; a turn that closes with no words resumes it from where it was. Sound that came to nothing never keeps a session open.

**A desk session on a microphone other than the tested array holds the room back by the person's level** (`ghost_voice::room`, in the speech-in stage, `stt::RoomGuard`):

- **What counts as the person:** speech within `GHOST_ROOM_MARGIN_DB` (8) of the person's own level in the 8 s after Ghost stops speaking (`GHOST_ROOM_WINDOW_SECS`), and within `GHOST_ROOM_LATE_MARGIN_DB` (6) after that, as the 90th percentile of the segment's 20 ms levels (its voiced syllables, whatever second of the room the VAD kept before it). Anyone at the desk talking as loud is the person: Ghost is not a speaker-ID product.
- **The person's level** comes from the segment that woke Ghost when the wake heard the session's input (the transcript wake, ADR 0010; a number in memory, never the words or the audio), then from the session's segments with words read while Ghost was quiet and taken on their level. It moves up at once and down a little at a time, never more than the late margin under the loudest the person has been, so a person who lowers their voice is followed and the room, let in on the wide margin, can't drag it to its own level. With no level yet (a manual wake, the trained detectors' wake), a segment in the window is the person's as before, and one later needs the name.
- **What is dropped:** a segment under the line is held where it is heard: no `VadStarted`, so no turn opens, nothing is interrupted and the screen does not show the person talking (ADR 0055); no interim, so the room can't cut Ghost off; no early read. If it grows to the person's level it opens late. If it ends held it is read once, and goes on whole only when it begins with Ghost's name (`hold::addressed`, as a hold resumes, ADR 0021); otherwise it is let go there, its words never leaving the stage, not counted for the microphone's health. A quiet "Ghost, …" from across the room always reaches Ghost.
- **The person stopping under the room:** while the person's segment is open and the room goes on, quiet frames under the line for 0.3 s end it (`room::LevelStop`), so the turn closes when the person stops.
- **When the room is too loud for the microphone** (five segments let go in a session, or the room's steady level within the late margin of the person's), the microphone's health note says so once, in plain words, with what would help: a headset, or the array Ghost is tested with (ADR 0064, `microphone::Problem::Noisy`, half an hour). Shown, never said; no repair is offered for it.

**Where it runs.** `ghostd::microphone::Plan::room_gate`: every desk session except on an on-chip array (the tested reSpeaker XVF3800), whose own gain control and beam sit between the person and Ghost, so a level there means less, and which must not change. Not the phone's call (its own gain control flattens the levels this rests on), not a typed session (no microphone). `GHOST_ROOM_GATE` overrides it. The realtime voice (ADR 0063) hears the session's audio before this stage, so its provider still hears the room; what reaches Parakeet's transcript, the fixed intents, the yes and the agent is gated.

**Cost.** Per 20 ms frame: one level over 320 samples and a push (well under a microsecond). Per turn of the person's: nothing waits; a turn whose onset is soft opens when it grows to their level, at most the segment's length late. Per held segment of half a second or more: one Parakeet read for the name (about 0.1–0.3 s of CPU), which can queue a read of the person's behind it in a loud room.

## Considered options

- **Noise suppression before the VAD and Parakeet** (RNNoise, DeepFilterNet, a spectral gate). Left out: the false turns come from speech, which suppression keeps by design; steady noise alone gave few (the `noise` rows). It would add a model or a dependency to the audio path and a few milliseconds a frame, for what the array already does on its chip.
- **Voice recognition** (who is talking). The "as loud" column of the report is what it would buy: the false turns a level can't separate. A model on every segment and an enrolment step; sized from the report for a later decision, with push-to-talk and recommended hardware beside it.
- **The name required after the window.** Breaks conversation at the person's normal level; kept only for doubtful segments.
- **Fixed thresholds** (raising `GHOST_VAD_MIN_VOLUME`). Fit one microphone at one distance; a laptop's level moves with the person's chair.

## What it does not solve

A second person at the same distance as the person, a loud television or a room within a few dB of the person: those are "as loud" and pass, as before. Ghost's own voice on unprotected speakers is the echo filter's, unchanged. A person who walks away from the laptop and talks from across the room is held unless they say Ghost's name.
