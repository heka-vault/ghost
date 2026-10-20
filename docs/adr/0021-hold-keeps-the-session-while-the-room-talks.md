# 0021: "Hang on, Ghost" holds the session while the room talks; "go to sleep" is gone

- **Status:** accepted
- **Date:** 2026-10-20
- **Amends:** ADR 0010 (the wake phrase also resumes a session on hold), ADR 0016 (the phone's mic stays open during a hold)

The operator uses Ghost in rooms with other people: a meeting where Ghost explains an
issue, the room argues the options, and Ghost is asked for the decision afterwards. Until
now the only way to make Ghost stop listening was to end the session, which lost the
conversation; and the phrase for it, "go to sleep", was a proof-of-concept artefact nobody
would say to a colleague.

## Decisions

**Hold is a session state.** "Hang on", "hold on a sec", "give me a minute", "be right
back" and the like, as a whole turn, put the session on hold: Ghost says a few words
("Take your time.") and stops listening. The session, its context, the active thread and
any question Ghost had asked all keep. The model has a `hold` tool for the same thing, and
uses it when two turns in a row are people talking to each other rather than to Ghost.
Hold works for the desk and the phone alike; it is the pipeline's, not a transport's.

**On hold, the rule is the desk's passive rule, inside a session.** Local STT keeps
running so the name can be heard; every transcript that does not address Ghost is dropped
on the spot, not logged, not stored, never shown to the model, and nothing leaves the
machine. The phone keeps sending its audio to the desk (that is the operator's own tunnel
or LAN, ADR 0020); the desk is where it is discarded. Both apps show the state as "Checked
out" (Ghost is checked out of this conversation), and the phone's notification says so.

**Addressed by name, at the start, resumes.** An utterance that begins by addressing
Ghost ("Ghost, ...", "okay Ghost", "alright, so, Ghost") ends the hold, and the rest of it
is the next turn ("Ghost, we have a solution: ..."). The name alone gets "I'm here." The
name in the middle of a sentence ("we should fix that in Ghost") is the room talking about
Ghost and does nothing: in this operator's rooms, Ghost is a project people discuss.

**A hold ends on its own after 30 minutes**, quietly: no goodbye spoken into the room.
The ordinary idle timeout does not run during a hold.

**Goodbye hangs up; "go to sleep" is no longer a phrase.** "Talk to you later", "bye",
"that's all", "thanks, Ghost" end the session with a short, warm goodbye: the operator tried
a silent hang-up and found the goodbye is what makes the end feel human (2026-10-20). The
speed comes from everything around it: an instant phrase match where possible, no model
follow-up after the `hang_up` call, a short end-of-turn fallback, and the phone closing the
call, with a hang-up tone of its own, the moment the goodbye has played. The model's
tool is `hang_up`. Nothing answers to "go to sleep" any more. The control API's `/sleep`
route and the `ghostd sleep` command keep their names for now; they are operator tooling,
not something said aloud.

## Consequences

- `ghost_voice.hold` (the matcher and the gate right after STT), a `hold` quick intent and
  model tool, `Phase.HOLD` in the control API, shown as "Checked out" in both apps.
- `Handled.action` for ability interceptors is `hang_up | hold | cancel`; the reserved tool
  names are `change`, `hang_up`, `hold`.
- With a question pending, "hold on" holds rather than denies; the question is still there
  when Ghost is addressed again.
- The hold timeout is `GHOST_HOLD_TIMEOUT_SECS` (1800).
