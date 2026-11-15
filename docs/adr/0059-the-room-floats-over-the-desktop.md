# 0059: The room floats over the desktop

- **Status:** accepted
- **Date:** 2026-11-12
- **Amended:** 2026-11-12 (a voice, not captions), 2026-11-13 (sharing, the stage), 2026-11-14 (the first run; no room in the window, which supersedes every mention of a room held or shown in the window above)
- **Builds on:** ADR 0013 (the control API and its events), ADR 0038 (the daemon ships inside the desktop app), ADR 0055 and ADR 0058 (what the desktop sees of a session), ADR 0057 (not listening)
- **Amends:** ADR 0013 (one more event, `show_app`)

The owner chose two postures for the app on 2026-11-12: the briefing at the desk and the room
during voice (docs/design-system.md, The two postures). The spike of the same day (PR #84,
docs/design/floating-2026-11.md on its branch) showed that the room can leave the window: a
transparent surface over the desktop that is a thin line of light until Ghost's name is
called, comes forward over whatever the operator is doing, and goes back when the session
ends. The owner: "Build it once the new frame has merged." Their words for the feel:
"communicating with and wielding the power of a super computer".

## Decision

**A third posture, `float`, opt-in and off by default.** The one rule in
`apps/desktop/src/app/posture.ts` decides it: a live session at this desk floats when the
operator turned the floating room on, it can show now, and the main window is not the window
the operator is using (focused and on screen). A room held in the window still wins. Off, or
where it cannot show, nothing changes. The main window, the floating surface and the
conversation screen's room view (which no longer keeps its own timer) all read that rule.

**A second process of the desktop app, through X11.** `heka-ghost-desktop --floating`, started
by the shell with `GDK_BACKEND=x11` when the switch is on and stopped when it is off (its stdin
closes). On X11 and XWayland one window can keep itself above the others
(`_NET_WM_STATE_ABOVE`), be on every workspace (`STICKY`, `_NET_WM_DESKTOP` all), stay off the
taskbar, the pager and the switcher, refuse the keyboard (`WM_HINTS` input false, never
mapped with focus), and take clicks only where it draws (an X input shape: empty at rest,
the buttons while forward). GNOME's native Wayland gives an ordinary client none of that, so
the main window stays native and only this one goes through XWayland. Where there is no X11
(a Wayland session without XWayland) the switch is refused with a sentence; a broken native
Wayland mode is not shipped. The surface reads the daemon's events through the same bridge as
the main window; it changes nothing the main window cannot.

**It never takes the keyboard and every answer has two ways.** A change waiting for a yes and
an agent's question show their buttons (a click is the operator's own, `classifier: ui` as in
the window, ADR 0013); the voice path is unchanged. "Open in Ghost" brings the main window to
the operator.

**It is honest about the microphone.** The line at rest is the listening sign; with listening
off (ADR 0057) it is a struck red microphone. While forward, the operator's turn is a sign
from `you_started`, never a level and never words while they speak (#73, ADR 0055).

**It hides from screen capture.** The shell asks mutter every two seconds while the switch is
on whether a screen-cast or remote-desktop session exists (introspection of
`/org/gnome/Mutter/ScreenCast/Session` and `/org/gnome/Mutter/RemoteDesktop/Session`, open to
any session client, no portal prompt). One exists: the surface draws nothing, not even the
line, and a spoken session shows in the window. Where that can't be read (another desktop,
or a client grabbing XWayland's pixels directly) Settings says so and the tray's "Float over
the desktop" turns it off at once.

**A focused full-screen app is left alone.** The monitor it fills is not chosen; with no other
monitor Ghost does not come forward (the voice goes on, the line is under the app).

**Which monitor is a pure function of what is known** (`floating::place::choose`): the
operator's choice, "the other monitor" (default), a named one, or "where I am". Where the
operator works is the focused window's monitor, else the pointer's. Under XWayland the X11
layer cannot tell either while a native Wayland window has the focus, so only a focused X
client counts; the rest comes from an optional GNOME Shell extension shipped with Ghost
(`apps/desktop/gnome-shell/`, positions only, installed by the operator). Without it "the
other monitor" is the one that isn't primary. With one monitor Ghost comes forward on the
half away from the work when it knows where that is.

**`Event::ShowApp { session_id }`**: "Ghost, open the app" (or "show me the window", a quick
intent with no model) asks the desktop to bring its window to the operator. The daemon
publishes it inside a session only; it carries nothing else; the phone does not get it. The
shell raises the window by unmapping and mapping it again, which puts it on the current
workspace and in front on GNOME, where a plain raise would switch workspace or only flash
"Ghost is ready". The tray's Open Ghost and the surface's button do the same.

## Amended 2026-11-12, evening: a voice, not captions

The owner used it live the same day: "good, not 9/10 good". The reply covered the work in 80 px
type and ran off the bottom of the screen, under a label that said "Listening to you". Their
words: "I don't really think I need to read what I just said, and if I can hear Ghost I don't
need to read what they said either", and of the working state, "I like the idea of seeing where
Ghost is looking for things". So:

- **No transcript.** The floating room shows neither the operator's words nor Ghost's reply.
  It shows the state (Wisp, the light and its colour, a few words under Wisp), where Ghost is
  looking, and only what sound can't carry: a change waiting for a yes and an agent's question,
  in a fixed card. Captions are a setting (off, when the sound is off, always; off by default),
  Ghost's own words, two lines. "When the sound is off" reads the default output's volume and
  mute with `wpctl` once per reply, read only; nothing changes the sound.
- **Where Ghost is looking is the working state.** Every app and agent the turn reaches for
  is a named place with a beam (`src/floating/reach.ts`): lit while its call runs and 750 ms at
  least, the answer coming back as light in proportion to what it brought, then a quiet trace
  for the rest of the turn. Ghost's own lookups are places (Memory, Docs, Agents). Names are
  the app's and at most two words from the tool's own name, never an identifier or a sentence.
  The size of what came back is the length of `GET /calls/{id}`'s preview (ADR 0058), fetched
  once per finished call of the turn by the surface on this desk; its text is never shown. This
  widens ADR 0058's "fetched when the operator opens the call" to this one reader, for a number.
  The corner list of calls is gone: the beams carry the same truth.
- **The calls make the working state, not the phase.** The daemon's phase follows the audio, so
  while Ghost says "One sec." over its calls the phase is `speaking`, then `listening`; the
  beams, drawn in `thinking` only, never showed in real use. Calls in flight now make the scene
  `thinking` whatever the phase, Ghost's voice wins over a turn the VAD opened (the daemon
  interrupts Ghost when the operator's turn really starts), and the goodbye leaves once its
  voice has been quiet 0.7 s, and after 4 s at the most.
- **A layout rule.** Everything forward is inside the monitor's safe area and within a share
  of it (36% beside the work, 16% in the corner, 45% in the centre), scaled by the shorter side;
  nothing is sized from what is said (`src/floating/layout.ts`, tested on five monitor shapes).
  The dim is an oval around Ghost, not the whole desktop. **Placements**: beside the work
  (default; the owner, seeing it on the other monitor: "I liked Ghost showing up without taking
  the full screen"), in the corner, in the centre (`floating.json`'s `placement`). The
  full-screen dim is gone; the centre is the largest it gets.
- **Cost** (measured again, `docs/floating-room.md`): at rest 0.1% of a core; forward 12.6% to
  14.6% beside the work and in the corner, 16.5% in the centre, against about 13% before.
- **Sending it away**: "Ghost, that's all", **Done** under Wisp, or a desktop shortcut to
  `ghostd sleep`. Each ends the session (`POST /sleep`, as the window's own button).

The look, the states and the motion are the same: this takes text down, it doesn't change
the character.

## Amended 2026-11-13: while the screen is shared, and the stage

The owner: "What are our options for making Ghost be able to float in when we are doing screen
sharing? I wanted to share my screen because I actually wanted to show off Ghost." Hiding from
every capture was right as a default and wrong as the only answer. Measured on the workstation
(`docs/floating-room.md`, What a capture sees): a whole-monitor capture (GNOME's recorder,
mutter's monitor stream, which the portal gives a browser and OBS) carries the surface with its
transparency and dim on either monitor; a single-window share never does; and the surface shared
alone as a window arrives on black, since a meeting drops the alpha. So:

- **A choice, Hide by default.** "While my screen is shared": Hide Ghost (default, as before),
  Show Ghost, Ask each time (`floating.json`'s `while_shared`). The shell keeps one share's
  memory (`floating::share`, a pure state machine fed the capture and the clock) and sends the
  surface one word with the capture: `none`, `hidden`, `shown` or `asking`. Ask shows a small
  card on the surface's monitor with Show and Hide and nothing else of Ghost; unanswered for
  20 s it is taken as Hide, since the card is itself in the share. The tray's "Show Ghost in
  this share" flips what is in effect, for that share only. Shown, Ghost wears a "Screen shared"
  mark it cannot be without. The posture rule's `floating` input is "on, up, and not kept out
  of a share". A capture line from an older shell still means hidden.
- **The stage.** A window of the main process, "Ghost · Stage" (`stage.html`), drawing the same
  page on an opaque designed backdrop from the same events, opened from the tray or Settings: the
  thing to share as a window or record. It never hides (it was opened to be seen), captions
  Ghost's own words for people who may not hear it, has no Open in Ghost or Done, and needs no
  X11. It reads what the main window reads and changes nothing the main window cannot.
- **The audience's ears.** Not the daemon's business: a meeting sends a microphone, and on this
  desk the reSpeaker cancels Ghost's voice out of its own (measured: 5.4 dB above the room
  against 23.7 dB at the webcam's). `make demo-audio` makes, for the length of a demo, a source
  "Ghost and your microphone" that mixes the operator's microphone and Ghost's `ghost-playback`
  stream; Ghost's own input is untouched. Headphones are the advice, so the call can't wake
  Ghost or speak for the operator.
- **Moving between monitors.** Found while measuring: the surface, sized to the whole monitor,
  didn't fit the primary monitor's work area and mutter kept it where it was, and a window asking
  to cover a monitor became a legacy full-screen one that hid GNOME's top bar on the primary.
  It now leaves full screen and takes the monitor's work area.

Cost: nothing at rest or forward changes; the share's word rides on the existing two-second
poll and one line on the surface's stdin when it changes. The stage is a second webview while it
is open, costing what the floating room costs forward (not measured on its own); closed, nothing.

## Amended 2026-11-14: the first run turns it on, at the moment

The owner: the first run was "very 'blocking' and people would struggle to get started"; "reduce
the steps it takes to get to the first 'hey ghost' and them seeing Ghost in the floating room",
which they call the killer feature. Off until found in Settings › General, a newcomer could finish
setup and never see it. So:

- **The first run turns it on, once, by the person's visible choice.** Its wake beat ("Say “Hey
  Ghost”") shows one plain line with a switch, on: "Ghost floats over your work when you call it",
  and after the moment "Keep Ghost floating over your work". The shell turns it on when that beat
  first opens and records that it did (`floating.json`'s `offered`, `floating::offer`, tested), so
  it never turns it on again: someone who turns it off keeps it off, someone who skips the first
  run keeps today's behaviour, and an install that finished the first run before this change (its
  window's `finished`) is not offered it at all. Where the desktop can't float the offer is
  recorded and nothing turns on.
- **The moment floats over the window that called it.** While the wake beat is open the shell tells
  the surface (`first_meeting` on its stdin) and the posture rule takes one more input,
  `introducing`: a desk session floats even while the main window has the keyboard. Everything
  else about the rule holds: a share keeps Ghost out unless the operator chose to show it, a held
  room wins, and with no X11 the moment is the window's room view with the sentence the switch
  already uses. The surface says when it came forward (`forward` on its stdout, `ghost://floating-forward`
  to the window), so the first run says where it happened. On the surface, the first meeting adds
  one line under Wisp, Ghost's own: "Say “that’s all” to send me back".
- **Ghost's own first line.** The first run arms the daemon too (`PUT /first-meeting`, memory only,
  the operator's window only): the next desk session with Ghost takes it once and opens with a
  fixed line, said before the model is asked anything (`ghostd::first_meeting`, `Greeting::Intro`),
  so a slow model never delays the first thing a newcomer hears; a request said with the name is
  answered by the model after it. With no model connected the line says, in one sentence, that
  Ghost needs one and where to give it, and every turn of that session answers with Ghost's own
  sentence (a scripted stand-in that reaches no network and calls no tool), never a failed call.
  A phone's call, a call to an agent and a session Ghost raised itself never take it.

Cost: nothing on a turn; one PUT when the beat opens and closes, one stdin line to the surface,
one stdout line back. The surface is started at the wake beat instead of never, which is the
floating room's own cost (below) from then on, as for anyone who turned it on.

## Amended 2026-11-14: no room in the window

The owner, after testing 2026.11.1 with other people: "the UI is a little confusing with the
normal view and 'the room' view. I think we want to remove the room view and have the floating
view or the regular (daily brief…) view. The room is cool, but it's not as cool as floating, and
it's useful but not as useful as the daily brief view."

**Two postures, `desk` and `float`, and no third.** The in-window room is removed: the layer over
the desk (`Room.tsx`), the conversation screen's room view (`RoomView`), the hold and its key
(Ctrl+., the rail's button, Settings › General's "Show the room", Escape), and the delays that
brought it by themselves (20 s of quiet in a session, 1, 3 or 10 minutes stepped away). The rule
in `posture.ts` keeps one question: does a live session at this desk float? It floats when the
floating room is on and can show and the main window is not the one in use (or the first run's
wake beat is open); everything else is the desk. A stored choice from an older install
(`ghost.room.away`, `ghost.room.session` in the window's storage) is removed quietly at start;
the hold was never stored. Ctrl+. does nothing now.

What took the room's place, case by case:

- **A session while the floating room is on and can show, with the window not in use**: it
  floats, as before.
- **A session where it doesn't float** (switched off, no X11, a share Ghost is kept out of, a
  focused full-screen app, or the operator using Ghost's own window): the regular window. Today's
  talk bar becomes a live strip in the conversation screen's own words ("Listening to you",
  "Using GitHub and Mobbin", "Ghost is speaking", on hold, on the phone), in the phase's colour,
  with a change waiting for a yes (Confirm, Cancel) or a question asked in the session (its
  choices) right above it, left out of Now's lists while it is there. The rail's status card
  and the conversation screen's timeline (ADR 0055, ADR 0058) are unchanged. Nothing new is
  asked of the daemon: no event, no route.
- **Stepped away, no session**: the window stays as it is (Today, or whatever page was open).
- **"I want Ghost big and present"**: the floating room. The rail's room button is the floating
  room's switch (a toggle with one line in its tooltip; where the desktop can't float there is no
  button, and Settings › General says why beside its switch), and Settings › General's "The room"
  section is "The floating room".
- **The first run where it can't float**: the moment happens on the wake beat itself, under the
  lit mark: Ghost's state, its first line as it is spoken (said words bright, the rest dim) and
  one sentence saying why it answered here ("Ghost answered here. On this desktop it can't float
  over your work: …", or that the screen is being shared). `pathToMoment` is unchanged.

**What is lost**: Ghost legible from three metres inside a window. With the floating room off or
unable to show, a person across the room sees the window as it is at the desk; the floating
room, Ghost on stage and the phone are the ways to see Ghost from a distance.

Cost: less than before. No idle timer, no listener on every key, pointer move and scroll for the
posture, and one large layer fewer; the live strip reads the events the window already had.

## What it costs (ADR 0018)

Measured on the workstation (Intel UHD 750, GNOME Shell 50.1, XWayland, two 1920×1080
monitors; `docs/floating-room.md` has the method). At rest: 0.1% of a core for the surface,
no GPU above the desktop's baseline; the loop is stopped and the input shape empty. Forward,
every scene: about 13% of a core for the surface, 9% for Xwayland and 6% for gnome-shell
(the spike measured 32 to 51%, 20 to 29% and 9 to 17%), at 30 frames a second (60 for the
arrival's zoom), the GPU at 58% busy at about 400 MHz. Most of the saving is Wisp drawn at the
scene's frame rate (`Mascot`'s `maxFps`): Wisp redrawn at 60 was half the cost, since every
frame is the monitor's whole surface copied through XWayland. Nothing is added to a turn: the
event is one more broadcast per "open the app", and the quick intent is a phrase match.

## Consequences

- The surface is a second WebKit process while the switch is on: 450 MB resident across its
  three processes in a debug build serving the dev page (shared libraries counted in each);
  a release build was not measured.
- The ADR 0057 rule holds for it: nothing it does opens or closes the microphone.
- A screen share on a desktop that isn't GNOME is not seen; the tray switch is the answer. Showing Ghost in a share is the operator's choice, never a default, and it is marked on the surface.
- Native Wayland support needs layer-shell (KDE, wlroots) or a GNOME Shell extension that
  stacks the window, both later.
