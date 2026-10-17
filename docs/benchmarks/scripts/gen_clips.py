"""Synthesize VoiceTriage-style test commands with Piper (ryan-high) as 16 kHz mono WAVs.

Usage: python -I gen_clips.py <models_dir> <audio_dir>
"""
import json
import subprocess
import sys
import wave
from pathlib import Path

from piper import PiperVoice

COMMANDS = [
    "What needs my sign-off before anyone can begin development?",
    "Approve option B for heka vault issue one forty two and mark it ready for development.",
    "Compare the Redis and Postgres options for that issue.",
    "Request changes on Acme issue forty six, the acceptance criteria are missing.",
    "Confirm.",
    "Next issue.",
    "Go to sleep.",
    "Remind me of the tradeoff between LiveKit and Pipecat.",
]

models, audio = Path(sys.argv[1]), Path(sys.argv[2])
voice = PiperVoice.load(str(models / "en_US-ryan-high.onnx"))
refs = {}
for i, text in enumerate(COMMANDS):
    raw = audio / f"cmd{i}_raw.wav"
    with wave.open(str(raw), "wb") as wf:
        voice.synthesize_wav(text, wf)
    out = audio / f"cmd{i}.wav"
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(raw), "-ar", "16000", "-ac", "1", str(out)], check=True)
    raw.unlink()
    refs[out.name] = text
refs["jfk.wav"] = "And so my fellow Americans, ask not what your country can do for you, ask what you can do for your country."
(audio / "refs.json").write_text(json.dumps(refs, indent=1))
print(f"wrote {len(refs)} clips")
