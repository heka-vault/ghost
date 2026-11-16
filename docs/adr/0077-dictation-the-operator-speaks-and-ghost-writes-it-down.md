# 0077: Dictation: the operator speaks, Ghost writes it down

- **Status:** proposed
- **Date:** 2026-11-15
- **Builds on:** ADR 0010 (the passive listener), ADR 0013 (the control API), ADR 0018 (performance), ADR 0057 (not listening), ADR 0064 (the microphone plan), ADR 0065 (kept wake takes), ADR 0074 (debug mode)
- **Amends:** the idle-audio rule in CLAUDE.md (a third state beside passive and the realtime voice: the operator's own dictation)

The operator dictates every day with nerd-dictation on F9: a 2021 Vosk model, typing through
`ydotool`. Ghost already has a better reader on the same machine (Parakeet, ADR 0043), the
microphone path tuned for the operator's voice (ADR 0064), and a passive loop that hands the
microphone over cleanly (the Hearing test, ADR 0042). The design plan (2026-11-15, with a
second review's amendments) puts the whole of dictation in seven slices: the key held down,
the paste into the window with the keyboard, the indicator, long dictations, other desktops.
This ADR records what dictation is and the lines it does not cross, and the first slice,
which ends at the clipboard.

## Decision

**Dictation is the operator's own act: start, speak, stop.** Ghost opens a capture on the
microphone a session hears (`microphone::Plan::session`), records until stopped, reads the
whole of it once with the Parakeet it already has (the trailing silence and the dither a
segment gets, waiting its turn behind a read in flight), cleans the text up, and puts it on
the clipboard. `ghostd::dictation` is the one coordinator: each change of stage goes one at a
time, each dictation has a generation, a second start while one starts, records or is read is
refused, stop and cancel with nothing going on say so and change nothing, and a read that
finishes after its dictation was cancelled does nothing. A start is a stage of its own
(*starting*) from its first step, taking the microphone from the passive loop, opening the
capture and waiting for its first frame: listening off, a session opening and Ghost stopping
cancel it as they cancel a recording, and the cancel returns only once whatever it was opening
is closed and the microphone given back; an open already under way is let finish and then
closed, never left to finish after. Just before it records, a start asks again whether
listening is on. Every mode has a hard limit on the recording, counted in samples and in time
(a capture that stalls still ends): 30 seconds in this slice (one whole read, no pieces); at
the limit it stops by itself and what was said is read.

**It starts only from the operator's routes.** `GET /dictation` and `POST
/dictation/start|stop|toggle|cancel` on the control API, and `ghostd dictate
start|stop|toggle|cancel|status`, a client of them, which a desktop's own keyboard shortcut
runs ([dictation.md](../dictation.md)). Said honestly: the control socket is a boundary of
the operator's user account, not proof of a key press, so a program running as the operator
can start dictation, as it can run any `ghostd` command or type with `ydotool`. What holds is
that no tool of Ghost's own app, no outside client, no phone route, no model, no run, no app
and no webhook starts it, stops it, or reads what it wrote: there is no tool for it, the
phone and outside listeners have no route to it, and `no_tool_starts_dictation_or_reaches_the_clipboard`,
`nothing_an_outside_client_must_not_reach_is_offered` and the phone's route test hold that.
When a later slice pastes into the focused window, the paste comes only from a real key seen
through the desktop's shortcut portal, never from these routes or the CLI: those end at the
clipboard.

**It refuses, with one plain sentence each:** while not listening (ADR 0057 stands
unchanged: no desk capture while it is off); while a session, a phone call or the Hearing
test has the microphone, or the passive loop is not waiting (dictation during a conversation
is a later slice, refused until then); while the speech model is not loaded; while PipeWire
shows no microphone; when the microphone will not open or gives no sound within three
seconds. **It is cancelled**, closing the capture and dropping what was recorded or is being
read, when listening is turned off, when a session opens (a wake, a typed turn, Call, the
phone), and when Ghost stops.

**The audio barrier.** Dictation takes the microphone from the passive loop with a lease, as
the Hearing test does. Before the loop grants it, it stops the wake listener and waits until
its captures are closed: the listener's pipeline (the VAD, the detector's window, Parakeet's
wake check, any pre-roll) has ended, which is when its capture is dropped, and the kept takes'
array tape (ADR 0065), which runs its own raw capture, has closed it, an open still in flight
included, and cleared its 30 s ring. Each start and stop of the tape moves its ring to a new
generation, so a frame a stopping capture had in hand is refused. A listener that did not stop
in time keeps the lease from being granted, and dictation is told the desk is busy. While
dictation records nothing of the passive listener runs, and no session runs, so no dictated
frame reaches the wake detector, the passive VAD, Parakeet's wake check, a session's
speech-to-text or the realtime uplink (ADR 0063). The loop listens again only once dictation's
own capture has closed; it waits for that with no time limit (a session or a phone call that
cancelled the dictation waits too), with a fresh pipeline and a tape that starts empty, so a
take kept right after a dictation holds none of it. Held by
`dictation_never_reaches_the_wake_or_a_kept_take` (a room whose every sample says when it was
heard, the tape's own task over a capture slow to open and to close, and any overlap of the
tape's capture with dictation's counted as a failure),
`stopping_the_tape_waits_for_an_open_in_flight_and_its_close` and
`the_listener_is_quiet_only_once_its_capture_is_closed`.

**Cleanup is deterministic, no model, dictated text only.** Fillers out (um, uh, er, hmm and
their spellings; all-capital words are kept, since "ER" is a word), a word said three or more
times in a row kept once ("I I I"; a doubled word stays, since English says "that that"), the
capital and the sentence's end handed on after a removal, and a final sanitizer, the last
step before the clipboard: CR, LF, tab and the Unicode line and paragraph separators become a
space; NUL, ESC, DEL, every other control character and the bidirectional controls go. No list
of the operator's words in this slice. A session's turn keeps its fillers as before.

**The clipboard, through X11.** GNOME's mutter gives an unfocused Wayland client no
clipboard (no data-control protocol), and `wl-copy` there needs a surface that takes focus.
An X11 client owns a selection without focus, and mutter bridges XWayland's CLIPBOARD to
Wayland clients (checked here on GNOME 50.1 on 2026-11-15: an unfocused X owner's text was
read by a Wayland client, mutter's clipboard manager read it at once, and the text was still
there after the owner went away). On X11 it is how every program copies. `ghostd` owns
CLIPBOARD from an input-only window, never shown or focused, through x11rb's pure-Rust
connection (no Xlib, which would end the whole daemon on an X I/O error, and no libxcb);
x11rb finds the display and reads the Xauthority file, Ghost never does. Every wait on the
display's socket is bounded (the setup and the selection within 3 s, each reply while
answering a paste within 2 s, each write within 2 s), and a cancel interrupts it at once by
shutting the socket down. One owner thread at a time: it answers pastes until something else
is copied, the next dictation replaces it, or Ghost stops (the daemon closes it as it winds
down); a replaced, failed or closed owner is joined, never left holding the text. A property
the display refused is answered as a refusal, never as success. A text over 16 KiB (thirty
seconds of speech is a few hundred characters) is refused with its own sentence, so one
request always carries it and the incremental transfer is never needed. Only CLIPBOARD, never
PRIMARY; nothing restores what was on the clipboard before in this slice: dictating replaces
it. Where it cannot be set (no `DISPLAY`, no XWayland) the outcome says so and the words are
gone.

**Ghost keeps nothing, and the words go nowhere but the clipboard.** Not the transcript
store, the event bus, the audit, `latency.jsonl`, a log line at any level, `debug::audio()`
(debug mode does not log dictated text either: ADR 0074 does not widen), an error or a
panic report, a diagnostic report, the phone, an outside client, a tool or a model.

**What is wiped from memory, and what is not.** Ghost's own buffers are zeroed when dropped:
the recording (sized once for the whole limit, so it never grows and leaves a copy behind),
each captured frame once it is copied in, the float and the dithered copies given to Parakeet,
the string Parakeet returned and its trimmed copy, every string and token the cleanup makes
(each sized once with room to spare, compared without lowercase copies), and the clipboard
owner's copy of the text and its Latin-1 form. Not wiped, because they are not Ghost's to
wipe: Parakeet's and ONNX Runtime's own buffers inside the model, the capture's ring inside
`ghost_audio` and PipeWire's buffers, x11rb's request buffers that carry the text to the
display, the display's own copies (XWayland, mutter's clipboard manager), and the X cookie,
which x11rb reads and holds in its setup request.

The API answers with a state, an outcome and
its reason from closed lists, and numbers (seconds recorded, characters, the delay to the
first frame, the queue wait, the read): its types have no string field
(`dictation_carries_no_words`), and `dictated_words_reach_only_the_clipboard` dictates a
marked sentence over a marked sound through a running daemon, with the real clipboard on a
display of the test's own where Xvfb is installed, through a read cancelled while it runs,
listening turned off while recording, a text too long for the clipboard and one that lands,
and searches every file it wrote (for the words and the sound's bytes), every line it logged
with debug mode on, every event on its bus, every answer it gave and every notification. Its
Parakeet is a fake; the live trials read with the real one.
What the promise is not: once on the clipboard the text is the desktop's. A clipboard
manager (mutter's own, or one the operator installed) may keep it, and the application it is
pasted into may store it or send it anywhere.

**What the operator sees in this slice.** The CLI's own line, and a desktop notification
through `notify-send`, as the desktop route already sends them: "Dictating" while it records,
then one sentence with the character count ("On your clipboard: 42 characters. Press
Ctrl+V."), replaced in place and transient, never the words. No event is published for the
desktop yet: before one is, the phone's forwarding of the bus (`phone::routes`,
`forward_events`, a deny list) becomes an allow list, held on the backlog and the live path.

**Coexisting with nerd-dictation.** The check that pauses the wake while the operator's
outside tool runs stays, renamed (`nerd_dictation::running`) so it is not taken for Ghost's
own. It goes once Ghost's dictation has the key.

## What it costs

- At idle: nothing running. The passive loop waits on one more lease beside the Hearing
  test's; no thread, no capture, no timer until a dictation starts. A turn: nothing; no tool
  schema, no stage.
- While dictating: one capture stream into memory (about 1 MB for 30 s), and the wake paused.
  After: one whole Parakeet read (about 30 to 50 ms per second of audio, ADR 0044), and one
  thread owning the clipboard, asleep on its socket with no timer, until something else is
  copied.
- Three crates (x11rb, x11rb-protocol, gethostname; MIT or Apache-2.0; cargo-deny passes;
  rustix was already there): about 6 s of a clean release build of a minimal program on a
  12-core machine, built beside the rest of `ghostd`'s, and about 146 KB of a stripped
  binary.
- The delay from the start request to the first captured frame is measured on every
  dictation and logged as numbers (the hand-over, the capture's open, the first frame).
  Measured on 2026-11-15 on this workstation (a debug build, 13 starts, the reSpeaker and a
  webcam microphone): 27 to 44 ms in 11, 130 and 134 ms in two, where PipeWire took about
  100 ms to open the stream; the passive loop let go of the microphone in 2 to 5 ms. A word
  begun within that much of the request is clipped; nothing before the request is heard, by
  design. That is under the plan's 100 ms line most of the time, so no ring of passive audio
  is added. Again after the second review's fixes (the hand-over now waits for the wake's
  captures to close), 13 starts: 26 to 50 ms in 12, 125 ms once; the hand-over 2 to 9 ms.
- One read waited 0 ms for the recognizer in every live run (nothing else reads while
  dictation holds the microphone); the read took 187 to 249 ms for 3.4 s of audio and
  1.57 s for 30 s, and 191 to 312 ms for 3.4 s in the second trial (483 ms for its first
  read).

## Considered options

- **The passive listener's stream, with a ring in front of it for the first word.** Would cost
  nothing to open, but keeps passive audio that a dictation might later read: a change to the
  idle-audio rule not made unless the measured open delay demands it.
- **`wl-copy`.** On GNOME it takes focus to set the clipboard; not shipped with Ghost.
- **The Clipboard portal.** It rides on a RemoteDesktop session, which asks in a GNOME dialog
  and hides the floating view while it lasts (ADR 0059); kept for the paste slice.
- **Dictation while not listening (the held key opens the microphone).** The operator's later
  decision; ADR 0057's promise stands until then.
- **Dictation beside a session.** Needs the session's own speech-in to drop frames with no
  turn, interim or yes made of them; refused until that slice.

## Consequences

- The idle-audio rule gains a third state, the operator's dictation: while it records, the
  session's microphone is open to Ghost's local models only, nothing passive runs, and the
  text goes only to the clipboard.
- A dictation never answers a waiting change: it cannot run in a session, and its words never
  reach the confirmation path.
- Later slices: paste into the focused window (the RemoteDesktop portal, Shift+Insert,
  restoring the clipboard, the destination checked), hold to talk (the GlobalShortcuts portal),
  the indicator and Settings › Sound › Dictation, dictation beside a session, past 30 s in
  pieces, X11 keys, KDE and Hyprland.
