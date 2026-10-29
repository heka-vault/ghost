# ghost-rtc

The phone's WebRTC audio (ADR 0016, ADR 0020; W8 of heka-vault/ghost#39). One trait,
`Media`, that the daemon's phone session talks to, and three stacks behind Cargo features,
one chosen by measurement (below). It replaces Pipecat's `SmallWebRTCConnection` and
`SmallWebRTCTransport` and the Python daemon's `ghost_voice.rtvi`.

The rules hold here (CLAUDE.md): nothing from the phone's audio is logged or stored by
this crate; a data-channel message outside the RTVI subset is dropped without its contents
reaching a log; no ICE servers, ever (the phone reaches the desk over the LAN or the
operator's own tunnel); the listener that signs the offer and the answer is the daemon's,
not here.

## What is here

| Module | What it does |
| --- | --- |
| `media` | `Media`: `answer(offer, restart)` (the protocol's `restart_pc` is a fresh peer), `take_frames()` (inbound 20 ms frames, 16 kHz mono s16, a `Stream`), `sink()` (outbound 24 kHz, any length, `clear()` for barge-in), `take_messages()` (the client messages that passed the subset), `send()` (the outbound RTVI messages, held until the channel opens, fifty at most as the Python did), `link()` (`Waiting`, `Connected`, `Gone`, a `watch`), `is_connected()` (the Python's ping rule: a `ping` in the last 3 s, else the link), `fingerprint()` (what the latest answer carries, the one the phone pins through the signed answer), `stats()`, `close()`. |
| `sdp` | The offer read into typed fields before any stack sees it (one audio m-line with Opus at 48 kHz, the data channel's m-line, ICE credentials, the sha-256 fingerprint among the ones aiortc sends, `setup`, candidates or trickle) and the shape every stack's answer must have (sha-256 fingerprint, `active` or `passive`, sendrecv audio, rtcp-mux, at least one candidate: the desk never trickles, the phone applies one signed answer). An error names the missing part, never a line of the offer. |
| `codec` | Opus through the system `libopus`: decode at 48 kHz mono, encode at 48 kHz mono VoIP, 20 ms a packet, concealment for a lost packet (five in a row at most). |
| `resample` | `rubato`'s FFT resampler, fixed input size: 48 kHz down to 16 kHz on the way in, 24 kHz up to 48 kHz on the way out, with the queues so a stack pushes what it has and pulls whole frames. The filter's delay is 3 to 4 ms. |
| `rtvi` | The data channel's subset as `ghost_voice.rtvi` narrowed it: `ping`, `client-ready` and `disconnect-bot` in, everything else dropped unlogged; `bot-ready` (Pipecat's shape and version rule), the four speaking events and `server-message` out, never a `log` event. |
| `stats` | The inbound RTP counters: packets, sequence gaps, RFC 3550 interarrival jitter. |
| `backend` | What the stacks share (the channels, the link, the ping rule, the inbound and outbound paths) and the three stacks: `str0m`, `webrtc`, `libwebrtc`. |
| `marker`, `probe` | The measurement: a 1 kHz burst once a second, 200 ms long, and the session that times it, answers `client-ready` and ends on `disconnect-bot`. Not on the daemon's path. |

Each stack is a feature; the daemon enables one. `new_media(config)` builds the first
enabled in the order `str0m`, `webrtc`, `libwebrtc`; `BACKEND` names it.

## The far end and the measurements

`scripts/rtc/phone.py` is the phone, automated with aiortc from the Python venv: one Opus
track playing the marker tone, a `chat` data channel with `ping` once a second and
`client-ready`, a `restart_pc` mid-call on a fresh peer connection, the call's stats, a
`disconnect-bot` at the end, and a JSON report with every burst's send and receive time.
`scripts/rtc/measure.py` builds `rtc-probe` with one stack, runs a call against it and
pairs the two reports.

```bash
# unit tests (27), the SDP shaping, the resamplers, the Opus round trip, the RTVI subset
cargo nextest run -p ghost-rtc
# the loopback tests against aiortc, per stack (GHOST_VENV defaults to the main checkout's .venv)
cargo nextest run -p ghost-rtc --features str0m --run-ignored only
# a 5-minute measured call with a restart_pc at 150 s
/home/detroitpro/projects/active/ghost/.venv/bin/python -I scripts/rtc/measure.py --backend str0m
```

One-way latency is each burst's receive time minus its send time, both wall clock on the
same machine: the phone's send is when the frame holding the burst's first sample was handed
to aiortc; the desk's receive is when the decoded, resampled 16 kHz frame holding it was
ready for the pipeline (what Ghost hears). The other way, the desk's send is when the frame
entered the sink (the pacer takes it within 20 ms) and the phone's receive is when aiortc's
decoder gave the frame out, after aiortc's own jitter buffer, which prefetches four packets
(80 ms): that part is the far end's, not the stack's. Frames are 20 ms, so each number is
quantised to that.

## Measurements (2026-10-28, this workstation, loopback)

Three calls of 300 s, one per stack, from the aiortc phone to `rtc-probe` on this machine, a
`restart_pc` at 150 s, the marker tone both ways; release builds; the two sides on one host
(12 cores, load from the operator's own work). The CPU column is from separate 60 s calls
without a restart, with the harness fixed to count every thread (`measurements/cpu-60s/`);
the rest is the 300 s call. aiortc gathers no loopback candidate, so the pair is the desk's
127.0.0.1 and the phone's LAN address, on this box. Packet counts are the live peer's
(after the restart, 150 s). The raw reports are in `measurements/`.

| stack | uplink p50 / p95 / max ms | downlink p50 / p95 / max ms | lost desk / phone of packets in | jitter desk / phone ms | desk CPU, one core | answer ms | restart_pc | fingerprint |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `str0m` | 21 / 23 / 26 | 93 / 96 / 296 | 0 / 0 of 7491 / 7490 | 0.15 / 0.17 | 1.1 % | [8, 4] | yes | match |
| `webrtc` | 20 / 21 / 22 | 99 / 100 / 315 | 0 / 0 of 7494 / 7493 | 0.47 / 0.48 | 1.0 % | [6, 102] | yes | match |
| `libwebrtc` | 31 / 32 / 52 | 98 / 98 / 98 | 0 / 0 of 7464 / 7494 | 0.00 / 0.04 | 1.0 % | [109, 104] | yes | match |

What the numbers say:

- **Uplink (what Ghost hears), 20 to 21 ms p50 for str0m and webrtc-rs**: one packet's
  worth (the burst's first sample waits for its 20 ms packet to fill before it is sent),
  the loopback hop, and about a millisecond of Opus and the resampler; no jitter buffer on
  the desk, a lost packet concealed rather than waited for. libwebrtc's NetEQ holds an
  adaptive buffer in front: 21 ms p50 in one call, 31 ms in another, 52 ms max.
- **Downlink, 93 to 99 ms p50**: aiortc's receive jitter buffer prefetches four packets
  (80 ms) before it plays, so the stack's own share is 13 ms (str0m), 18 ms (libwebrtc),
  19 ms (webrtc-rs): the pacer's tick (up to 20 ms), the resampler and Opus. The one
  burst over 290 ms in the two Rust stacks is the first after the restart. The real
  phone's own jitter buffer replaces aiortc's here.
- **Loss and jitter**: none and under half a millisecond on loopback, all three; the
  phone's receiver reports agree. libwebrtc's own jitter figure is its NetEQ's.
- **The budget** (`docs/performance.md`: 0.15 s p50 one way, 0.3 s p95) holds on
  loopback for every stack with room for a LAN or the tunnel; the Python daemon's aiortc
  transport paid an 80 ms prefetch on the desk side too, which none of these do.
- **The answer**: str0m and webrtc-rs answer in 4 to 8 ms with one host candidate on the
  address Ghost binds; libwebrtc takes about 105 ms to gather on every interface of this
  box (10 candidates, the docker bridges and the tunnel among them) and offers them all,
  since it does not take a bind address. webrtc-rs's second answer took 102 ms with the
  100 ms pause the restart keeps after closing the old peer.
- **restart_pc**: a fresh peer, a fresh fingerprint pinned through the new answer, audio
  and `bot-ready` again, in every stack. str0m notices the phone closing its old peer and
  reports `Gone` before the restart's `Waiting`; the other two go straight to `Waiting`.
- **CPU**: about 1 % of one core for a call, all three (the probe's own tone and
  detection included).

## Recommendation: str0m

All three answer a real offer and carry a 5-minute call with the handshake, the restart
and the fingerprint check. str0m is the one to ship:

- The lowest latency both ways and the quickest answer, with nothing in front of the
  decoder: a lost packet is concealed, not waited for, and the pipeline's own VAD and
  turn detection take the frames as they come.
- Sans-IO: Ghost owns the socket (bound on the listener's own address, so the answer's
  candidate is on the network the offer came in on, LAN or tunnel, ADR 0020), the timers
  and the thread, on one tokio task per call; nothing runs that Ghost did not start, and
  the whole module is 451 lines (`backend/str0m.rs`).
- Pure Rust over the system OpenSSL already on the box; no prebuilt C++ (libwebrtc's
  download and clang-only link), no second async layer (webrtc-rs's driver and runtime
  abstraction), `cargo deny` clean.
- Opus and the resamplers are the crate's own (`codec`, `resample`), the same code for
  webrtc-rs, so the audio path is tested in Rust unit tests rather than inside a vendored
  stack.

webrtc-rs is a sound second: the same latency, the same audio path, and it binds where
asked; it needed the DTLS-server answer for its data channel (its SCTP as DTLS client
refuses the phone's reply in 0.21) and a pause after closing a peer before the next.
libwebrtc is Google's stack and the phone's own, with NetEQ, but it is a 100 MB prebuilt
with a hermetic libc++ that pins the clang version, it gathers on every interface and
cannot be told where to bind, its NetEQ adds 10 to 30 ms the pipeline does not need, and
its audio device stops playout when the last receiving stream closes (the restart starts
the new peer first to work around it).

What is not measured here: the real phone over the LAN and the WireGuard tunnel, the
Wi-Fi to cellular switch and the phone's own jitter buffer need the operator and the
signed listener in `ghostd` (W8's listener), and come after; `rtc-probe --echo` is the
session for that day. The daemon's keepalive grace (`PHONE_PING_GRACE_SECS`, 15 s) stays
the authority on when a gone peer is gone for good; `Link::Gone` is the stack's view and
a `restart_pc` brings it back.

Known limits: the stacks' own `rtt_ms`, `remote_loss` and `path` are filled only where the
stack surfaces them (str0m's egress stats and its socket pair); Opus in-band FEC is off,
as it was with aiortc, and could be turned on for the tunnel if loss appears; a trickled
candidate (`PATCH /offer`) is accepted but the desk itself never trickles.

## The probe

`rtc-probe` answers one offer over plain HTTP (the protocol's shapes without the
signatures: `POST /offer`, `PATCH /offer`, `POST /end`) and runs the session: the marker
tone out and the bursts timed (`--echo` sends the inbound audio back instead, for a phone
in hand; `--silent`), the stats every 5 s, a JSON report at the end (`--report`). Never binds
the real phone listener's addresses by itself; pass `--listen`. The real phone over the LAN
and the WireGuard tunnel needs the operator and the signed listener in `ghostd`, and comes
later.

```bash
cargo run --release -p ghost-rtc --features str0m --example rtc-probe -- --listen 127.0.0.1:0 --echo
```
