# Phone protocol

This is the wire format between `apps/mobile` and `ghostd`'s phone listener (ADR 0016). The
listener is plain HTTP on every address in `GHOST_PHONE_LISTEN`, comma separated: the LAN address
first, then the operator's tunnel address for use away from home (ADR 0020), for example
`192.168.1.20:8765,10.8.0.4:8765`. The same app serves on each. JSON bodies are UTF-8.

## Pairing

The desktop app calls the control API (Unix socket, ADR 0013):

| Route | Result |
|---|---|
| `POST /phone/pairing` | `{code: "482913", address: "192.168.1.20:8765", addresses: [...], expires_at}`. `address` is the first listen address, the one the operator types; `addresses` is every one. A new code replaces the last one. 409 if the listener is off. |
| `GET /phone` | `{address: string\|null, addresses: string[], device: {id, name, paired_at, last_seen\|null}\|null}` |
| `DELETE /phone` | `{ok, detail}`. Forget the paired phone. Its signatures stop working at once, and a phone session that is running ends. |

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

Errors are `{error: <code>}`: `unauthorized` (401), `bad_code` (403, `/pair`), `bad_request` (422, a body
or query that doesn't fit), `bad_offer` and `bad_candidate` (400), `not_found` (404, including an unknown
`pc_id`), `busy` (409), `not_ready` (503), `internal` (500).

## Routes

| Route | Body | Result |
|---|---|---|
| `GET /hello` | | `{workstation, addresses, status: Status}`. `Status` is the control API's, including `session_origin: "desk" \| "phone" \| null`. `addresses` is the current list; the phone replaces its saved one when it differs. |
| `GET /audit?limit=N&offset=M` | | the control API's `AuditPage` |
| `POST /offer` | `{sdp, type: "offer", pc_id?, restart_pc?}` | `{sdp, type: "answer", pc_id}`. 409 `{error: "busy"}` while any session runs, desk or phone. 503 `{error: "not_ready"}` until the models load. |
| `PATCH /offer` | `{pc_id, candidates: [{candidate, sdp_mid, sdp_mline_index}]}` | `{ok: true}` (trickle ICE). `candidate` may keep its `candidate:` prefix; an empty one means end of candidates. |
| `POST /end` | | `{ok: true}`. Ends the phone's session if one is running. |

A `POST /offer` without `pc_id` starts a session; with the live session's `pc_id` it renegotiates. A
second `POST /offer` without `pc_id` while the phone's own session is still running gets 409 `busy` too:
a phone that lost track of its session sends `POST /end` first and retries once `/hello` shows `passive`.
The session ends when the phone closes the peer connection, stops sending `ping` for 5 s, sends
`POST /end`, or says goodbye to Ghost. The pipeline ending closes the connection from the desk side.
After a peer drops, the desk can take about 10 s to wind the session down. A peer that hasn't connected
30 s after its offer was accepted is dropped.

## The data channel

The phone creates one data channel labelled `chat` before making the offer. It sends the text `ping`
once a second (Pipecat's keepalive), then RTVI messages `{label: "rtvi-ai", type, id, data}`:

- `client-ready` `{version: "2.1.0", about: {library: "ghost-mobile"}}`, sent once the channel opens.
- `disconnect-bot`, which ends the session (the same as `POST /end`).

The desk ignores every other client message type.

The desk sends `bot-ready`, the speaking events (`user-started-speaking`, `user-stopped-speaking`,
`bot-started-speaking`, `bot-stopped-speaking`), and `server-message` whose `data` is one control API
event, exactly as the WebSocket would send it: `status`, `turn`, `pending`, `outcome` or `conversation`.
Log events are never sent. Server messages start after `client-ready`: first the turn, pending, outcome and
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
(`GHOST_ECHO_WINDOW_SECS`) when it is mostly a run of what Ghost just said (`ghost_voice.echo`). Answers
under three words always pass, so a spoken yes is never mistaken for echo.

End sends `disconnect-bot` on the data channel, then `POST /end`, then closes the peer.
