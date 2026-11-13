# 0063: A realtime voice the operator may choose

- **Status:** accepted (decided by the owner 2026-11-12; built on the `feat/realtime-voice` branch)
- **Date:** 2026-11-12
- **Builds on:** ADR 0010 (idle audio stays local), ADR 0017 (the request is the yes), ADR 0018 (performance), ADR 0026 (a cloud voice), ADR 0036 (passwords), ADR 0044 (the turn as recorded), ADR 0047 (the run with nobody there), ADR 0050 (the fast model), ADR 0055 and ADR 0058 (what the screen sees), ADR 0056 (the provider check), ADR 0057 (not listening)
- **Amends:** ADR 0010 (where the operator's voice may go after the wake), CLAUDE.md's rule on idle audio

## Context

With the local voice (Parakeet, the model connection, Kokoro) Ghost's first sound is about
1.3 to 2.3 s after the operator stops and a lookup is answered in 5 to 10 s
([docs/benchmarks/perf/2026-11-12-reply-time.md](../benchmarks/perf/2026-11-12-reply-time.md)).
The floor of that design is the text model's own first token and its rounds. The leaders'
voices, and OpenClaw's (its Discord voice runs a realtime model with an "agent consult"
brain and a "speak exactly" message for the agent's answer), put a realtime speech-to-speech
model in front: it hears the audio as it comes, speaks within a few hundred milliseconds,
handles small talk and hands real work to the agent. The owner: "Make it something that users
can opt into, but by default we should use the local privacy first model."

The providers as their own documentation had them on 2026-11-12:

- **OpenAI Realtime** (`gpt-realtime-2.1`, `gpt-realtime-2.1-mini`): one WebSocket with the
  key as a bearer header, PCM16 at 24 kHz both ways, server VAD whose automatic reply and
  interruption can each be turned off mid-session (`session.update`), `response.cancel`,
  `conversation.item.truncate` at the milliseconds heard, a per-response `instructions`,
  `input` and `tool_choice`, a metadata tag echoed on `response.created`, function calls with
  ids, a transcript of its own speech as it speaks, usage per response with audio, text and
  cached tokens apart, sessions up to 60 minutes. Audio is about 600 tokens a minute heard
  and 1,200 spoken; the mini model is $10 and $20 a million (about 3 cents a minute each way
  of plain audio), the full one $32 and $64, and every response re-reads the conversation
  (mostly from the cache).
- **Google Gemini Live** (`gemini-3.8-live`): a WebSocket with the key in the URL's query,
  16 kHz in and 24 kHz out (Ghost's own rates), function calls, an `interrupted` flag, input
  and output transcription, the cheapest audio ($3 and $12 a million, about half a cent and
  2 cents a minute), but 15-minute audio sessions without its compression, and no documented
  way to stop its automatic reply mid-session.
- **OpenAI GPT-Live-1** (in the API since 2026-09-10): built for exactly this split (a voice
  that delegates to the developer's own agent, `$0.05` a minute), but its delegation event
  carries no words, it paraphrases what the agent hands back by design, and it has no
  cancel or session update. Ghost needs the answer said exactly.

## Decision

**Two voice modes, the local one the default.** `GHOST_VOICE_MODE` is `local` unless the
operator sets `realtime` in Settings › Sound. Turning it on shows, before it is on, what
changes (`ghost_core::realtime::settings::disclosure`, the same lines as
[docs/realtime-voice.md](../realtime-voice.md)): after the wake, for the session, the
operator's voice itself goes to the provider; idle audio and the wake stay on the machine;
the agent still does every lookup and change; what it roughly costs; that it can be turned
off at any time. Going back never asks. Only the operator sets it: no tool, app, model, run
or outside tool has a route to `GHOST_VOICE_MODE` or `GHOST_REALTIME_*`, and Ghost never
changes the mode itself; a failure falls back for one session only.

**Where ADR 0010's line moves.** Before the wake nothing changes: passive frames go only to
local models and are discarded. After the wake, in a desk session with the realtime voice on,
the session's microphone audio (through Ghost's echo setup) also goes to the provider, for
the length of the session: never on hold, never while Ghost speaks unless Ghost's own
barge-in rule says the operator is talking over it (then with the 1.5 s before), never in a
session opened by a typed turn while not listening (ADR 0057: no microphone, no realtime
session), never on the phone's call (the local voice there, for now), and never in a
scheduled run. Ghost writes none of it to disk.

**The seam.** `ghost_voice::realtime::RealtimeProvider` connects with fixed instructions and
tools and returns a `RealtimeLink` (audio in at 16 kHz, cancel, truncate, a call's answer,
say exactly, automatic replies on or off) and a stream of `Event`s (speech started and
stopped, a response started with its tag, audio at 24 kHz, its words, a tool call, done with
usage, a declined request, a failure). A second provider is an implementation of these two
traits. **OpenAI Realtime ships first**, `gpt-realtime-2.1-mini` by default: it is the one
whose session Ghost can steer turn by turn (the automatic reply off while a change waits for
its yes, a relay with no tool allowed, a cut at the milliseconds heard), the key travels in a
header and not a URL, and an hour is a session. Gemini Live is the second (cheapest, and
16 kHz in as Ghost already has it); GPT-Live-1 is not a fit until it says the agent's words
as given.

**A mouth and ears, not the agent.** The realtime model gets three tools and no app's:
`consult` (hand the request to Ghost's agent), `hang_up` and `hold`. "Stop listening", "open
the app", "never mind" and the rest of Ghost's fixed intents are heard by Ghost's own
recogniser and taken by the interceptors as today; listening is still never a tool's. A
consult is a user turn into the session's own model stage (`Frame::user_text` at the
session's source, let through the realtime gate): the same agent, context, dispatcher,
gate, fast model and audit as a spoken turn on this machine, no second agent loop. The
agent is given the operator's words since the last consult as Ghost heard them, and the
realtime model's reading of them marked as that. Its final reply is said by the realtime
voice with `response.create`: the exact text as the instructions, an empty `input`,
`tool_choice: none`.

**What is said is checked.** The provider transcribes its own speech as it speaks
(`response.output_audio_transcript.delta`). `ghost_voice::realtime::relay::RelayCheck` reduces
both texts to plain words (numbers as digits, written or spelled) and requires the spoken
ones to follow the answer in order: up to three words of filler and two small words left
out, nothing else. The relay's audio is held and released only two words ahead of the words
verified. A reading that strays (an inserted "not", another number, a word of the answer
missing, "allow" for "block") is cut at the provider and the speaker, and the answer from the
word that was not said is said in the local voice; one that never starts within 4 s is said
locally whole. A change's outcome reaches the operator the way it does today: the gate
executes, the confirmation processor puts the verified result in the agent's context, and
the agent's answer is relayed and checked. The realtime model is told it knows nothing and
must never say a thing was done; nothing it says on its own is Ghost's record of a change.

**The words of record stay Ghost's own.** Parakeet keeps reading every segment beside the
provider: the stored transcript (ADR 0044), `heard` (ADR 0058), the fixed intents, the
gate's "plainly asks" (ADR 0017) and the yes. While a change waits for its yes the provider's
automatic reply is off, so it cannot answer "yes" itself; the operator's turn goes to the
confirmation processor on Ghost's transcript and to the agent, whose answer is relayed. The
provider's input transcription is not requested: the evidence never depends on a model's
reading. `ghost_answer`'s rule is unchanged for the same reason. The cost is Parakeet on
every segment as now.

**Everything else.** The wake and its detectors are unchanged. Barge-in is Ghost's rule
(`interrupt_response` off; a leak of Ghost's own voice past the canceller cannot cut it off),
and an interruption cancels the response and truncates it at what was heard. Hold stops the
audio to the provider. Activity, voice, heard and saying come from the same stages (the
provider's audio goes through Ghost's output stage). Typed turns go to the agent and are
said by the realtime voice. Schedules and runs never touch it. Latency records of a realtime
session carry `"voice": "realtime"`, and each exchange is a record of its own
(`kind: realtime`: `talk` with its first sound, `answer` with the consult, the relay's start
and its outcome, `connect`, `fallback`).

**Failure is said and shown.** Unreachable, refused (401, 403, quota), rate-limited, a drop
mid-session or a server error: the session goes on locally with one sentence ("I lost the
realtime voice, so I'm answering on this machine for now."; "The realtime voice isn't
answering…" at the start), what was not yet said of an answer is said locally, the
operator's last unanswered words go to the agent, and a note goes on the waiting list
(shown, not said). A missing password or today's cap reached opens the session locally with
the note and no sentence.

**Cost.** Each response's usage is priced from the provider's list as built
(`PRICES_AS_OF`), added to a session total and to the day's in `realtime-spend.json` beside
Ghost's data (numbers only), shown in Settings with the last session's. An optional daily cap
(`GHOST_REALTIME_DAILY_CAP_USD`) ends realtime for the day, mid-session with a sentence.

**The check.** `POST /voice/realtime/check`, Settings › Sound's Test and `ghostd llm test`
(when the mode is on, or with `--realtime`) connect, ask for a fixed line said exactly (its
first audio) and a fixed written request (handed on through `consult` or not). The harness:
`make perf-turns REALTIME_KEY_ENV=VAR` (`ghostd perf realtime`) streams Kokoro-rendered lines
at the pace of speech and times first sound, the consult and the relay's start, with the
key from the named variable only.

## Consequences

- The default build and the default session are unchanged: with the mode off no realtime
  stage is built and nothing of this runs on a turn.
- A second provider, a second model voice per agent and the phone's call are later work.
  An agent's own Kokoro voice does not carry into the realtime voice (one voice a session).
- What is said is the agent's answer, but only up to the two words the audio runs ahead of
  the check: a reading that strays may be heard for a word or two before it is cut.
- The realtime model's small talk is its own words, spoken as Ghost's. It sees no app's text;
  its instructions are fixed in code.
- A relay costs provider output audio for every answer the agent gives, and the agent's own
  model calls are unchanged.
