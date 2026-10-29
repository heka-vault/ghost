"""The oracle (ADR 0039): the Python the Rust daemon is measured against.

The Python daemon is gone (W10 of heka-vault/ghost#39). These modules are the wrappers it
ran over the same model files, kept word for word so the parity fixtures
(``scripts/fixtures/record.py``) and the wake tooling keep producing what the Rust crates
(``crates/ghost-infer``, ``crates/ghost-voice``) are compared with:

- ``oracle.parakeet``: Parakeet TDT 0.6B v3 through onnx-asr, as ``ghost_voice.parakeet``
  loaded and ran it.
- ``oracle.wakeword``: the trained wake detectors through livekit-wakeword, as
  ``ghost_voice.wakeword`` scored them.
- ``oracle.wake``: the transcript matcher, as ``ghost_voice.wake`` matched a phrase.

Nothing here reads a microphone or keeps a transcript.
"""
