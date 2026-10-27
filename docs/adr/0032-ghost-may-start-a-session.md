# 0032: Ghost may start a session: it greets, waits, and tells only on a reply

- **Status:** accepted (decided 2026-10-27; built: `ghostd.routes.DeskRoute`, the reply window, `Phase.ASKING`)
- **Date:** 2026-10-27
- **Amends:** ADR 0010 (a session may start without a wake phrase)

Until now every session started with the operator: a wake phrase, a click, or a call from
the phone. To reach the operator at the desk (ADR 0031), Ghost has to speak first. The
operator decided on 2026-10-27 how.

## Decisions

**Desk voice is off until the operator enables it** in the route policy.

**Ghost greets and says nothing about the item.** A chime and a short greeting from the
policy ("Eric, got a minute?"), then a reply window (15 s unless set). The content waits
for a reply, so it is not spoken to a room the operator isn't in.

**The reply window is passive.** Local STT only; nothing heard is kept or sent unless it is
a reply. A reply opens an ordinary session with the item in context, and is the
acknowledgment. "Not now" snoozes the item. Silence ends the window, the desk goes back to
passive, and the item goes to the next enabled route at once.

**It never interrupts.** Not a running session, not a hold, not a phone session; the item
waits for the session to end or goes to the next route.

## Consequences

- The daemon's passive loop gains a fifth thing to wait on: Ghost has something to raise.
- `Status` shows the window as its own phase, so the apps can show "Ghost is asking".
- The phone's call (the desk rings the phone) is the same idea on another transport; it
  waits for push.
