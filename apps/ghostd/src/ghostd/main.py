"""Passive -> wake -> conversation -> sleep, forever."""

from __future__ import annotations

import asyncio
import os
import signal
import sys
from collections.abc import Sequence
from datetime import timedelta

from ghost_core import (
    AbilityError,
    AutonomyPolicy,
    ConfirmationClassifier,
    Events,
    GhostContext,
    LoadedAbility,
    RegexClassifier,
    Store,
    WriteGate,
    discover,
    load,
)
from ghost_core.jev import JevClassifier, JevClient
from ghost_voice import VoiceSettings, build_session, debug
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
                stt_factory=models.stt.fork,
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
        autonomy = AutonomyPolicy.load(self.cfg.policy_path)
        abilities = await load_abilities(self.cfg, self.voice, store, autonomy)
        jev = build_jev(self.cfg)
        classifier: ConfirmationClassifier = (
            JevClassifier(
                jev,
                fallback=RegexClassifier(),
                threshold=self.cfg.jev_threshold,
                deadline=self.cfg.jev_deadline_secs,
                on_decision=debug.audio,
            )
            if jev
            else RegexClassifier()
        )
        gate = WriteGate(
            store,
            autonomy=autonomy,
            classifier=classifier,
            undo_window=timedelta(seconds=self.cfg.undo_window_secs),
            on_decision=debug.audio,
        )
        for a in abilities:
            gate.register(a.name, a.contributions.operations)
        events = Events(ln for a in abilities for ln in a.contributions.listeners)
        logger.info(f"autonomy policy: {autonomy.source}")
        for line in describe_abilities(abilities, autonomy):
            logger.info(line)
        logger.info(
            "confirmation classifier: "
            + (f"jev via {jev.provider}, regex fallback" if jev else "regex")
        )

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
                    await asyncio.gather(wake_task, return_exceptions=True)  # releases the mic
                    break
                if is_dictating(self.cfg.dictation_flag):
                    logger.info("wake ignored: dictation is active")
                    continue
                await events.emit("wake")
                await self._conversation(
                    models, abilities, gate, events, ducker, wake_task.result()
                )
        finally:
            wake.close()
            ducker.restore()
            for a in abilities:
                await a.instance.aclose()
            if jev:
                await jev.aclose()
            store.close()

    async def _conversation(
        self,
        models: Models,
        abilities: Sequence[LoadedAbility],
        gate: WriteGate,
        events: Events,
        ducker: Ducker,
        first_utterance: str | None = None,
    ) -> None:
        ducker.duck()
        if self.cfg.chime:
            await asyncio.to_thread(chime, True, self.voice.output_device)
        session = build_session(
            self.voice,
            models=models,
            abilities=abilities,
            gate=gate,
            events=events,
            first_utterance=first_utterance,
        )
        logger.info(f"session {session.session_id} start")
        await events.emit("session_start", session_id=session.session_id)
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
            await events.emit("session_end", session_id=session.session_id)


def enabled_names(cfg: GhostdSettings, autonomy: AutonomyPolicy) -> list[str]:
    return list(autonomy.enabled) if autonomy.enabled is not None else list(cfg.abilities)


async def load_abilities(
    cfg: GhostdSettings, voice: VoiceSettings, store: Store, autonomy: AutonomyPolicy
) -> list[LoadedAbility]:
    discovered = discover(cfg.abilities_dir)

    def make_context(name: str) -> GhostContext:
        return GhostContext.build(
            name,
            data_root=cfg.data_dir,
            cache_root=voice.cache_dir,
            store=store,
            env=os.environ,
            debug_audio=debug.audio,
        )

    return await load(
        enabled_names(cfg, autonomy),
        discovered=discovered,
        make_context=make_context,
        passive_allowed=autonomy.passive,
    )


def describe_abilities(abilities: Sequence[LoadedAbility], autonomy: AutonomyPolicy) -> list[str]:
    lines = []
    for a in abilities:
        levels = autonomy.describe(a.name, {op.name: op.risk for op in a.contributions.operations})
        table = ", ".join(f"{op}={lv.value}" for op, lv in levels.items()) or "read-only"
        extras = []
        if a.contributions.interceptors:
            extras.append(f"{len(a.contributions.interceptors)} interceptor(s)")
        if a.contributions.processors:
            extras.append(f"processors in {', '.join(a.contributions.processors)}")
        if a.contributions.listeners:
            extras.append(f"{len(a.contributions.listeners)} listener(s)")
        suffix = f" [{'; '.join(extras)}]" if extras else ""
        lines.append(f"  {a.name} {a.ability.version}: {table}{suffix}")
    return lines


async def run_ability_command(name: str, args: list[str]) -> int:
    """``ghostd <ability> <command> [args]``: load just that ability and dispatch."""
    cfg = GhostdSettings()
    voice = VoiceSettings()  # type: ignore[call-arg]
    store = Store(cfg.db_path)
    try:
        [loaded] = await load(
            [name],
            discovered=discover(cfg.abilities_dir),
            make_context=lambda n: GhostContext.build(
                n, data_root=cfg.data_dir, cache_root=voice.cache_dir, store=store, env=os.environ
            ),
            passive_allowed=AutonomyPolicy.load(cfg.policy_path).passive,
        )
    except AbilityError as e:
        print(e, file=sys.stderr)
        return 1
    try:
        commands = {c.name: c for c in loaded.contributions.commands}
        if not args or args[0] not in commands:
            print(
                f"{name} commands: "
                + ", ".join(f"{c.name} ({c.description})" for c in commands.values())
            )
            return 2
        return commands[args[0]].run(args[1:])
    finally:
        await loaded.instance.aclose()
        store.close()


def build_jev(cfg: GhostdSettings) -> JevClient | None:
    """A Jev client per GHOST_JEV and whichever key is present, or None for regex only."""
    return JevClient.from_env(
        cfg.jev,
        {"TYPESAFE_API_KEY": cfg.typesafe_api_key, "OPENROUTER_API_KEY": cfg.openrouter_api_key},
        timeout=max(cfg.jev_deadline_secs, 0.1),
    )


def cli(argv: list[str] | None = None) -> None:
    argv = sys.argv[1:] if argv is None else argv
    cmd = argv[0] if argv else "run"
    logger.remove()
    logger.add(sys.stderr, level="INFO", format="{time:HH:mm:ss.SSS} {level:<7} {message}")

    if cmd == "run":
        cfg = GhostdSettings()
        voice = VoiceSettings()  # type: ignore[call-arg]
        if voice.log_level.upper() != "INFO":
            logger.remove()
            logger.add(
                sys.stderr,
                level=voice.log_level.upper(),
                format="{time:HH:mm:ss.SSS} {level:<7} {name}: {message}",
            )
        debug.enable(voice.debug_audio)
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
        if not voice.anthropic_base_url:
            problems.append("ANTHROPIC_BASE_URL is not set")
        autonomy = AutonomyPolicy()
        try:
            autonomy = AutonomyPolicy.load(cfg.policy_path)
            print(f"autonomy policy: {autonomy.source}")
        except ValueError as e:
            problems.append(f"policy: {e}")
        try:
            abilities = asyncio.run(load_abilities(cfg, voice, Store(), autonomy))
            print("abilities:")
            print("\n".join(describe_abilities(abilities, autonomy)))
        except AbilityError as e:
            problems.append(f"ability: {e}")
        try:
            jev = build_jev(cfg)
            how = f"jev via {jev.provider} (regex fallback)" if jev else "regex"
            print(f"confirmation: {how}")
        except ValueError as e:
            problems.append(str(e))
        print("\n".join(problems) if problems else "config ok")
        sys.exit(1 if problems else 0)
    elif cmd == "abilities":
        cfg = GhostdSettings()
        autonomy = AutonomyPolicy.load(cfg.policy_path)
        enabled = set(enabled_names(cfg, autonomy))
        for name, ab in sorted(discover(cfg.abilities_dir).items()):
            flag = "enabled " if name in enabled else "         "
            print(f"{flag}{name} {ab.version}: {ab.description}")
    elif cmd in discover(GhostdSettings().abilities_dir):
        sys.exit(asyncio.run(run_ability_command(cmd, argv[1:])))
    else:
        print(
            "usage: ghostd [run|wake|restore-volumes|devices|check|abilities|<ability> <command>]"
            f"  (got {cmd!r})"
        )
        sys.exit(2)


if __name__ == "__main__":
    cli()
