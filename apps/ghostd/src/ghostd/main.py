"""Passive -> wake -> conversation -> sleep, forever."""

from __future__ import annotations

import asyncio
import signal
import sys

from ghost_domain import Store, WriteGate
from ghost_skills import load_skills
from ghost_voice import VoiceSettings, build_session
from ghost_voice.pipeline import Models
from ghost_voice.wake import build_wake_listener, parse_phrases
from loguru import logger

from ghostd.audio import Ducker, chime, restore_from_state
from ghostd.config import GhostdSettings, runtime_dir
from ghostd.dictation import is_dictating
from ghostd.wake import FileWakeDetector, TranscriptWakeDetector, WakeDetector

WAKE_FILE = "wake"
DUCK_STATE = "duck.json"


class Daemon:
    def __init__(self, cfg: GhostdSettings, voice: VoiceSettings) -> None:
        self.cfg = cfg
        self.voice = voice
        self.rt = runtime_dir()
        self._stop = asyncio.Event()

    def build_wake(self, models: Models) -> WakeDetector:
        manual = FileWakeDetector(self.rt / WAKE_FILE)
        if self.cfg.wake_mode == "transcript":
            phrases = parse_phrases(self.cfg.wake_phrases)
            logger.info(f"wake phrases: {', '.join(repr(p) for p in phrases)}")
            listener = build_wake_listener(
                self.voice,
                stt_factory=lambda: models.stt.fork(log_transcripts=False),
                phrases=phrases,
            )
            return TranscriptWakeDetector(listener, manual)
        if self.cfg.wake_mode == "file":
            return manual
        if self.cfg.wake_mode == "onnx":
            raise NotImplementedError(
                "GHOST_WAKE_MODE=onnx needs the trained model adapter from the wake-word spike"
            )
        raise ValueError(f"unknown GHOST_WAKE_MODE {self.cfg.wake_mode!r}")

    async def run(self) -> None:
        loop = asyncio.get_event_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            loop.add_signal_handler(sig, self._stop.set)

        self.cfg.db_path.parent.mkdir(parents=True, exist_ok=True)
        store = Store(self.cfg.db_path)
        skills = load_skills(
            self.cfg.skills,
            {
                "github_issues": {
                    "token": self.cfg.github_token,
                    "repos": self.cfg.repos,
                    "mock": self.cfg.github_mock,
                }
            },
        )
        if self.cfg.github_mock:
            logger.warning("GITHUB MOCK: issues are in-memory; nothing reaches GitHub")
        # The gate writes through the issue skill's writer and its allowlist.
        issues = next(s for s in skills if s.name == "github_issues")
        gate = WriteGate(store, issues.writer, issues.policy)  # type: ignore[attr-defined]
        logger.info(f"allowed repos: {sorted(issues.policy.allowed)}")  # type: ignore[attr-defined]

        logger.info("loading models")
        models = await asyncio.to_thread(Models.load, self.voice)
        logger.info("models ready")

        ducker = Ducker(self.cfg.duck_apps, self.cfg.duck_level, self.rt / DUCK_STATE)
        restore_from_state(self.rt / DUCK_STATE)  # undo a previous crash
        wake = self.build_wake(models)

        try:
            while not self._stop.is_set():
                logger.info("passive: waiting for wake word")
                wake_task = asyncio.ensure_future(wake.wait())
                stop_task = asyncio.ensure_future(self._stop.wait())
                await asyncio.wait({wake_task, stop_task}, return_when=asyncio.FIRST_COMPLETED)
                stop_task.cancel()
                if self._stop.is_set():
                    wake_task.cancel()
                    break
                if is_dictating(self.cfg.dictation_flag):
                    logger.info("wake ignored: dictation is active")
                    continue
                await self._conversation(models, skills, gate, ducker, wake_task.result())
        finally:
            wake.close()
            ducker.restore()
            for s in skills:
                await s.aclose()
            store.close()

    async def _conversation(
        self,
        models: Models,
        skills,
        gate: WriteGate,
        ducker: Ducker,
        first_utterance: str | None = None,
    ) -> None:
        ducker.duck()
        if self.cfg.chime:
            await asyncio.to_thread(chime, True, self.voice.output_device)
        session = build_session(
            self.voice, models=models, skills=skills, gate=gate, first_utterance=first_utterance
        )
        logger.info(f"session {session.session_id} start")
        stop_task = asyncio.ensure_future(self._stop.wait())
        try:
            await session.start()  # type: ignore[attr-defined]
            run_task = asyncio.ensure_future(session.run())
            done, _ = await asyncio.wait({run_task, stop_task}, return_when=asyncio.FIRST_COMPLETED)
            if run_task not in done:
                await session.cancel()
                await run_task
        except Exception:
            logger.exception("session failed")
        finally:
            stop_task.cancel()
            if self.cfg.chime:
                await asyncio.to_thread(chime, False, self.voice.output_device)
            ducker.restore()
            logger.info(f"session {session.session_id} end")


def cli(argv: list[str] | None = None) -> None:
    argv = sys.argv[1:] if argv is None else argv
    cmd = argv[0] if argv else "run"
    logger.remove()
    logger.add(sys.stderr, level="INFO", format="{time:HH:mm:ss.SSS} {level:<7} {message}")

    if cmd == "run":
        cfg = GhostdSettings()
        voice = VoiceSettings()  # type: ignore[call-arg]
        asyncio.run(Daemon(cfg, voice).run())
    elif cmd == "wake":
        (runtime_dir() / WAKE_FILE).touch()
        print("wake requested")
    elif cmd == "restore-volumes":
        restore_from_state(runtime_dir() / DUCK_STATE)
    elif cmd == "devices":
        import sounddevice as sd

        print(sd.query_devices())
    elif cmd == "check":
        cfg = GhostdSettings()
        voice = VoiceSettings()  # type: ignore[call-arg]
        problems = []
        if cfg.github_mock:
            print("GITHUB MOCK is on")
        else:
            if not cfg.github_token:
                problems.append("GITHUB_TOKEN is empty (or set GHOST_GITHUB_MOCK=1)")
            if not cfg.repos:
                problems.append("GHOST_REPOS allowlist is empty")
        if not voice.anthropic_base_url:
            problems.append("ANTHROPIC_BASE_URL is not set")
        print("\n".join(problems) if problems else "config ok")
        sys.exit(1 if problems else 0)
    else:
        print(f"usage: ghostd [run|wake|restore-volumes|devices|check]  (got {cmd!r})")
        sys.exit(2)


if __name__ == "__main__":
    cli()
