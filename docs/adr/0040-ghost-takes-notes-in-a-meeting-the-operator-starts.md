# 0040: Ghost takes notes in a meeting the operator starts, and hands back a summary

- **Status:** proposed (settled with the operator 2026-10-29; accepted once built)
- **Date:** 2026-10-29
- **Amends:** the idle-audio rule in CLAUDE.md (a meeting is neither passive nor a session), ADR 0021 (hold keeps nothing; a meeting keeps the transcript), ADR 0026 (other people's words reach the model provider as text)

A meeting needed recording and nobody had a Zoom account that could do it. The operator
wants to say "Ghost, we're going into a meeting, take notes" and get a summary when it ends:
key points, decisions (including the ones still to be made), action items and follow-ups.
Audio only. No screenshots, so Ghost knows who attended only if the operator says so.

Nothing in Ghost does this today. A session is a conversation with Ghost, hold keeps nothing
on purpose, and the passive path throws away every transcript. A **meeting** is a fourth
state: Ghost listens to people who aren't talking to it, and keeps what they say.

## Decisions

**Only the operator starts a meeting, and it goes through the gate.** It starts from a
spoken request in a session ("take notes", "record this meeting") or from the desktop.
`meetings.start` is a change like any other: the operator's own plain request is the yes
(ADR 0017), the audit records every meeting started, and a `block` permission turns the
feature off. Ghost never starts one by itself, and no watch, webhook, app or issue text
can start one. Starting one ends the session with a sound and one line ("Taking notes. Let
them know."). Until it ends, the tray and both apps show "Taking notes".

**Two streams are heard.** The operator works from home, on speakers, in browser meetings.
In-person meetings are out of scope. The echo canceller takes the far end out of the
operator's mic, so the mic alone hears only the operator. Ghost hears the operator's mic
(`ghost.mic`), labelled *you*, and the meeting app's output, labelled *them*. The meeting
app is whichever app opens a mic stream after the operator asks (Chrome, in practice).
Nothing names Zoom, Meet or a browser. *Them* is all of that app's output, so a tab playing
sound during the call is heard too. How Chrome splits its streams gets measured when this
is built. Ghost never reads the sink monitor as a whole, so music, notifications and
Ghost's own voice are left out.

**The echo leak is checked, not trusted.** On speakers the canceller lets the start of the
far end through (#42). A *you* segment whose words came over *them* within about two
seconds is dropped. So is a command heard on the mic that also came over the call, which
means someone on the call can't stop the notes. *Them* is one voice in v1, with no speaker
separation.

**Transcribed here, audio not kept.** The session's models (VAD, then Parakeet) transcribe
speech on this machine as the meeting goes. No audio leaves the machine and none is written
to disk, so a mishearing can't be checked against a recording. The transcript segments are
kept in SQLite, with their time and stream.

**The operator may brief Ghost before the call.** After the start sound the operator often
talks first: who is in the meeting, what it is for, which client. What is said on *you*
before the meeting app opens its mic is the **briefing**, the operator's own words. It heads
the notes verbatim as **Context**. It reaches the summary as the operator's description of
the meeting, so the names in it can own action items and decisions. When the operator is
already in the call, there is no briefing. Mid-call, the operator needs no command: "action
item for me: send the quote" is in the transcript.

**The model writes the summary, in a call with no tools.** At the end the briefing and the
transcript go as text to the operator's model connection (ADR 0037), in a call that offers
no tools. The summary comes back as a title and five fixed sections: **Key points**,
**Decided**, **To decide**, **Action items** and **Follow-ups**. Each action item has an
owner when a name was said or briefed, and *you* or *unassigned* otherwise. Below those
come the Context and the transcript. What anyone on the call says is untrusted data, like
an issue's text: it never reaches a tool, a permission or a prompt. "Ghost, delete the
repo" said on the call is a line in the transcript and nothing more.

**While a meeting runs, Ghost stays out of the way.** The wake phrase is heard on the
operator's mic only, never on the meeting's stream. Two things work: "Ghost, stop taking
notes" and "Ghost, mark that", which flags the moment in the notes. Each is answered with a
sound, not speech. Nothing is said into the call, and questions wait until it ends.

**How it ends.** Whichever comes first ends it, then a sound plays and the summary starts:
- the meeting app closes its mic stream (the call hung up), after a 60 s grace for a
  rejoin; this counts only once a mic stream has opened, so asking before joining is fine;
- "Ghost, stop taking notes", or Stop in the desktop;
- 20 minutes with no speech on either stream, counted from the start, so a lobby or a
  briefing needs no special case;
- 90 minutes from the start.

**Where the notes go.** The notes are a Markdown file in Ghost's outputs (ADR 0033),
written through the gate. The file is named by date and time (`2026-10-29-1405-meeting.md`)
because a file name never comes from model output; the drafted title is the heading inside.
An item also goes on the waiting list (ADR 0029). Ghost reads the notes aloud only when
asked.

**Meetings are kept until the operator deletes them** and belong to no agent: like outputs,
none keeps a copy in its memory. An in-tree read tool lets any agent answer "what did we
decide on Tuesday?". The transcript it returns is untrusted text.

**A summary that can't be written waits.** The transcript is already saved. If the model is
unreachable or the keyring is locked, the waiting list says "notes taken, summary pending"
and Ghost retries. After an hour of failures the item says so and keeps the transcript.

**Action items are not acted on.** Ghost lists them. Filing one as an issue, or adding to
the notes afterwards (`outputs.replace`), is an ordinary change the operator asks for in a
session, and it goes through the gate.

## Considered options

- **Screenshots of the call** for the attendees: deferred by the operator; audio only for now.
- **Cloud speech-to-text:** rejected for the same reason as ADR 0026. Parakeet runs faster
  than real time here, and a meeting's audio is other people's voices.
- **Recording the audio and transcribing at the end:** slower to a summary, and it leaves an
  audio file of other people on disk.
- **A debrief after hang-up**, spoken during the grace, as its own section: wanted later,
  left out of v1 (#44).
- **A silence clock that starts at the first speech from the call:** simpler as a flat 20
  minutes from the start.

## Consequences

- A new state beside passive, session and hold. The product says **taking notes** and
  **meeting notes**, never *recording*, since no audio is kept. "Record this meeting" still
  starts one. GLOSSARY.md gains **Meeting** and **Briefing**.
- Desk only in v1. The phone shows "Taking notes" and the notes on its waiting list, but
  can't start a meeting.
- Other people's words now reach the model provider as text. Before this, only the
  operator's own did.
- Consent is the operator's. Ghost gives the one-line reminder and shows the state the
  whole time.
- Parakeet runs on two streams for up to 90 minutes. Its CPU cost is measured (ADR 0018)
  when this is built.
- Built in the Rust daemon (ADR 0039), merged in #40.
