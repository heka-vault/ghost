# 0051: The operator is asked, and answers anywhere

- **Status:** accepted (decided 2026-11-11; built on the `feat/ask` branch: questions and their table, `ask`, `question`, `questions_list`, `question_cancel` and `answer` on Ghost's own app, the follow-up run, the apps' asker, the control API's `/questions`, the desktop's answer, the phone's live stream, the call route, the ntfy route; the Android side is to be built from [phone-protocol.md](../phone-protocol.md))
- **Date:** 2026-11-11
- **Builds on:** ADR 0029 (the waiting list), ADR 0030 (a change at `confirm` waits on it), ADR 0031 (the routes), ADR 0032 (Ghost may start a session)
- **Amends:** ADR 0016 (the phone listener gains a live stream and signed answers), ADR 0031 (the phone route is delivered on the phone's word, not a poll; the call route and an ntfy route exist), ADR 0045 (five more tools on Ghost's own app; one only in a live session), ADR 0046 (the apps' asker is real), ADR 0047 (a run may ask and is followed up), ADR 0048 (an outside tool may ask), ADR 0018 (the cost, below)

An agent could put an item on the operator's waiting list but could not ask them
anything and hear back. A run with nobody there could only set a change aside. An app
asking the operator something in the middle of a call was declined. And the phone heard
nothing unless its app was open and polling, so the phone route was "delivered" when the
phone had read the list in the last minute, which is a guess. OpenClaw has `ask_user` and
forwards approvals to chat; Hermes Agent has `clarify` with a few choices and "Other", and
requests to the client that are replayed when it reconnects. This takes the shape of both
and keeps Ghost's rules.

## Decision

**A question is an item with an answer.** It is on the waiting list like any item (key
`question:<id>`, source its asker), so it alerts, escalates, keeps quiet hours and
do-not-disturb, and reaches the operator by the routes they enabled. What an item does not
have is in the `questions` table (`ghost_core::questions`): who asked (`agent:<slug>`,
`client:<name>`, `app:<name>`) and as whom they are shown, the question and its detail, up
to four choices and whether only they will do, the urgency, when it expires (a day unless
said, a week at most), and its state: open, answered (with the answer, the choice it
clearly is, the route it came from and when), declined, expired or cancelled. Answering is
its acknowledgment and its resolution in one step. It survives a restart; an app's
question open at a restart is declined, since the app's call is gone.

**Asking** is `ghost_ask` on Ghost's own app, a change through the gate (quiet unless a rule
says otherwise, as Ghost's own changes are, ADR 0045):

- In a voice session the model asks aloud; `ask` is for an answer that may come later, and
  returns the key at once (`wait_s` is ignored). The prompt says so.
- From a run with nobody there it does not hold the run. `wait_s` (up to 120) may wait;
  otherwise the run is told to finish `[SILENT]`, and when the operator answers, one more
  run of the same agent is queued with the question and the answer as its instruction
  ("You asked the operator … The operator answered (phone): …"), delivered like any run.
  That instruction says the answer is already recorded and the question closed, to do what
  the answer calls for, and to reply exactly `[SILENT]` when nothing remains (a "no" that
  needs nothing), so the operator is not told what they just said. The follow-up is not
  offered `ghost_answer` or `ghost_ask` (`questions::FOLLOW_UP_WITHHELD`, refused by name
  too), and what it delivers is titled by the question it follows up. One per answer: it is queued once for the question's key, through the scheduler's own
  once-per-key rule, so neither a second answer nor a restart starts another. None for a
  decline or an expiry. The answer is never a yes to a change: the run's changes still go
  through the gate.
- From an outside tool it returns the key; `events_wait` says when it is settled and
  `question(key)` reads the answer. `wait_s` may hold the request up to 120 seconds.
- `question`, `questions_list` and `question_cancel`: an asker reads and takes back only its
  own; Ghost (agent zero) reads all.

**What counts as an answer, on each route, and why a model never supplies one.** The
answer is the operator's words or the choice they picked, from a surface only they hold:

- **Voice.** `ghost_answer(key, text)` is offered only to Ghost's own agents. The session's
  dispatcher sets `_heard` to its session id, and only when the answer's words appear in
  the operator's own last turn (`ghost_core::rules::said`, the check `rules.add` makes).
  The server refuses an answer without it, in a run, from an outside tool (where the tool
  is not even listed), and from the agent that asked the same question in the same
  session. The model can choose which question the words answer; it cannot make up the
  words, and they are recorded as said, never paraphrased. A choice is matched by fixed
  rules (the confirmation allowlist and its negation guard for Yes and No), never by a
  model.
- **Desktop.** A click on a choice, or the operator's typed words, on Home or on a desktop
  notification's buttons (`notify-send --action`): route `desktop`.
- **Phone.** A signed request from the paired phone (`POST /questions/answer`): route
  `phone`. It is evidence because the request is signed by the paired device's secret.

At the desk, the digest puts a question rather than announcing it ("Ledger asks: should the
October invoice go out today? Yes or no?"), last in the line so the reply answers it, and
the note the model gets carries the question's key and says to record the operator's exact
words. Desk voice does the same once the operator has replied to its greeting (ADR 0032
still says nothing before a reply).

**Pending changes are questions too.** A change waiting for a yes (ADR 0030) is shown as a
question with the choices Yes and No, through the same API and the same phone protocol.
What counts as a yes does not change: the desktop's Yes is the Confirm it always was
(`classifier: ui`), and the phone's is the gate's confirm by id with `classifier: phone`,
evidence only because the request is signed by the paired device.

**Apps may ask.** `ghost_core::questions::AppAsker` is set before the apps connect, so Ghost
tells them it takes questions. A form-mode elicitation of one field becomes a question from
the app (`app:<name>`): a fixed list as its choices (only those will do), a boolean as Yes
or No, a string as a free answer. Anything else (two fields, a number) is declined, and
the log says why. It is held until the app's own limit (300 seconds, Hermes Agent's value)
and declined a second before it. The app's words are shown and said as the app's ("GitHub
asks: …"), never as Ghost's, and only the operator answers them.

**The phone hears at once.** `GET /stream` on the phone listener is Server-Sent Events, one
long-lived signed stream (`ghostd::phone::stream`): a snapshot, then the waiting list's
changes and every question asked or settled, an `alert` when the phone route delivers, a
`ring` when the call route rings, and a heartbeat every 15 seconds. Each message is signed
over the stream request's nonce and its own bytes. Given a cursor this run of the desk
still has, the phone gets only what it missed; otherwise a snapshot again. The phone
answers with signed `POST`s (`/questions/answer`, `/questions/decline`,
`/changes/confirm`, `/changes/cancel`, `/seen`, `/ring`), each idempotent by an `id` it
chooses.

Server-Sent Events, not a WebSocket: the request is an ordinary signed `GET`, so the
existing middleware verifies it unchanged and only the response is streamed; the stream
is one-way, which is what the desk needs, and everything the phone says is a short signed
request that already works and is retried by `id`. Behind a WireGuard tunnel that drops,
the phone simply opens the stream again with its cursor; there is no upgrade, no frame
masking and no second signing scheme for messages going up. OkHttp's SSE client, which
the Android app can hold in a foreground service, does this directly.

**The phone route is delivered on the phone's word.** `PhoneRoute` sends an `alert` on the
live stream and waits 15 seconds for `POST /seen`, which the phone sends once it has shown
the notification: delivered. No stream, or no `seen`: unavailable, and the item goes to
the next route at once. A read of `/waiting` is no longer a delivery.

**The desk rings the phone** (issue #22). `CallRoute` (off unless `call` is in
`[routes].enabled`, under the operator's call caps) sends a `ring` for the first item at
`call` urgency, with the agent to open and the item's key. The phone shows a full-screen
incoming call. Answered (`POST /ring`), the phone opens an ordinary session through
`/offer` naming that agent and `item`, and Ghost opens with it: a question is put in its
asker's words, anything else said in a sentence. That is the acknowledgment. Declined is
"not now": snoozed for 30 minutes. Not answered in 30 seconds: nobody heard, the next
route at once.

**A route that needs no tunnel: ntfy** (off unless the operator enables it). `NtfyRoute`
posts to the operator's ntfy topic (self-hosted or ntfy.sh). The topic's address and its
token are passwords in the vault, named in `[routes]` (`ntfy`, `ntfy_token`); the API
carries the names, never the values. By default a push says "Ghost: something is waiting
(2: 1 from a watch, 1 from an agent)", the count and the kind of each source and nothing
else (direction.md: what leaves the machine through a third party is the least that
works); `carry = "title"` lets titles through. It is held to `ntfy_per_hour` (4), it can
never be acknowledged, so it is the last route on any ladder louder than the digest.

**The ntfy exception.** The operating manual says never to write a bespoke client for a
service: connect an app. ntfy is plain HTTPS with `reqwest` inside the routes code, and
that is deliberate: a route the daemon starts on its own must not wait on an app being
connected, signed in or up, and must not put a model or a tool list between the waiting
list and the operator's pocket. The exception is this one route, it sends only what the
route policy allows, and it never reads anything back.

## Consequences

- Nothing on the phone path is a transcript. The stream carries the waiting list, the
  questions, alerts, rings and heartbeats; a question answered by voice goes to the phone
  by its choice and state, never its words; there is no route for a turn, a transcript,
  the event stream or audio (asserted by `the_phone_surface_carries_no_transcript_and_no_audio`).
- A question's words are the asker's on every surface: data, shown as text, said as "X
  asks: …", never as Ghost's own.
- The audit tells the routes apart: a change confirmed from the phone has `classifier:
  phone`, from the desktop `ui`, by voice the classifier that judged it.
- Cost on a turn (ADR 0018), measured by `what_the_question_tools_add_to_every_turn`:
  five tools and **2,465 bytes** of JSON schema (`ghost_ask` 1,335, `ghost_answer` 421,
  `ghost_question` 276, `ghost_questions_list` 199, `ghost_question_cancel` 234), about 620
  tokens, in the cached prefix of every session and run; the test fails past 3,200 bytes.
  Ghost's paragraph of the prompt gains two sentences, 200 bytes. An outside tool sees
  four of the tools (not `answer`).
- An idle phone costs one open HTTP connection and a heartbeat every 15 seconds; the desk
  keeps the last 512 changes for it to resume from, in memory.
- A ring waits up to 30 seconds and a phone alert up to 15 inside the routes' tick; the
  desk route already waited up to two minutes, so the tick was never short.

## Alternatives considered

- **A parallel list of questions.** It would need its own routes, quiet hours and
  escalation. An item with an answer has all of them already.
- **Let the model fill in the answer from the conversation.** It would be convenient and
  it would be the model's words. The verbatim check is the one rule that keeps the answer
  the operator's.
- **A WebSocket for the phone.** Two-way, but the upgrade bypasses the signed-response
  path, and every message up would need a signing scheme of its own; the signed `POST`s
  already exist.
- **Push through Firebase.** It would wake a phone off the tunnel, at the price of a
  Google account in the path and content in a third party's hands. ntfy is the operator's
  choice of server and carries a count.
- **Block the run until the operator answers.** A run holds an agent and a model
  connection; an answer can take hours. A follow-up run costs nothing while it waits.
