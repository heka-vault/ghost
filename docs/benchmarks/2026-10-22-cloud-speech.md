# Cloud speech review: ElevenLabs, and what would make Ghost feel real, 2026-10-22

**Question:** keep Parakeet for the wake word but move session speech to ElevenLabs over its websockets. What does that win, what does it cost, and is a different architecture closer to an Alexa-like feel?

**Method:**
- **Timings:** every turn in `~/.local/share/ghost/latency.jsonl`, Oct 19–21. That is 214 turns, 94 of them voice turns with a model reply: 85 from the phone and 9 from the desk. The log holds timings only, no text.
- **Code:** Pipecat 1.12's source, as installed.
- **Vendor docs:** read on 2026-10-22, linked below. Where a fact is unverified, it says so.
- **Network:** the desk reaches ElevenLabs (`api.elevenlabs.io`, `api.us.elevenlabs.io`) in a 42 ms TCP round trip and 65–70 ms to TLS.
- **Not run:** there is no ElevenLabs key on this machine. No ElevenLabs voice, latency or accuracy figure here comes from this desk.

## Where a turn's time goes today

Medians for a voice reply with no tool call (51 turns). The median reply is 3.64 s; the stage medians below sum to 3.37 s.

| Stage | p50 | p90 | What it is |
|---|---|---|---|
| hear | 0.36 s | 0.43 s | 0.2 s of silence before VAD calls the end of speech, then Parakeet's transcript (0.16 s) |
| hold | 0.80 s | 0.80 s | `GHOST_TURN_GRACE_SECS`: after Smart Turn says "complete", wait in case you go on |
| model | 1.67 s | 2.87 s | Sonnet 5.5; first token 1.27 s. With `between_tools` the text arrives in one burst |
| voice | 0.54 s | 1.11 s | Kokoro renders a whole clause before any audio, about 0.23 s per second of speech |

Tool turns (43 of them) have a median of 4.94 s and a p90 of 11.5 s. The extra time is a second model call plus the tool itself.

**The model is half of every turn, and no speech vendor changes it.** Speech work (hear plus voice) is 0.9 s, and the hold is another 0.8 s.

## What makes a voice assistant feel real

1. **The gap after you stop.** Alexa answers simple requests in about 1–1.5 s, and people in conversation leave 0.2–0.5 s. Ghost leaves 3.6 s.
2. **Interrupting it.** Ghost stops talking only after Parakeet has a transcript of at least two words (`BargeInUserTurnStartStrategy`). Parakeet transcribes only after you pause (`SegmentedSTTService`).
   - So Ghost talks over a whole interruption: "no wait, the other one" keeps it going for that phrase plus about 0.4 s.
   - With partial transcripts, it would stop about two words in.
3. **The voice.** Prosody, emphasis, and not sounding read aloud.
4. **Wake at the name.** Today the wake fires at the end of the whole utterance. A trained detector would fire at the end of "ghost" ([wake review](2026-10-21-wake.md)).
5. **Feedback while it works.** The thinking sound does this today. The array's LED ring could too.

## ElevenLabs, piece by piece

### Speech to text: Scribe v2 Realtime over the websocket

- **Supported in Ghost's Pipecat:** `ElevenLabsRealtimeSTTService`.
  - It commits a turn when Pipecat's VAD says stop (`CommitStrategy.MANUAL`).
  - It turns partials into interim frames, and supports keyterms and word timestamps.
  - It sends the key as an `xi-api-key` header, not in the URL (checked in the source).
- **Not faster to the final transcript.** Pipecat measures ElevenLabs realtime at 0.41 s p99 from the end of speech, VAD silence included (`pipecat/services/stt_latency.py`). Parakeet here is 0.36 s p50 and 0.43 s p90 on the same measure.
- **What it adds:**
  - **Partial transcripts while you speak:** about 150 ms, published ([models](https://elevenlabs.io/docs/overview/models)). Ghost's barge-in rule already counts interim words, so mid-sentence interruption would work as is.
  - **Keyterms:** 50 terms for +$0.08/h. Repo and org names are a known Parakeet weakness (ADR 0004).
- **Cost:** $0.39/h of open session, and silence is billed ([pricing](https://elevenlabs.io/pricing/api)).
- **Risks:**
  - **Phantom turns:** Pipecat issue [#5197](https://github.com/pipecat-ai/pipecat/issues/5197) is open: re-sent partials open phantom user turns. With Ghost's barge-in rule, that means phantom interruptions.
  - **Reconnects:** Pipecat 1.13 (2026-10-19) fixes realtime STT going silent after a reconnect.
  - **Commit modes:** `turn_prediction` commits arrived in the [2026-10-12 changelog](https://elevenlabs.io/docs/changelog/2026/10/12), but it's unverified whether Scribe v2 Realtime supports them, and Pipecat doesn't expose them.

### Text to speech: Flash v2.5 over the multi-context websocket

- **Supported in Ghost's Pipecat:** `ElevenLabsTTSService`, on `multi-stream-input` with Flash v2.5 by default.
  - It closes the context on an interruption.
  - It uses word timestamps, so after a barge-in only the words actually spoken go into the context.
  - The key is sent as a header.
- **Faster, and the time stops growing with sentence length.** Published model latency is about 75 ms; with this desk's 40 ms round trip, first audio should be roughly 0.2–0.3 s.
  - Kokoro's is 0.54 s p50 and 1.1 s p90, and grows with the first clause: 7 of 94 turns took over 1 s.
  - This frees CPU on the desk too.
- **The best voices aren't on the websockets.** Eleven v3 and v4 aren't supported there ([websockets](https://elevenlabs.io/docs/websockets)).
  - They stream only over the Text to Dialogue socket, which waits for about 8 words before the first audio.
  - Low latency means Flash v2.5 or v4 Turbo (~100 ms).
- **Voice quality is the one place a cloud vendor clearly beats what runs here.** That can't be measured from a doc. It needs a listen.
- **Cost:** $0.04 per 1K characters for Flash ([pricing](https://elevenlabs.io/pricing/api)).
  - The busiest day in the log, Oct 20, had 501 s of Ghost speech: about 7.5K characters, or $0.30.
  - At that rate it's under $10 a month.
- **What leaves the machine:** Ghost's reply text only, which the LLM provider already has. No microphone audio.

### ElevenLabs Agents and Speech Engine

These are the managed versions. ElevenLabs runs speech recognition, its own turn-taking model (`turn_v3`, with tunable eagerness), interruptions and TTS, for $0.08/min plus LLM tokens. Speech Engine leaves the "brain" with you ([Pipecat guide](https://elevenlabs.io/docs/eleven-api/guides/how-to/speech-engine/pipecat-integration)).

Neither fits:
- **A public URL.** Both have ElevenLabs' cloud call your brain or custom LLM at a public URL; the docs use ngrok ([custom LLM](https://elevenlabs.io/docs/eleven-agents/customization/llm/custom-llm)). ADR 0020 says no public listener.
- **Ghost's own processors are bypassed:** the interceptors, the hold gate, the confirmation classifier, narration cut, the thinking sound and line transfers.
- **Retention:** Agents keeps transcripts and audio for 2 years by default ([retention](https://elevenlabs.io/docs/eleven-agents/customization/privacy/retention)).
- **The turn model** isn't offered on its own.

### Speech-to-speech models (OpenAI realtime, Gemini Live)

These are the fastest feel available, under a second. But:
- **They can't run Claude.**
- **Their user transcript comes from a separate model.** OpenAI says it is not "precisely what the model heard" ([client events](https://developers.openai.com/api/reference/resources/realtime/client-events)).
- **That breaks the gate's spoken yes,** which is judged on the transcript (ADR 0017). The model that decides what to write would no longer be the model the operator chose.

Out.

## Trade-offs

| | Win | Cost |
|---|---|---|
| **ElevenLabs TTS** | Voice quality. First audio ~0.3 s faster at p50 and ~0.8 s at p90. Less CPU. After a barge-in, the context holds only the words spoken | Reply text goes to a second vendor. Network dependency. Under $10/mo |
| **ElevenLabs STT** | Mid-sentence barge-in. Keyterms for repo names | All session audio leaves the machine. No faster to the final transcript. Phantom-turn bug open. ~$0.39/h |
| **Both** | Both columns above | Both columns above, and an outage silences Ghost unless it falls back to Parakeet and Kokoro |
| **Agents / Speech Engine** | A tuned turn model | Public URL, bypasses the pipeline, 2-year retention. No |
| **Speech-to-speech** | Fastest | No Claude, and the transcript isn't what the model heard. No |

## Rules a cloud speech service touches

- **Architecture:** "only two things leave the machine: LLM calls, and GitHub API calls" ([architecture](../architecture.md)). Either cloud piece needs an ADR that amends ADR 0004, which already anticipated a cloud service "behind the same config".
- **Idle audio stays local.** That still holds: wake stays on Parakeet.
- **Hold (ADR 0021): a cloud STT must not hear a hold.** On hold, the room talks for up to 30 minutes, and Ghost listens only for its name. That listening has to go back to local Parakeet, or the room's conversation streams to ElevenLabs.
- **The spoken yes (ADR 0017)** would be judged on a vendor's transcript. That's still a frame the pipeline produced, so the rule holds. But Parakeet was picked because it got one-word commands like "Confirm." right, and Scribe is unmeasured on those.
- **Data handling** ([ZRM](https://elevenlabs.io/docs/eleven-api/resources/zero-retention-mode), [training](https://elevenlabs.io/docs/help-center/legal/is-my-data-used-to-improve-eleven-labs-ai-models)):
  - ElevenLabs trains on your data by default. The opt-out (Settings → Data use) covers only data sent afterwards, and whether it covers API traffic isn't stated.
  - Zero Retention Mode (`enable_logging=false`) and data residency are Enterprise only.
  - Deleting history still leaves debugging and moderation logs, and backups are kept up to 30 days.
- **Secrets:** an `ELEVENLABS_API_KEY` from the secrets store, sent as a header, never logged. Pipecat already does this.

## What to do, in order of feel per unit of risk

Tracked in [#3](https://github.com/heka-vault/ghost/issues/3) (children #4–#9), with [ADR 0025](../adr/0025-a-trained-wake-detector-with-parakeet-as-verifier.md) and [ADR 0026](../adr/0026-ghosts-voice-may-come-from-a-cloud-tts.md), both proposed.

1. **Start the model during the turn hold (local, no vendor).**
   - Pipecat 1.12 has speculative replies: `EagerUserTurnStopStrategy` and a `SpeculationGate` inside the LLM service.
   - The gate holds the response until the turn is confirmed and drops it if you go on.
   - **A speculative inference never runs a tool.** It withdraws itself and the confirmed turn runs the call (`llm_service.py`), so the write gate is never involved.
   - Pipecat wires this to STT services that predict the end of a turn (Deepgram Flux, Cartesia). Ghost would emit the same frames from Smart Turn's verdict and confirm them when the 0.8 s hold ends. That needs a prototype.
   - **Gain:** up to 0.8 s on every reply with no tool call, at the cost of some discarded model calls.
2. **Partial transcripts for barge-in, local first.**
   - While Ghost speaks, re-run Parakeet on the open segment every ~0.3 s and emit interim frames. Parakeet takes 0.10–0.14 s for a short phrase on 4 threads.
   - The existing two-word rule then stops Ghost about two words into an interruption, with no audio leaving.
   - Measure self-interruption on the array before turning it on. A streaming model (Nemotron streaming 0.6B in sherpa-onnx; Moonshine streaming) is the next step if this loop is too heavy.
   - Cloud STT only if both fall short. If so, Deepgram Flux fits Pipecat's speculative replies better than ElevenLabs does: its eager end of turn plugs straight in, at the same $0.39/h ([Flux](https://developers.deepgram.com/docs/flux/quickstart)).
3. **ElevenLabs Flash v2.5 for Ghost's voice, behind the config, with Kokoro as the fallback.**
   - This is the cloud piece worth having: the biggest change in how Ghost sounds, a faster and steadier first audio, and no microphone audio leaves.
   - It needs a key to A/B voices first, then an ADR.
4. **The model.** At 1.67 s it is the largest stage. Haiku 5.5 on OpenRouter reached its first token in about 0.6 s in the Oct 19 benchmark, but through the proxy it answered 3 of 10 write requests with a question instead of the write. That's a separate decision, recorded in [latency](2026-10-19-latency.md).
5. **Wake at the name:** the trained detector from the [wake review](2026-10-21-wake.md).

With 1–3, a reply with no tool call goes from about 3.4 s to about 2.3 s. With a faster model as well, about 1.5 s. Those are estimates.

## Not measured

- **ElevenLabs end to end from this desk:** voices, first audio through Pipecat, and Scribe on one-word answers. These need a key; the free tier would cover a test.
- **The speculative reply:** how often Smart Turn's "complete" is withdrawn, and the discarded model calls that costs.
- **Barge-in:** how long it takes today from the start of an interruption to silence. The latency log doesn't record it.
