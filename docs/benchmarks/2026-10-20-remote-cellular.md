# 2026-10-20: first phone session from cellular over the operator's WireGuard (ADR 0020)

One live session, phone on cellular (Wi-Fi off), reaching the desk through the operator's
WireGuard hub; the desk on OpenRouter Sonnet 5.5 and real GitHub. Timings from
`latency.jsonl` (`make latency`); the media path from the desk log.

| Turn | Kind | Reply | Breakdown |
|---|---|---|---|
| 1 | opening | 0.99 s | tts 0.99 |
| 2 | voice, one tool | 9.53 s | hear 1.17 (hold 0.80) + llm 4.04 (2 calls, ttfb 2.10, cache 44%) + tools 3.85 (`list_issues`, real GitHub) + tts 0.44 |
| 3 | voice | 3.29 s | hear 1.15 (hold 0.80) + llm 1.53 (ttfb 1.27, cache 98%) + tts 0.60 |
| 4 | voice, goodbye | 3.29 s | hear 1.20 (hold 0.80) + llm 1.56 (ttfb 1.37, cache 99%) + tts 0.53 |

- The media path was host (desk, tunnel address) to peer-reflexive (phone, tunnel address):
  no STUN, no TURN, nothing public. About 350 KB each way through the hub for a 50 s call.
- Turns 3 and 4 match the LAN baseline (about 3 s live, of which 0.8 s is the grace hold):
  the tunnel and cellular add nothing a person notices. Turn 2's cost is GitHub (3.85 s for
  one `list_issues`) and a second model call, not the network; that is the next thing to
  measure with `make perf` against the real API.
- The phone's own WebRTC stats for the call (shown in its Settings afterwards): 49 s, 0% loss,
  jitter 3 ms, round trip 29 ms. The desk→phone ICMP probe before the call read about 280 ms,
  which was the phone's Wi-Fi radio dozing, not the path.
- The goodbye ended the call from the phone side as designed; the desk's 2 s write timeout
  fired once on the drained peer (expected since the wind-down fix) and the desk was passive
  again 1.9 s after the phone went.

## Later the same day: two more cellular sessions with the foreground service

Seven turns over three minutes, then three turns; both over the tunnel, both with the
microphone foreground service and its notification up and gone again at the end. Plain turns
were 2.8 to 4.6 s. Four turns across the two sessions carried `turn 2.8-2.9` in `hear`: the
end-of-turn model judged the utterance incomplete and the stop strategy waited out its fallback,
so those replies took 5.6 to 7.9 s. That fallback is the next latency item, independent of the
network (`docs/performance.md` follow-ups).
