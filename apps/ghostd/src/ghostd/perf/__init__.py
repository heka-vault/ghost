"""Ghost's latency harness: scripted conversations through the real pipeline, headless.

Nothing here opens the microphone or the speakers. User turns are Kokoro-rendered
utterances fed in real time through a scripted transport; Ghost's speech is consumed
by a transport that plays to nowhere at real-time pace. STT, Smart Turn, the LLM, the
gate and TTS are the real ones. GitHub is the mock, in a temporary data directory,
so the operator's state is never touched. See docs/benchmarks/latency-harness.md.
"""
