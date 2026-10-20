# 0020: The phone reaches the desk from anywhere over the operator's own WireGuard

- **Status:** accepted
- **Date:** 2026-10-20
- **Amends:** ADR 0005 (records the exception: a VPN client on the phone), ADR 0016 (the listener binds more than one address; "LAN only" ends)

The point of the phone is to work with Ghost while the phone can't be used: in the car, on a
walk, at the other house. ADR 0016 built the phone path for the home LAN only: the listener is
bound to the LAN address, both ends gather host candidates and nothing else, and the desk's
media ports are random. From cellular, none of that is reachable, and ADR 0005 asked that
reaching the house not open a router port or put a VPN client on the phone, or else record an
explicit exception. This is that exception.

## Decision

**The phone reaches the desk through a WireGuard network the operator already runs**, on a
fleet host that already has the one UDP forward it needs. The desk and the phone each become a
peer of that hub. The hub forwards only between those two peers; the phone sees nothing else on
the network, and nothing on the network sees the phone. On the phone, the tunnel carries only
Ghost's traffic (the WireGuard app's per-app setting) and stays on.

Everything above the tunnel is ADR 0016 unchanged: the same signed HTTP listener, the same
WebRTC offer, no STUN, no TURN, DTLS-SRTP end to end. The listener binds a list of addresses
(`GHOST_PHONE_LISTEN` takes several, comma separated), one on the LAN and one on the tunnel.
`/hello` and the pairing reply carry that list, and the phone keeps all of them and uses the
first that answers, so at home it stays on Wi-Fi and away it goes through the tunnel without
the operator choosing.

**Why this and not a relay.** The other route that respects ADR 0005 is a public rendezvous
(a Cloudflare Tunnel in front of the listener) plus a TURN relay for the media (Cloudflare
Realtime TURN or a hosted coturn). It needs no VPN client and no port, but it puts a third party
in the path of every session, needs credentials minted per session, needs ICE restarts to
survive a Wi-Fi to cellular switch, and is several days of work. WireGuard roams on its own,
so a session survives that switch with no WebRTC change; nothing public is added; and the
path exists today. The VPN client is the price, and it is a known, audited one. Tailscale was
rejected for the same reason the fleet rejected it for T3 ingress (a third-party control
plane). A port forward to the desk with a public listener was rejected: it exposes the daemon
and the fleet rule prefers a tunnel to a forward.

## Considered options

- **Cloudflare Tunnel plus TURN.** The documented alternative. Built only if the VPN client
  proves a problem in daily use (Android allows one VPN at a time). It would add: a `/ice`
  route returning short-lived TURN credentials, ICE servers on both ends, ICE restart on
  network change in the app, and a rule that `/pair` is refused through the tunnel.
- **Tailscale.** Simplest to set up, rejected: a third-party control plane, and still a VPN
  client on the phone.
- **Port forward and a public listener.** Rejected: it exposes `ghostd` to the internet, with
  an unsigned `/pair` route, against the fleet's tunnel-over-forward rule.

## Consequences

- The passive-audio rule is untouched: the phone still has no passive state, and its session
  audio travels inside WireGuard and inside DTLS-SRTP.
- The desk keeps a short keepalive grace for the phone; it grows (5 s to 15 s) so a cellular
  blip or a tunnel re-handshake is not a hang-up, and the phone retries the offer with an ICE
  restart (`restart_pc`, already in the protocol) when the call fails rather than ending.
- Latency over cellular is measured before and after (ADR 0018): `latency.jsonl` from real
  sessions, tunnel round trip, and the call's packet loss and jitter from the phone's own
  WebRTC stats at session end.
- Using Ghost with the phone in a pocket needs a microphone foreground service and sensible
  audio routing (Bluetooth, not forced speaker); that is app work, independent of this
  decision, and planned with it.
- The addresses, keys and host names of the operator's network are not in this repository.
