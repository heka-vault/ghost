"""Synthetic voices for the retraining, made on this machine with Kokoro (the voice Ghost
already ships), in voices the replay rig's programs do not use, so the Kokoro rows of the
report stay unseen by the model.

Positives are "hey ghost" said several ways; negatives are the named confusables of the
first training (ADR 0025, issue #13) and ordinary sentences. Each clip has silence either
side. Clips are cached as 16 kHz WAV in the work folder and made once.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

from wake_train.data import RATE, read_wav, write_wav

# The replay rig's voices (`.scratch/wake-review/corpus`): never used here.
RIG_VOICES = frozenset(
    "af_bella af_heart af_nicole af_river af_sarah af_sky am_adam am_eric am_liam am_michael "
    "am_onyx am_puck bf_emma bf_isabella bm_george bm_lewis".split()
)
VOICES = (
    "af_alloy af_aoede af_jessica af_kore af_nova am_echo am_fenrir am_santa "
    "bf_alice bf_lily bm_daniel bm_fable"
).split()

POSITIVE = (
    "Hey Ghost.",
    "Hey, Ghost.",
    "Hey Ghost!",
    "Hey Ghost?",
    "Hey Ghost, what needs me?",
    "Hey Ghost, are you there?",
    "Hey Ghost, what's on my list today?",
    "Hey Ghost, let's work on the infra repo.",
)
CONFUSABLE = (
    "Hey guys.",
    "He goes.",
    "Hey Gus, how have you been?",
    "Hey coach, what time is practice?",
    "Hey, goat cheese or cheddar?",
    "He goes to the gym every morning.",
    "He set off early.",
    "Okay, coast is clear.",
    "Hey host, where do we sit?",
    "Hey, go straight ahead.",
    "Hey toast is ready.",
    "Say most of it.",
    "A ghost story.",
    "I saw a ghost last night.",
    "The ghost of Christmas past.",
    "Hang on, Ghost.",
    "Later, ghost.",
    "Hey boss, got a minute?",
    "Hey, Joe's here.",
    "Ate a ghost pepper.",
)
SENTENCES = (
    "Can you pass me the remote?",
    "I think the meeting moved to three.",
    "Let's order pizza tonight.",
    "The build failed again on the arm runner.",
    "What time does the store close?",
    "She said the package arrived this morning.",
    "Turn the music down a little.",
    "We should review the pull request after lunch.",
    "It's going to rain tomorrow.",
    "Did you feed the cat?",
    "Hold on, I'm on a call.",
    "That was a great movie.",
)


def _kokoro(cache: Path):
    from kokoro_onnx import Kokoro

    d = cache / "kokoro"
    return Kokoro(str(d / "kokoro-v1.0.onnx"), str(d / "voices-v1.0.bin"))


def make(work: Path, cache: Path, speeds: tuple[float, ...] = (1.0,), jobs: int = 8) -> dict:
    """``{"pos": [paths], "neg": [paths]}``, making what is missing."""
    out = work / "synth"
    out.mkdir(parents=True, exist_ok=True)
    plan = []
    for v in VOICES:
        assert v not in RIG_VOICES
        for s in speeds:
            for i, t in enumerate(POSITIVE):
                plan.append(("pos", f"pos-{i:02d}-{v}-{s}", t, v, s))
            for i, t in enumerate(CONFUSABLE):
                plan.append(("neg", f"conf-{i:02d}-{v}-{s}", t, v, s))
            if s == 1.0:
                for i, t in enumerate(SENTENCES):
                    plan.append(("neg", f"sent-{i:02d}-{v}", t, v, s))
    files: dict[str, list[Path]] = {"pos": [], "neg": []}
    todo = []
    for kind, name, text, voice, sp in plan:
        f = out / f"{name}.wav"
        if not f.exists():
            todo.append((str(f), text, voice, sp, str(cache)))
        files[kind].append(f)
    if todo:
        from concurrent.futures import ProcessPoolExecutor

        with ProcessPoolExecutor(jobs) as ex:
            list(ex.map(_say, todo, chunksize=4))
    return files


_K = None


def _say(job: tuple[str, str, str, float, str]) -> None:
    """One clip: Kokoro at 24 kHz, to 16 kHz, 0.6 s of silence either side."""
    global _K
    import soxr

    path, text, voice, sp, cache = job
    if _K is None:
        _K = _kokoro(Path(cache))
    audio, sr = _K.create(text, voice=voice, speed=sp, lang="en-us")
    pcm = soxr.resample(np.asarray(audio, np.float32), sr, RATE)
    pad = np.zeros(int(0.6 * RATE), np.float32)
    pcm = np.concatenate([pad, pcm, pad])
    pcm = np.clip(pcm * 32767 * 0.7, -32768, 32767).astype(np.int16)
    f = Path(path)
    tmp = f.with_suffix(".tmp")
    write_wav(tmp, pcm.tolist())
    tmp.replace(f)


def load(path: Path) -> np.ndarray:
    return np.asarray(read_wav(path), np.int16)
