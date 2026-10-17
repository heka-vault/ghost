# 0007: The domain package gates every write; the model is never the authority

- **Status:** accepted
- **Date:** 2026-10-16
- **Carries forward:** original spec ADR-003

LLM tool arguments are an untrusted request. A write happens only when `domain` holds a pending, unexpired proposal bound to this session and target, and the next user turn, taken from the pipeline's own transcript and not from LLM output, is a clear affirmative. Execution claims the proposal atomically, applies bounded steps, reads the result back, and records success, partial, or failed in the audit log. Issue and comment text is data and never changes permissions.

## Consequences

- Because STT runs in our process, "server-observed confirmation" is simple: the transcript frame is the evidence. The original spec's fallback (a separate confirmation recognizer) is not needed.
