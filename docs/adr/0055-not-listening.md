# 0055: Not listening

- **Status:** accepted (decided 2026-11-12; built on the `feat/stop-listening` branch)
- **Date:** 2026-11-12
- **Builds on:** ADR 0010 (the passive listener), ADR 0013 (the control API), ADR 0016 (the phone), ADR 0031 (do-not-disturb), ADR 0042 (the Hearing test), ADR 0045 and ADR 0052 (Ghost's own app)
- **Amends:** ADR 0016 (one more signed route on the phone listener), ADR 0045 (`ghost_status` says whether Ghost is listening)

The operator asked for "a way to have Ghost stop listening to everything, including wake
words". Until now the only way was Quit, which also stops schedules, watches, the phone and
typing. Smart speakers solve this with a hardware mic-off switch and a red light: the
promise is that the microphone is off, not that the software is ignoring it. OpenClaw's Mac
app has a Voice Wake toggle whose "stopping capture releases the microphone", and push-to-
talk beside it; Hermes Agent has no such control. This takes the smart speaker's promise
and OpenClaw's rule that off means the capture is released.

## Decision

**One state, *not listening*.** While it holds:

- **Not running:** no capture stream on `ghost.mic` (or `GHOST_INPUT_DEVICE`). The passive
  loop does not build the wake listener, so no PipeWire stream is opened and `pw-top` and
  `pactl list source-outputs` show none from Ghost. No frame reaches the VAD, the wake
  detector or Parakeet, and nothing is transcribed. The Hearing test refuses to start
  ("Ghost is not listening. Turn listening on to run the Hearing test.") and one running is
  closed. Ghost does not raise anything at the desk by voice (ADR 0032): `can_raise` is
  false, so the routes reach the operator another way. `ghostd wake`, SIGUSR1 and the
  desktop's Call button do nothing (`/wake` without words answers `ok: false`).
- **Still running:** the daemon, its models (loaded, so turning listening on is instant),
  the speaker, typed turns (the desktop's box and `say` open a session whose input is
  silent: `transport::desk_media` with `mic: false` opens no capture), the phone's own call
  (its audio is the phone's microphone, which the operator opened by tapping; the desk's
  stays closed), scheduled runs and watches, the waiting list and its routes, questions and
  their answers, the control API, the outside tools' listener.

The phase stays `passive` and `Status.not_listening` says it (`{until}` or null): the state
is orthogonal to a session, since a typed turn or a phone call can run meanwhile.

**It is the operator's switch.** It turns off and on from the desktop (the switch by the
brand in the sidebar, Ctrl+Shift+M, Settings › Sound), the tray (Stop listening / Listen
again), the control API (`GET`/`PUT /listening`, so `ghostd listening on|off|toggle
[minutes]` can be bound to a keyboard shortcut), and the paired phone (signed `POST
/listening`). By voice it turns off only: "Ghost, stop listening" (also "turn off your
mic", "mute yourself") is a quick intent with no model; Ghost says "Okay, I've stopped
listening. Turn me back on from the desktop or your phone.", the change it was waiting on
is set aside as with a hang up, and the session ends. There is no voice way back, since
Ghost cannot hear it.

**No tool reaches it, either way.** An agent never turns listening back on: that would
reopen a microphone the operator closed. It does not turn it off either: a model told by an
app's text (or a confused one) to mute Ghost would leave the operator talking to nothing,
and the operator's own words already do it with no model. So there is no tool for it on
Ghost's own app, and `no_tool_turns_listening_on_or_off` holds that line; `ghost_status`
reports `listening: {on, until}` as a read.

**It survives a restart.** It is kept like do-not-disturb, in the store's settings
(`not_listening`: empty, `off`, or the end's time), read before the passive loop starts.
An operator who muted Ghost and rebooted is still muted; an end that passed while Ghost was
down is listening. A value this build cannot read is taken as off, the careful reading.

**For a while, or until turned back on.** Off takes `minutes` (1 to 1440) like
do-not-disturb: the desktop offers 30 minutes, an hour, or until turned back on; the tray,
the phone and the voice intent turn it off until turned back on. At the end the daemon
opens the microphone again by itself and the status says so.

**Turning it off ends a session at the desk** (`Daemon::set_listening`), and closes the
Hearing test, so nothing keeps the microphone. A phone call goes on. A typed turn's session
opened while not listening keeps its silent input until it ends, even if listening is
turned back on meanwhile; the next passive wait opens the microphone.

**Different from do-not-disturb.** Quiet stops Ghost reaching out; not listening stops
Ghost hearing. They are stored and shown separately, and either may hold without the other.

**The status tells the truth everywhere.** The desktop's status card reads "Not listening ·
Mic off" (or "until 3:30 PM") with a struck-through mic in `--danger` on the danger tint, the
switch beside the brand shows the same red glyph, the tray's icon is struck through in red
with the tooltip "Ghost: not listening (mic off)", the phone's Home says "Your desk isn't
listening. Calls from here still work." with a button to turn it on, and `ghost_status`
says it to agents.

## Consequences

- The cost on a turn is nothing: no new tool schema, and the passive loop's check is one
  read of a watch channel per wait.
- The switch has no undo in the gate's sense and is not audited as a change: it is not a
  change to an app or to Ghost's folder, it is the operator's device control, like the
  volume. Every turn of it is logged (`listening: on`, `listening: off …`).
- `Phase::Passive` now means "not in a session"; a client that wants to say "waiting for
  Hey Ghost" checks `not_listening` too (the desktop's sidebar and tray do).
- The session screen's own start state (owned elsewhere) still says to say the wake phrase
  while not listening; the sidebar's card is the truth until it is updated.
