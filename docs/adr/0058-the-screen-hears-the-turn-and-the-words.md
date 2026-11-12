# 0058: The screen gets the turn's final words, the words being heard, what a call returned, and what a session did

- **Status:** accepted
- **Date:** 2026-11-12
- **Amends:** ADR 0055 (the said-so-far estimate, activity live only, a call's result not shown), ADR 0013 (two more events, two more reads)

ADR 0055 gave the conversation screen Ghost's voice level and the session's activity. Four
things it left out showed in use: the operator's turn was shown as its pieces joined, though
the daemon reads a turn of several pieces again whole (ADR 0044) and the model and the store
get that text; "said so far" was an estimate from voiced time; an opened call said that what
came back was not shown; and a reload lost the calls, handovers and cuts of the session.

## Decision

**`Event::Heard { heard: { session_id, at, text, reopened } }`**: the operator's spoken turn
closed with the words the model was given, from the same `TurnRecord` the store and the gate's
request keep (`SessionHooks::on_heard`, from the user aggregator or an interceptor that took
the turn). `reopened` (with an empty `text`): the last turn closed was taken back before its
answer was heard; its words come back in the turn that goes on. The screen holds the pieces
(`turn` events, still sent as they are read) and replaces them with this text; nothing else
closes a turn any more (the status leaving `listening` was a stand-in and is gone). It is a
live session's own turn, the words it already stores, published only while a session is open
(`DaemonState::publish_heard` drops it otherwise): never a passive transcript (ADR 0013).

**The phone gets `heard`.** Its captions show the same pieces and need the same close; the
data channel already carries the session's `turn` events, and `heard` is that turn's final
form, not new information. It does not get `saying` (below), as it does not get `voice` or
`activity`: nothing on the phone draws them and the channel shares the call.

**`Event::Saying { saying: { session_id, reply, from, to } }`**: which words of Ghost's reply
are being heard. The TTS stage (`KokoroTts::stamp`) numbers each reply (one more at each model
reply's `LlmStart` and each line spoken directly) and counts its words as it renders each
clause (`ghost_voice::spoken::word_count`: runs between spaces with a letter or a digit, after
`spoken` has dropped Markdown, so a list marker is not one); each audio chunk carries its
clause's span (`Audio::spoken`); the output stage hands it to the voice meter, which already
paces windows to when they are heard, and the meter reports the span when its first window
plays and it differs from the last (`level::OnSaid`). The screen counts the reply's words the
same way (`@ghost/ui` `wordEnds`) and marks the clause being heard; within a clause it has
nothing finer, so a cut is placed inside the clause by its voiced time. A reply number is tied
to the newest reply on screen when first heard; a reply with nothing to say aloud gets no
stamp. The cloud voice (`elevenlabs.rs`) does not stamp: the screen falls back to ADR 0055's
estimate.

**`GET /calls/{id}`**: what one call of the live session returned, as the model was given it.
The dispatcher (`ToolDispatcher::set_on_result`) hands each call's result to the daemon after
it came back; for an app's tool that is the result already redacted and cut to the app's budget
by `ghost_core::apps` (ADR 0046), and for Ghost's own tools Ghost's own text. The daemon redacts
it again, pretty-prints it and keeps the first 2,000 characters (`CALL_PREVIEW_CHARS`) and how
many were cut, for the last 64 calls (`CALLS_KEPT`), in memory only: never written to the store,
never pushed on the event stream, gone at a restart. The screen fetches it when the operator
opens a call. Session tools (hang up, hold, a transfer) keep nothing. For a change the screen
also shows the gate's audit row that finished while the call ran.

**`GET /activity?sessions=a,b`**: what sessions did, kept beside the transcript in SQLite
(`session_activity`: the session, the kind, the time, a tool's name and call id, a handover's
names and seconds): `call`, `result`, `handover`, `cut`, `error`. The operator's turn opening
and closing stays live only. Written off the pipeline's task (`spawn_blocking`) as the
activity is published; at most 400 a session are read back; a session's rows are pruned with
the last of its turns (`Store::prune_turns`), so retention is the transcript's. The screen asks
for the sessions it shows and merges them with what arrives live.

## What it costs (ADR 0018)

Measured on the workstation in release (a throwaway timing test, 2026-11-12):

| What | When | Cost |
|---|---|---|
| `word_count` of a clause, in the TTS stage | per clause rendered | 85 ns |
| the span copied into each audio chunk, compared per 50 ms window in the meter's task | per chunk / window | a 12-byte copy, one comparison |
| `saying` event, encode and broadcast | per clause heard | 0.12 µs, 104 bytes |
| `heard` event | per closed turn | 0.39 µs |
| `keep_call`: clone, pretty-print, redact, cut | per call, after its result is in the model's context | 0.5 µs (a small result) to 13.3 µs (24,000 characters) |
| `session_activity` insert, on the blocking pool | per call, result, handover, cut, error | 0.63 ms, off every pipeline task |

No step is added between the end of the operator's speech and Ghost's first sound: the stamps
ride frames that were already there, and everything else happens after a result or beside the
speaker. `make perf` was not rerun for that reason.

## Consequences

- The screen shows the turn as the model heard it, the clause being said exactly, what a read
  returned, and a session's work after a reload or in history.
- A new kind of stored activity is a new value of `session_activity.kind` and of
  `ghost_api::calls::kept`; it must carry no words.
- What a call returned is untrusted app text on screen: shown as text, never markup, inside the
  same bounds the model had. A password that reached a result is redacted twice.
