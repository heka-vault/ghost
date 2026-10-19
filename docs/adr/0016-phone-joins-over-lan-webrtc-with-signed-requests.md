# 0016: The phone joins over LAN WebRTC, through a small signed listener

- **Status:** accepted
- **Date:** 2026-10-19
- **Amends:** ADR 0005 (how mobile reaches the desk, LAN only for now), ADR 0013 (a second, narrow listener that is not the control API)

The Android app (`apps/mobile`) starts a voice session on the workstation by joining the
pipeline as a Pipecat `SmallWebRTCTransport`. The gate, the policy and Jev apply
unchanged. The phone confirms by voice, and its only touch actions are Start, End and Mute.

`ghostd` serves a second HTTP listener for the phone, bound to one configured LAN address
(`GHOST_PHONE_LISTEN`, off when empty). It is not the control API and doesn't proxy it.
It has five routes: pair, hello (status), a page of the audit log, the WebRTC offer, and end.
The wire format is in [docs/phone-protocol.md](../phone-protocol.md).

- **Pairing.** The desktop app asks the control API for a six-digit code, which is good for two
  minutes and five tries. The phone sends that code to the listener and gets back a device id and
  a 32-byte secret. Only one phone can be paired: pairing again replaces it, and the desktop app
  can forget it.
- **Signed, not TLS.** The listener speaks plain HTTP. Every request after pairing is signed with
  HMAC-SHA256 under the device secret, with a timestamp and a nonce. Every response is signed back,
  bound to the request's nonce. The phone checks the signature on the WebRTC answer before using it,
  which pins the desk's DTLS fingerprint, so the audio is DTLS-SRTP end to end and the secret
  never crosses the network after pairing. The pairing exchange itself is the exposed moment:
  someone active on the home LAN during those two minutes could take the secret. A QR code
  carrying a key can close that later.
- **No STUN or TURN.** Both ends pass an empty ICE server list. Nothing leaves the LAN.
- **One session at a time.** Phone Start is refused while a desk session runs ("Ghost is busy
  at your desk"). While a phone session runs, the passive loop is not listening, so the desk's
  wake phrase does nothing. `Status.session_origin` says which one is live.
- **The desk stays quiet in a phone session.** No music ducking and no chime on the desk
  speaker. The thinking sound is mixed into the phone's audio track instead.
- **The data channel is narrower than RTVI.** Pipecat's default RTVI processor accepts typed
  text into the LLM, raw audio, function results and more. Ghost replaces it with one that
  allows only `client-ready` and `disconnect-bot` from the phone. The desk's session events
  (status, turns, pending, outcomes, conversation) reach the phone as RTVI `server-message`s
  carrying the same event JSON that the control API's WebSocket sends.

## Consequences

- The phone has no passive state. Its mic is open only between Start and End, inside a
  session, so the idle-audio rule holds.
- Reaching the desk from cellular is not solved here. With no open ports and no VPN, the remaining
  routes need a public relay, which ADR 0005 requires to be an explicit, separate decision.
- Android blocks cleartext HTTP by default. The app allows it because the paired address is
  only known at runtime, and it relies on the signatures, not the transport, for integrity.
- If `ufw` is on, the operator allows the listener's TCP port and UDP from the LAN subnet.
  aiortc picks its UDP ports at random.
