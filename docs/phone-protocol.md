# Phone protocol

This is the wire format between `apps/mobile` and `ghostd`'s phone listener (ADR 0016). The
listener is plain HTTP on every address in `GHOST_PHONE_LISTEN`, comma separated: the LAN address
first, then the operator's tunnel address for use away from home (ADR 0020), for example
`192.168.1.20:8765,10.8.0.4:8765`. The same app serves on each. JSON bodies are UTF-8.

## Pairing

Ghost pairs with one phone, the operator's own. `phone.json` holds at most one device, and
pairing again replaces it: the old phone's signatures stop working at once.

The desktop app calls the control API (Unix socket, ADR 0013):

| Route | Result |
|---|---|
| `POST /phone/pairing` | `{code: "482913", address: "192.168.1.20:8765", addresses: [...], expires_at, link}`. `address` is the first listen address, the one the operator types; `addresses` is every one; `link` is the QR code's payload (below). A new code replaces the last one. 409 if the listener is off. |
| `GET /phone` | `{address: string\|null, addresses: string[], listen: [{address, network, listening}], device: {id, name, paired_at, last_seen\|null, last_address\|null, last_network\|null}\|null}`. See [Reachability](#reachability). |
| `DELETE /phone` | `{ok, detail}`. Forget the paired phone. Its signatures stop working at once, and a phone session that is running ends. |

### The QR code

The desktop shows the pairing as a QR code beside the address and the code. Its payload is a link in
the phone app's own scheme (`scheme` in `apps/mobile/app.json`), so the phone's system camera can open
the app with it too:

```
ghost://pair?code=482913&address=192.168.1.20%3A8765&address=10.8.0.4%3A8765
```

`code` is the six digits. `address` repeats once per listen address, in the configured order (home
first, then the tunnel); each is `host:port`, form-encoded, an IPv6 host in brackets. The daemon builds
it (`ghostd::phone::pairing`), and `@ghost/client-ts/pairLink` builds and parses the same string. The
phone accepts only exactly this shape: scheme `ghost`, host `pair`, one six-digit `code`, one to eight
addresses; it ignores other keys, which are left for later versions. The link carries what typing
would and nothing more: no secret, and the code is still single use, 120 s and five tries.

Scanning in the app pairs at once, trying each address in order and moving on only when one doesn't
answer (a wrong code stops, since the desk counts every try). A link the system camera opened shows
the addresses and pairs only after a tap, since a link can come from anywhere. Typing the address and
the code stays the fallback.

The phone sends `POST /pair` to the listener, unsigned:

```json
{ "code": "482913", "device_name": "Pixel 9" }
```

`200` returns `{device_id, secret, workstation, addresses}`. `secret` is 32 random bytes, base64url without padding.
`workstation` is the desk's hostname. `addresses` is every address the listener serves, in the configured
order; the phone keeps the list and, whenever it has no address in use, asks all of them and keeps the
first in the list that answers (home before tunnel). The phone stores the secret in secure storage, and the desk keeps it
in `~/.local/share/ghost/phone.json` (mode 0600). A code is single use and valid for 120 s. After five
wrong tries the code is spent. A wrong or spent code returns `403 {error: "bad_code"}`.

### Reachability

Each listen address is served by its own server, so the desk knows which one a request came in on.
Every signed request that verifies (not `/pair`, not a refused one) records the time, that listen
address, and its network in `phone.json`: written at most once a minute, or at once when the address
or network changed. `GET /phone` returns them as `last_seen`, `last_address` and `last_network`.

The network is `home` or `tunnel`, from the interface that holds the local IP the connection landed on
(`ghostd::phone::networks`): an interface the kernel calls WireGuard (`DEVTYPE=wireguard` in
`/sys/class/net/<name>/uevent`) or any other tun device is the tunnel, any other interface is the home
network. Nothing is configured and nothing is read from the order of `GHOST_PHONE_LISTEN`. `listen`
gives each configured address the same way, with `listening` false while its interface isn't up (it is
retried). `network` is `null` when Ghost can't tell: a wildcard address (`0.0.0.0:8765`), or one no
interface of this computer has held since the daemon started. An address that was up and went down
keeps the network it had. The desktop says "Reached Ghost 3 min ago over your home network" or
"through your tunnel", and when every address is on the home network it shows a short guide to
reaching Ghost away from home (ADR 0020).

## Signing

Every other route is signed. Request headers:

| Header | Value |
|---|---|
| `X-Ghost-Device` | the device id |
| `X-Ghost-Time` | Unix seconds, as an integer |
| `X-Ghost-Nonce` | 16 random bytes, hex |
| `X-Ghost-Signature` | base64url(HMAC-SHA256(secret, string-to-sign)), no padding |

The string to sign is these lines joined by `\n`:

```
ghost-v1
<METHOD>
<path and query, exactly as sent>
<X-Ghost-Time>
<X-Ghost-Nonce>
<hex sha256 of the body; of the empty string when there is none>
```

The desk returns 401 `{error: "unauthorized"}` if the device is unknown, the time is more than 60 s off,
the nonce was already seen in the last 120 s, or the signature doesn't match. A nonce is remembered only
once its request verified.

Each response to a signed request, including errors, is signed back with headers `X-Ghost-Time` and
`X-Ghost-Signature`, over:

```
ghost-v1-response
<HTTP status code>
<the request's X-Ghost-Nonce>
<X-Ghost-Time>
<hex sha256 of the response body>
```

The phone drops any response whose signature doesn't check. Above all, it never applies an SDP
answer that doesn't verify. A 401 for an unknown device carries no signature, because the desk has no
secret to sign it with; to the phone it is the same as any unverified response.

The live stream (`GET /stream`, below) is the one exception to signing the response's body: its head is
signed as above over an empty body (the hash of the empty string), and every message on it carries its
own signature. A non-200 answer to `/stream` is an ordinary signed JSON error.

Errors are `{error: <code>}`:

| Code | Status | When |
|---|---|---|
| `unauthorized` | 401 | Not signed, signed wrong, a stale time or a replayed nonce, or a device that isn't paired. |
| `bad_code` | 403 | `/pair` only: a wrong or spent code. |
| `bad_request` | 422 | A body or query that doesn't fit (a missing `id`, a key too long, an unknown `answer`). |
| `bad_offer`, `bad_candidate` | 400 | `/offer` and `PATCH /offer`. |
| `not_found` | 404 | No such route, question or `pc_id`. |
| `no_such_line` | 404 | `/offer` names an agent that is not a live one. |
| `method_not_allowed` | 405 | A route with another method. |
| `busy` | 409 | `/offer` while any session runs. |
| `settled` | 409 | A question already answered, declined, expired or taken back: `{error: "settled", state: "answered"}`. |
| `not_waiting` | 409 | `/changes/*` for a change that is no longer waiting for a yes. |
| `bad_answer` | 422 | An answer that doesn't fit the question (none of its choices when only they will do): `{error: "bad_answer", detail}`. |
| `not_ready` | 503 | `/offer` until the models load; `/stream` while the desk has no feed yet. |
| `internal` | 500 | Anything else. |

## Routes

Every route below is signed both ways ([Signing](#signing)).

| Route | Body | Result |
|---|---|---|
| `GET /hello` | | `{workstation, addresses, status: Status}`. `Status` is the control API's, including `session_origin: "desk" \| "phone" \| null`. `addresses` is the current list; the phone replaces its saved one when it differs. |
| `GET /audit?limit=N&offset=M` | | the control API's `AuditPage` |
| `GET /waiting` | | the control API's `WaitingView`: what needs the operator (ADR 0029). An item that is a question, or a change waiting for a yes, carries `question: QuestionView`. A read is not a delivery (ADR 0051): only `POST /seen` is. |
| `POST /waiting/ack` | `{key}` | `{ok}`. The operator heard it; it stops reminding until it changes. Not a gate write. |
| `POST /waiting/snooze` | `{key, minutes}` | `{ok}`. Back after `minutes` (1 to 10080). |
| `GET /conversations` | | the control API's `ConversationView` list: every agent a call can start with, Ghost first, retired ones left out (ADR 0014, ADR 0019). |
| `POST /offer` | `{sdp, type: "offer", pc_id?, restart_pc?, conversation?, item?}` | `{sdp, type: "answer", pc_id}`. `conversation` is a slug from `/conversations`: the session opens with that agent, which greets as itself; absent, with Ghost. `item` is the `key` of a ring the phone answered ([The ring](#the-ring)): the session opens with it. 404 `{error: "no_such_line"}` for a slug that is not a live agent. 409 `{error: "busy"}` while any session runs, desk or phone. 503 `{error: "not_ready"}` until the models load. |
| `PATCH /offer` | `{pc_id, candidates: [{candidate, sdp_mid, sdp_mline_index}]}` | `{ok: true}` (trickle ICE). `candidate` may keep its `candidate:` prefix; an empty one means end of candidates. |
| `POST /end` | | `{ok: true}`. Ends the phone's session if one is running. |
| `POST /listening` | `{listening, minutes?}` | `{listening, until}`. The desk's microphone off or on (ADR 0057); `minutes` (1 to 1440) only with `listening: false`, else until turned back on. A state, not a toggle, so sending it twice is harmless. 422 for a body that doesn't fit. While it is off `/hello`'s `status.not_listening` is set; a call from the phone still works, on the phone's own microphone. |
| `GET /stream?cursor=C` | | The live stream, Server-Sent Events ([The live stream](#the-live-stream)). `cursor` is optional; it is part of the signed path. |
| `GET /questions` | | `QuestionsView`: `{questions: QuestionView[]}`, the open ones oldest first (changes waiting for a yes among them), then the ones settled in the last day. |
| `POST /questions/answer` | `{id, key, text}` | `{ok: true, question: QuestionView}`. The operator's answer, their words or a choice as written, recorded with the route `phone`. `text` is 1 to 2,000 characters. 404 `not_found`, 409 `settled`, 422 `bad_answer`. A change's key is refused: confirm or cancel it at `/changes`. |
| `POST /questions/decline` | `{id, key}` | `{ok: true, question: QuestionView}`. The operator will not answer it. |
| `POST /changes/confirm` | `{id, change}` | `{ok: true, outcome: OutcomeView}`. `change` is `question.change_id` (the proposal's id). The gate's own confirm by id; the audit records the yes with `classifier: "phone"`. It is evidence only because the request is signed by the paired device. 409 `not_waiting`. |
| `POST /changes/cancel` | `{id, change}` | `{ok: true}`. The change is cancelled; nothing ran. 409 `not_waiting`. |
| `POST /seen` | `{alert_id}` | `{ok}`. The phone showed an `alert`'s notification. This, and only this, is the phone route delivering (ADR 0051). `ok` is false for an alert the desk stopped waiting for. |
| `POST /ring` | `{ring_id, answer: "answered" \| "declined"}` | `{ok}`. The operator answered or declined a `ring`. `ok` is false once the ring is over. |

`id` is chosen by the phone, 1 to 64 characters (a UUID will do), one per action the operator took.
The desk remembers it for ten minutes: the same `id` again gets exactly the answer it got the first time
and does nothing twice, so a request the tunnel dropped is simply sent again with its `id`. Only the
four routes above that list `id` in their body take one. `/waiting/ack` and `/waiting/snooze` have no
`id` (an extra field is ignored, not remembered); sending either twice is harmless, a second snooze only
moving the time it comes back. `/seen` and `/ring` are keyed by their `alert_id` and `ring_id`: a second
one answers `{ok: false}`.

A `POST /offer` without `pc_id` starts a session; with the live session's `pc_id` it renegotiates. A
second `POST /offer` without `pc_id` while the phone's own session is still running gets 409 `busy` too:
a phone that lost track of its session sends `POST /end` first and retries once `/hello` shows `passive`.
The session ends when the phone closes the peer connection, stops sending `ping` for 5 s, sends
`POST /end`, or says goodbye to Ghost. The pipeline ending closes the connection from the desk side.
After a peer drops, the desk can take about 10 s to wind the session down. A peer that hasn't connected
30 s after its offer was accepted is dropped.

## The data channel

The phone creates one data channel labelled `chat` before making the offer. It sends the text `ping`
once a second (the keepalive the desk expects), then RTVI messages `{label: "rtvi-ai", type, id, data}`:

- `client-ready` `{version: "2.1.0", about: {library: "ghost-mobile"}}`, sent once the channel opens.
- `disconnect-bot`, which ends the session (the same as `POST /end`).

The desk ignores every other client message type.

The desk sends `bot-ready`, the speaking events (`user-started-speaking`, `user-stopped-speaking`,
`bot-started-speaking`, `bot-stopped-speaking`), and `server-message` whose `data` is one control API
event, exactly as the WebSocket would send it: `status`, `turn`, `heard`, `pending`, `outcome` or `conversation`.
`turn` events carry the operator's words as each stretch of speech is read; the phone holds those
pieces until `heard` says the turn closed and shows its `text` in their place, the words the model was
given (ADR 0058); `heard` with `reopened` true takes the last closed turn back into the one still open.
Log events, Ghost's voice level (`voice`), the session's `activity` and the words being heard
(`saying`) are never sent. Server messages start after `client-ready`: first the turn, pending, outcome and
conversation events from before the channel opened, then a fresh `status`, then live events. Ghost's greeting
is spoken once the peer connects. The phone builds its state from these messages and doesn't use RTVI's own
transcripts.

## Audio

The phone sends one Opus mic track from `getUserMedia({audio: true})`, with the platform's echo
cancellation, noise suppression and gain control. The desk resamples it to 16 kHz and sends Kokoro's
24 kHz speech back on one track, with the thinking sound mixed in. Mute disables the phone's track and
leaves the session open. Both ends use `iceServers: []`.

On speakerphone some of Ghost's voice leaks past the phone's echo canceller and comes back late, after
Ghost has stopped speaking. The desk drops a transcript heard during Ghost's speech or up to 3 s after it
(`GHOST_ECHO_WINDOW_SECS`) when it is mostly a run of what Ghost just said (`ghost_voice::echo`). Answers
under three words always pass, so a spoken yes is never mistaken for echo.

End sends `disconnect-bot` on the data channel, then `POST /end`, then closes the peer.

## The live stream

ADR 0051. One long-lived signed stream from the paired phone to the desk, so the phone hears at once
what it used to poll for. It carries the waiting list's changes, every question as it is asked and
settled, alerts, rings and a heartbeat. It never carries audio, a transcript, a turn or a log line.

### Opening it

`GET /stream` (or `GET /stream?cursor=<cursor>`), signed like any request, with `Accept:
text/event-stream`. The answer is `200` with `Content-Type: text/event-stream` and the headers
`X-Ghost-Time` and `X-Ghost-Signature` signed over an empty body:

```
ghost-v1-response
200
<the request's X-Ghost-Nonce>
<X-Ghost-Time>
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

The phone keeps that nonce: every message on this stream is signed over it. Anything but 200 is an
ordinary signed JSON error (401, 503), except a 401 for a device the desk doesn't know, which carries
no signature (as on every route).

### Each message

Each is one Server-Sent Event:

```
event: question
id: 3fa9c2d1.42
data: {"msg":"{\"type\":\"question\",\"question\":{...},\"seq\":42,\"cursor\":\"3fa9c2d1.42\"}","sig":"q3Vb…"}

```

`data` is a JSON object with two strings: `msg`, the message as JSON text, and `sig`, base64url without
padding of HMAC-SHA256(secret, string), where the string is these lines joined by `\n`:

```
ghost-v1-event
<the stream request's X-Ghost-Nonce>
<hex sha256 of the exact bytes of msg>
```

The phone checks `sig` over `msg` exactly as received, then parses `msg`. A message that does not
verify is dropped, and the phone closes the stream and opens a new one. `event:` repeats `msg.type`.
`id:` is present on numbered messages and is their `cursor`. The keys inside `msg` come in
alphabetical order (`type` is not first), which is one more reason never to rebuild `msg` before
checking it. `packages/api/phone-vectors.json` has signed requests, responses and whole frames,
written by the daemon's own code (`crates/ghostd/src/phone/vectors.rs`); the phone's tests read it.

### The cursor, the snapshot, and nothing lost

A numbered message has `seq` (1, 2, 3 … in this run of the desk) and `cursor`, `<epoch>.<seq>`, where
`epoch` names this run of the desk. The phone keeps the cursor of the last numbered message it
handled.

On connect:

1. `hello` first, always.
2. Without a cursor, or with one the desk cannot resume (another epoch: the desk restarted; or older
   than the 512 changes it keeps), `hello.resumed` is `false` and a `snapshot` follows: the whole
   waiting list and every question as they are now, with the cursor they stand at. The phone replaces
   its state with the snapshot.
3. With a cursor the desk can resume, `hello.resumed` is `true` and the numbered messages after that
   cursor follow, in order, then live ones. No snapshot.
4. Then every change as it happens, and a `heartbeat` after 15 seconds without one.

A change between the cursor a snapshot reports and the snapshot itself may arrive again after it. Every
message says the whole of what changed, so applying one twice is harmless.

The phone treats the stream as dead after 35 seconds without any message (two heartbeats and a margin),
closes it and opens it again with its cursor, backing off 1, 2, 4 … up to 30 seconds while the desk does
not answer. Both listen addresses (home and tunnel) serve the same stream.

### Message types

```json
{"type": "hello", "epoch": "3fa9c2d1", "resumed": false, "heartbeat_secs": 15}
```

```json
{"type": "snapshot", "cursor": "3fa9c2d1.41",
 "items": [WaitingItemView, …], "questions": [QuestionView, …]}
```

`items` are the control API's `WaitingItemView`s, as `GET /waiting` gives them; `questions` as
`GET /questions` gives them. An item's `place` (a closed set: `model`, `sound`, `passwords`, `outside-tools`, `apps`, `watching`, only on Ghost's own notes) says where on the desktop it is dealt with; the phone may show it as words and never as a link.

```json
{"type": "item", "change": "added", "key": "watch:ci:main", "item": {WaitingItemView},
 "seq": 42, "cursor": "3fa9c2d1.42"}
{"type": "item", "change": "changed", "key": "watch:ci:main", "item": {WaitingItemView},
 "seq": 43, "cursor": "3fa9c2d1.43"}
{"type": "item", "change": "resolved", "key": "watch:ci:main", "seq": 44, "cursor": "3fa9c2d1.44"}
```

`added` and `changed` carry the item whole (a new title, acknowledged, snoozed, delivered). `resolved`
means it left the list: remove it, and take down its notification.

```json
{"type": "question", "question": {
   "key": "question:0b00bdd56307", "kind": "question",
   "asked_by": "Ledger", "source": "agent:ledger",
   "question": "Should the October invoice go out today?", "detail": null,
   "choices": ["Yes", "No"], "choices_only": false, "urgency": "notify",
   "asked_at": "2026-11-11T09:00:00Z", "expires_at": "2026-11-12T09:00:00Z",
   "state": "open", "answer": null, "choice": null,
   "answered_via": null, "answered_at": null, "change_id": null},
 "seq": 45, "cursor": "3fa9c2d1.45"}
```

Sent when a question is asked and again when it is answered, declined, expired or taken back. `kind`
is `question`, or `change` for a change waiting for a yes (its `choices` are `Yes` and `No`, its
`change_id` the proposal's id). `state` is `open`, `answered`, `declined`, `expired` or `cancelled`.
`answered_via` is `desktop`, `phone`, `voice` or `ghost` (an expiry, or an app's question that ran out
of time). An answer given by voice is shown by `choice` and `state` only: its `answer` is always null on
the phone, since those are the operator's words from a session. `question`, `detail` and `choices` are
the asker's words: show them as text, never as markup, and always with who asked.

```json
{"type": "alert", "alert_id": "9c1e04a7b2d3", "keys": ["question:0b00bdd56307"],
 "titles": ["Ledger asks: Should the October invoice go out today?"], "count": 1,
 "seq": 46, "cursor": "3fa9c2d1.46"}
```

The phone route delivering (ADR 0031): show a notification for these items now, then send
`POST /seen {alert_id}`. The desk waits 15 seconds for it; without it, the items go to the next route.
Send `seen` only once the notification is actually shown. An alert replayed after a reconnect may be
shown and `seen` like a live one; `seen` for one the desk stopped waiting for answers `{ok: false}`.
An `item` added is not a reason to notify: the alert is, since it comes after the desk's quiet hours,
batching and do-not-disturb. A `WaitingItemView` has no urgency of its own; only a question does
(`urgency`), so that is what a notification's channel can follow.

While the phone's own call with Ghost is running (or about to start), the desk sends no `alert`, and
while any session runs it sends no `ring`: the routes hand the items to the next route at once, as
desk voice does during a desk session. The phone keeps its own guard as well: during its own session
it posts any notification silently and does not ring.

```json
{"type": "ring", "ring_id": "5d2f8e0c41aa", "key": "question:0b00bdd56307", "agent": "ledger",
 "title": "Ledger asks: Should the October invoice go out today?",
 "question": "Ledger asks: Should the October invoice go out today? Yes or no?",
 "ring_secs": 30, "seq": 47, "cursor": "3fa9c2d1.47"}
{"type": "ring_over", "ring_id": "5d2f8e0c41aa", "why": "answered", "seq": 48, "cursor": "3fa9c2d1.48"}
```

`question` is null when the item is not a question. `why` is `answered`, `declined` or `timed_out`. A
ring is live from its arrival until its `ring_over` or `ring_secs` later, whichever is first. A ring
carries no time of its own, so the phone cannot tell the age of one it first hears in a replay; what
it can rely on is that the desk always logs a ring's `ring_over` within `ring_secs`, so a ring
replayed after it was over is followed in the same burst by its `ring_over`. The phone therefore
waits a moment (400 ms) before ringing, and never rings a ring whose `ring_over` arrived in that
moment, one older than `ring_secs` by its own clock, or a `ring_id` it already rang.

```json
{"type": "heartbeat", "cursor": "3fa9c2d1.48", "at": "2026-11-11T09:01:15Z"}
```

Not numbered; its cursor is where the stream stands.

### Answering from the phone

- A question: `POST /questions/answer {id, key, text}` with a choice as written or the operator's own
  words (unless `choices_only`), or `POST /questions/decline {id, key}`.
- A change waiting for a yes (`kind: "change"`): `POST /changes/confirm {id, change: change_id}` for
  Yes, `POST /changes/cancel {id, change}` for No. Never `/questions/answer`.
- An item: `POST /waiting/ack {key}` or `POST /waiting/snooze {key, minutes}` as before.

What came of it arrives on the stream as well (`question` with its new state, `item` resolved), so the
phone may update its screen from either.

## The ring

ADR 0051, issue #22. The desk rings the phone for an item at `call` urgency, when the operator enabled
the `call` route, within the operator's call caps. It never rings while a session is running, the
phone's own call or one at the desk (an answered ring could not open a call beside it): the item goes
to the next route, and rings again on a later tick once the session is over if it still needs to.

```
desk                                                  phone
 |-- stream: ring {ring_id, key, agent, question} --->|  full-screen incoming call
 |                                                    |
 |   (answered)                                       |
 |<-- POST /ring {ring_id, answer: "answered"} -------|
 |-- 200 {ok: true} --------------------------------->|
 |-- stream: ring_over {why: "answered"} ------------>|
 |<-- POST /offer {sdp, type, conversation: agent,    |
 |                 item: key} ------------------------|
 |-- 200 {sdp, type: "answer", pc_id} --------------->|  the call connects as any call does
 |   Ghost opens with the item: a question is put in  |
 |   its asker's words ("Ledger asks: …? Yes or no?") |
 |                                                    |
 |   (declined)                                       |
 |<-- POST /ring {ring_id, answer: "declined"} -------|  "not now": the item is snoozed 30 min
 |-- stream: ring_over {why: "declined"} ------------>|
 |                                                    |
 |   (nothing for 30 s)                               |
 |-- stream: ring_over {why: "timed_out"} ----------->|  stop ringing; the item goes to the next route
```

Answering the ring is the acknowledgment. The call itself is an ordinary phone session: the operator's
spoken answer to the question is recorded by Ghost as they said it, and a spoken yes to a change is
judged as any spoken yes is. If the offer is refused (`busy`, `not_ready`), the phone says so on its call
screen; the item stays on the list.

## Types

`WaitingItemView`, `QuestionView`, `QuestionsView`, `OutcomeView`, `Status`, `AuditPage` and
`ConversationView` are the control API's, defined in `packages/api/openapi.json` (generated from
`crates/ghost-api`), and `packages/client-ts` has them as TypeScript types. The phone uses the same
names.

## What the phone app must do

- Hold the stream in a foreground service of its own, not the microphone service a call uses: type
  `connectedDevice` with a quiet persistent notification (`dataSync` is capped at six hours a day on
  Android 15 and may not start at boot). Open it when the app starts, when the network changes, and
  when the tunnel comes up. The app does this in Kotlin (`apps/mobile/modules/ghost-call`), so it runs
  with the app's screen and JavaScript gone.
- Keep the stream's request nonce, verify every message's `sig` before parsing `msg`, and drop and
  reopen on a failure.
- Keep the last cursor; reconnect with it; replace the state on a snapshot.
- Treat 35 seconds without a message as a dead stream; reopen with backoff up to 30 seconds; try every
  address from `/hello` in order.
- On `alert`: post a local notification per item (a question with its choices as notification actions,
  a change with Yes and No, anything else with Got it and Later), then `POST /seen {alert_id}`.
- On `ring`: a full-screen incoming-call notification (`CATEGORY_CALL`, full-screen intent) showing which
  agent is calling and nothing of what about until it is answered, ringing for `ring_secs` at most. Answer: `POST /ring answered`, then
  `POST /offer` with `conversation: agent` and `item: key`, through the existing call path. Decline:
  `POST /ring declined`. Stop ringing on `ring_over`.
- On `item resolved` or a question that is no longer open: take its notification down.
- Every action from a notification or a screen (answer, decline, confirm, cancel) is a signed `POST`
  with a fresh `id`, sent again with the same `id` until it gets a signed answer, for less than the ten
  minutes the desk remembers it. Acknowledge and snooze are sent again the same way; they have no `id`.
- Show a question's words and an item's title as text, with who asked. Never show or keep an answer
  given by voice (the stream does not send one).
- Never record or send audio outside a call, and never ask the desk for a transcript: there is no route
  for one.
