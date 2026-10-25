"""Passive -> wake -> conversation -> sleep, forever."""

from __future__ import annotations

import asyncio
import json
import signal
import sys
from collections.abc import Callable, Sequence
from dataclasses import replace
from datetime import timedelta
from pathlib import Path
from typing import Any

from ghost_api import (
    ApiServer,
    AuditPage,
    DaemonState,
    Deps,
    GhostClient,
    Phase,
    PhonePairing,
    PhoneView,
    Status,
    audit_page,
    create_app,
    default_socket_path,
    outcome_view,
    pending_view,
    status_snapshot,
)
from ghost_api.conversations import conversation_view
from ghost_api.models import ConversationView, StatusEvent
from ghost_core import (
    AutonomyPolicy,
    ConfirmationClassifier,
    Events,
    RegexClassifier,
    Store,
    ToolSource,
    WriteGate,
)
from ghost_core.conversations import GHOST, Conversations, Profile, conversations_source
from ghost_core.conversations import SOURCE as CONVERSATIONS
from ghost_core.jev import JevClassifier, JevClient
from ghost_core.mcp import Servers, source_of
from ghost_core.skills import Skills
from ghost_core.templates import Templates
from ghost_core.toolkits import Grant, merge
from ghost_voice import GhostSession, VoiceSettings, build_session, debug
from ghost_voice.elevenlabs import describe_tts, tts_problems
from ghost_voice.pipeline import TTS_SAMPLE_RATE, Models
from ghost_voice.wake import WakeListener, build_wake_listener, parse_phrases
from ghost_voice.wakeword import (
    ADDRESS_MODEL_GLOB,
    DEFAULT_MODEL_NAME,
    DETECTOR_PHRASE,
    Detector,
    WakeModel,
    WakeWordScorer,
)
from loguru import logger

from ghostd.audio import Ducker, chime, restore_from_state
from ghostd.config import GhostdSettings, runtime_dir
from ghostd.dictation import is_dictating
from ghostd.phone import PhoneListener, PhoneStore, Refusal
from ghostd.wake import FileWakeDetector, SpokenWakeDetector, WakeDetector

WAKE_FILE = "wake"
DUCK_STATE = "duck.json"
DEFAULT_WAKE_FUZZY = GhostdSettings.model_fields["wake_fuzzy"].default
PHONE_STATE = "phone.json"
VERSION = "0.1.0"
PHONE_PING_GRACE_SECS = 15  # no keepalive from the phone for this long ends its session;
# long enough for a cellular blip or a tunnel re-handshake away from home (ADR 0020)
PHONE_WRITE_TIMEOUT_SECS = 0.5  # a write to a gone phone fails after this; the desk is free sooner
PHONE_WIND_DOWN_SECS = 3.0  # how long the loop waits for a cancelled phone session to drain
# After the connection: how long to wait for the phone's client-ready. The phone holds it until
# its audio route has settled (a headset takes up to ~2 s), so this must outlast that.
PHONE_READY_SECS = 4.0
PHONE_GREETING_LEAD_SECS = 0.3  # let the phone's playback settle before the first word
PHONE_CONNECT_SECS = 30  # an accepted offer whose peer hasn't connected by then is dropped

# While a phone session winds down after the phone has gone, Pipecat's output transport
# reports its writes to the dead peer as errors. They are the expected tail of a hang-up,
# not faults; the sinks drop them while this is set.
_PHONE_TEARDOWN = {"on": False}
_TEARDOWN_NOISE = ("writing audio to the transport", "can no longer do its job")


def quiet_phone_teardown(record: Any) -> bool:
    """A loguru filter: True to keep the record."""
    if not _PHONE_TEARDOWN["on"]:
        return True
    if not str(record["name"]).startswith("pipecat"):
        return True
    message = record["message"]
    return not any(noise in message for noise in _TEARDOWN_NOISE)


def wake_model_paths(cfg: GhostdSettings, voice: VoiceSettings) -> tuple[Path, list[Path]]:
    """The "hey ghost" model and the "Ghost, …" ones there are, as set or in the voice
    cache."""
    wake_dir = voice.cache_dir / "wake"
    hey = cfg.wake_model_path or wake_dir / DEFAULT_MODEL_NAME
    if cfg.wake_address_model_path:
        address = [cfg.wake_address_model_path]
    else:
        address = sorted(wake_dir.glob(ADDRESS_MODEL_GLOB))
    return hey, [p for p in address if p.exists() and p != hey]


class Daemon:
    """Runs the loop and implements ``ghost_api.Controller`` for the desktop app."""

    def __init__(self, cfg: GhostdSettings, voice: VoiceSettings) -> None:
        self.cfg = cfg
        self.voice = voice
        self.rt = runtime_dir()
        self._stop = asyncio.Event()
        self.state = DaemonState(version=VERSION, wake_mode=cfg.wake_mode)
        self.session: GhostSession | None = None
        self.gate: WriteGate | None = None
        self._first_via = "voice"  # how the wake-time utterance arrived
        self._start_in = GHOST  # the line a manual wake opens the session in (a Call)
        self._manual: FileWakeDetector | None = None
        self._listener: WakeListener | None = None
        self._rearm = asyncio.Event()  # set when the passive listener must be rebuilt
        self._ducker: Ducker | None = None
        self._chimed = False  # the detector's early chime played; the session won't repeat it
        self.conversations: Conversations | None = None
        self.skills: Skills | None = None
        self.templates: Templates | None = None
        self._reach: Callable[[Profile], list[ToolSource]] | None = None
        self._models: Models | None = None
        self._deps: Deps | None = None
        # The phone (ADR 0016). An accepted offer's connection goes on the queue; the
        # passive loop takes it, releases the desk mic and runs the session on it.
        self.phone_listener: PhoneListener | None = None
        self._phone_q: asyncio.Queue[Any] = asyncio.Queue()
        self._phone_end = asyncio.Event()
        self._waiting = False  # the passive loop is waiting for a wake or a phone
        self._origin: str | None = None  # "desk" | "phone" while a session runs

    # --- Controller (called by the API) -------------------------------------

    async def wake(self, first_utterance: str | None, conversation: str | None = None) -> bool:
        """``conversation``: the line the session opens in (the desktop's Call button);
        Ghost when unset. A settled or unknown line is refused."""
        if self.state.phase is not Phase.PASSIVE or self._manual is None:
            return False
        if not self._phone_q.empty():  # a phone session is about to start
            return False
        if conversation is not None and conversation != GHOST:
            target = self.conversations.get(conversation) if self.conversations else None
            if target is None or target.settled:
                return False
        self._start_in = conversation or GHOST
        self._manual.trigger(first_utterance)
        return True

    async def sleep(self) -> bool:
        if self.session is None:
            return False
        self.state.set_ending()
        await self.session.end()
        return True

    async def say(self, text: str, conversation: str | None = None) -> bool:
        if self.state.phase is Phase.STARTING:
            return False
        if self.session is None:
            self._first_via = "text"
            return await self.wake(text, conversation)
        self.state.add_turn("user", text, via="text")
        await self.session.say_text(text)
        return True

    async def confirm_pending(self):
        if self.gate is None or self.session is None or self.state.session_id is None:
            return None
        outcome = await self.gate.confirm_pending(self.state.session_id, source="ui")
        if outcome is None:
            return None
        self.state.set_pending(None)
        self.state.outcome(outcome_view(outcome), "ui")
        await self.session.system_note(
            "[system] The operator confirmed from the desktop app. Verified result: "
            f"{outcome.summary()} Say what happened in a few words."
        )
        return outcome

    async def cancel_pending(self) -> bool:
        if self.gate is None or self.state.session_id is None:
            return False
        p = self.gate.cancel(self.state.session_id, "cancelled from the desktop app")
        if p is None:
            return False
        self.state.set_pending(None)
        if self.session:
            await self.session.system_note(
                "[system] The operator cancelled the pending change from the desktop app. "
                "Nothing was changed. Acknowledge briefly."
            )
        return True

    def reload_policy(self) -> AutonomyPolicy:
        policy = AutonomyPolicy.load(self.cfg.policy_path)
        if self.gate is not None:
            self.gate.set_autonomy(policy)
        logger.info(f"policy reloaded from {policy.source}")
        return policy

    def apply_settings(self, values: dict[str, str | None]) -> list[str]:
        """Apply what can change without a restart; return those keys. The wake phrases
        and the near-miss threshold are picked up by rebuilding the passive listener; the
        crossing-streams toggles (ADR 0019) by the next session, which reads them."""
        applied: list[str] = []
        for key, attr in (
            ("GHOST_CROSS_DEPARTMENT_ANSWERS", "cross_department_answers"),
            ("GHOST_DEPARTMENT_ROUTING", "department_routing"),
        ):
            if key in values:
                on = str(values[key] or "").strip().lower() in ("1", "true", "yes", "on")
                setattr(self.voice, attr, on)
                logger.info(f"{key.removeprefix('GHOST_').lower()}: {'on' if on else 'off'}")
                applied.append(key)
        if self._listener is None:
            return applied
        if "GHOST_WAKE_PHRASES" in values:
            phrases = parse_phrases(values["GHOST_WAKE_PHRASES"])
            self.cfg.wake_phrases = values["GHOST_WAKE_PHRASES"] or ""
            self._listener.set_phrases(phrases)
            logger.info(f"wake phrases: {', '.join(repr(p) for p in phrases)}")
            applied.append("GHOST_WAKE_PHRASES")
        if "GHOST_WAKE_FUZZY" in values:
            try:
                fuzzy = float(values["GHOST_WAKE_FUZZY"] or DEFAULT_WAKE_FUZZY)
            except ValueError:
                fuzzy = None
            if fuzzy is not None:
                self.cfg.wake_fuzzy = fuzzy
                self._listener.set_phrases(self._listener._phrases, fuzzy)
                logger.info(f"wake fuzzy threshold: {fuzzy:g}")
                applied.append("GHOST_WAKE_FUZZY")
        if applied:
            self._rearm.set()
        return applied

    # --- conversations (ADR 0014) ------------------------------------------------

    async def switch_conversation(self, slug: str) -> bool:
        """A click in the desktop app: the same transfer the model performs, fed in
        at the pipeline source so the announce, tone, voice and context frames
        arrive in order."""
        s = self.session
        if s is None or s.conversation is None or self.conversations is None:
            return False
        target = self.conversations.get(slug)
        if target is None or not s.conversation.switch_allowed():
            return False

        frames = await s.conversation.switch(target)
        try:
            if frames:
                await s.inject(frames)
        except Exception:
            s.conversation.transfer_done()
            raise
        return True

    async def compact_conversation(self, slug: str) -> bool:
        s = self.session
        if s is None or s.conversation is None or s.conversation.active.slug != slug:
            return False
        await s.system_note(
            "[system] The operator asked, from the desktop app, to compact this "
            "conversation's memory now. Call change with conversations.compact and a "
            "fresh summary, then say in a few words that it is done."
        )
        return True

    def voices(self) -> list[str]:
        if self._models is None:
            return []
        try:
            return sorted(self._models.tts.kokoro.voices)
        except Exception:  # noqa: BLE001
            return []

    async def sample_voice(self, voice: str, speed: float, text: str | None) -> bool:
        if self._models is None:
            return False
        line = text or f"Hi, this is {voice.partition('_')[2].capitalize()}. This is how I sound."
        kokoro = self._models.tts.kokoro

        def play() -> None:
            import sounddevice as sd

            samples, rate = kokoro.create(line, voice=voice, speed=speed, lang="en-us")
            sd.play(samples, rate, device=self.voice.output_device)
            sd.wait()

        try:
            await asyncio.to_thread(play)
        except Exception as e:  # noqa: BLE001
            logger.warning(f"voice sample failed: {e}")
            return False
        return True

    # --- the phone (ADR 0016): Controller side ---------------------------------

    def phone_pairing(self) -> PhonePairing | None:
        return self.phone_listener.new_pairing() if self.phone_listener else None

    def phone_view(self) -> PhoneView:
        if self.phone_listener is None:
            return PhoneView(address=None, device=None)
        return self.phone_listener.view()

    async def forget_phone(self) -> bool:
        return await self.phone_listener.forget() if self.phone_listener else False

    # --- the phone: PhoneHost side (the listener calls these) --------------------

    def status(self) -> Status:
        assert self._deps is not None
        return status_snapshot(self._deps)

    def audit(self, limit: int, offset: int) -> AuditPage:
        assert self._deps is not None
        return audit_page(self._deps.store, limit=limit, offset=offset)

    def admit_phone(self) -> Refusal | None:
        if self.state.phase is Phase.STARTING or not self.state.models_ready:
            return "not_ready"
        if (
            not self._waiting
            or self.session is not None
            or self.state.phase is not Phase.PASSIVE
            or not self._phone_q.empty()
        ):
            return "busy"
        return None

    def lines(self) -> list[ConversationView]:
        """The threads a phone call can start in: Ghost first, settled ones left out."""
        if self.conversations is None:
            return []
        profiles = self.conversations.list(include_settled=False)
        return [conversation_view(p, self.state.conversation) for p in profiles]

    async def start_phone(self, connection: Any, conversation: str | None = None) -> Refusal | None:
        line = conversation or GHOST
        if line != GHOST:
            profile = self.conversations.get(line) if self.conversations else None
            if profile is None or profile.settled:
                return "no_such_line"
        refusal = self.admit_phone()
        if refusal is None:
            self._phone_end.clear()
            self._phone_q.put_nowait((connection, line))
        return refusal

    async def end_phone(self) -> bool:
        if self._origin == "phone" or not self._phone_q.empty():
            _PHONE_TEARDOWN["on"] = True
            self._phone_end.set()
            return True
        return False

    # --- the loop --------------------------------------------------------------

    def build_wake(self, models: Models) -> WakeDetector:
        manual = FileWakeDetector(self.rt / WAKE_FILE)
        self._manual = manual
        if self.cfg.wake_mode == "transcript":
            phrases = parse_phrases(self.cfg.wake_phrases)
            logger.info(f"wake phrases: {', '.join(repr(p) for p in phrases)}")
            listener = build_wake_listener(
                self.voice,
                stt_factory=models.stt.fork,
                phrases=phrases,
                fuzzy=self.cfg.wake_fuzzy,
            )
            self._listener = listener
            return SpokenWakeDetector(listener, manual)
        if self.cfg.wake_mode == "file":
            return manual
        if self.cfg.wake_mode == "onnx":
            return self._detector_wake(models, manual)
        raise ValueError(f"unknown GHOST_WAKE_MODE {self.cfg.wake_mode!r}")

    def wake_model_path(self) -> Path:
        return wake_model_paths(self.cfg, self.voice)[0]

    def _detector_wake(self, models: Models, manual: FileWakeDetector) -> WakeDetector:
        """The trained detectors (ADR 0025): "hey ghost", and "Ghost, …" when its model
        is there; no transcript fallback. Without the "hey ghost" model Ghost would be
        deaf, so it says so loudly and listens by transcript instead."""
        path, address = wake_model_paths(self.cfg, self.voice)
        if not path.exists():
            logger.error(
                f"wake: GHOST_WAKE_MODE=onnx but there is no model at {path}; "
                "listening by transcript instead (set GHOST_WAKE_MODEL_PATH)"
            )
            self.cfg.wake_mode = self.state.wake_mode = "transcript"
            return self.build_wake(models)
        wake_models = [WakeModel.known(path, self.cfg.detector_threshold)]
        wake_models += [
            WakeModel.known(p, self.cfg.address_threshold, verify=self.cfg.address_verify)
            for p in address
        ]
        for m in wake_models:
            checked = self.cfg.wake_verify if m.verify is None else m.verify
            check = "Parakeet checks each hit" if checked else "no check"
            logger.info(f"wake: trained detector {m.name}, threshold {m.threshold:g}, {check}")
        logger.info("wake: those phrases only")
        detector = Detector(
            scorer=WakeWordScorer([path, *address]),
            recognize=models.stt.recognize,
            models=tuple(wake_models),
            verify=self.cfg.wake_verify,
            cooldown_secs=self.cfg.wake_cooldown_secs,
        )
        listener = build_wake_listener(
            self.voice,
            stt_factory=models.stt.fork,
            phrases=(DETECTOR_PHRASE,),
            fuzzy=self.cfg.wake_fuzzy,
            detector=detector,
        )
        self._listener = listener
        return SpokenWakeDetector(listener, manual, on_heard=self._heard_early)

    async def _heard_early(self) -> None:
        """The detector decided while the operator may still be talking ("hey ghost, what
        needs..."): duck and chime now, as a smart speaker does. The session starts once
        they finish, without a second chime."""
        if self._chimed or self._ducker is None:
            return
        self._chimed = True
        self._ducker.duck()
        if self.cfg.chime:
            asyncio.ensure_future(asyncio.to_thread(chime, True, self.voice.output_device))

    async def run(self) -> None:
        loop = asyncio.get_event_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            loop.add_signal_handler(sig, self._stop.set)

        self.cfg.db_path.parent.mkdir(parents=True, exist_ok=True)
        store = Store(self.cfg.db_path)
        autonomy = AutonomyPolicy.load(self.cfg.policy_path)
        mcp = Servers(autonomy.mcp)  # ADR 0022: the servers the policy file declares
        await mcp.connect()
        skills = Skills(self.cfg.skills_dir, self.cfg.home_skills_dir)  # ADR 0024
        self.skills = skills
        templates = Templates(self.cfg.infra_dir / "templates", self.cfg.templates_dir)
        self.templates = templates
        # What a session's model may call: every server's offered reads and declared
        # writes, and the skills (the index, read_skill, add and remove).
        tool_sources: list[ToolSource] = [*mcp.sources(), skills.source()]
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
            on_decision=debug.audio,
        )
        for a in tool_sources:  # declared writes go through the gate
            gate.register(a.name, a.operations)
        self.gate = gate
        conversations = Conversations(
            store,
            self.cfg.conversations_dir,
            default_voice=self.voice.tts_voice,
            default_speed=self.voice.tts_speed,
            retention=timedelta(days=self.cfg.turn_retention_days),
            # What a thread can be scoped to (ADR 0019): each scoped server's allowlist.
            scopes={a.name: a.scope_items for a in tool_sources if a.scope_kind and a.scope_items},
            # What a department can be given (ADR 0023): the policy file's toolkits, live,
            # and what each one grants on a server, so a request routes to the department.
            toolkits=lambda: list(gate.autonomy.toolkits),
            toolkit_items=lambda name, server: _granted_items(gate.autonomy, name, server),
            templates=templates,
        )
        gate.register(CONVERSATIONS, conversations_source(conversations).operations)
        self.conversations = conversations

        def reach(department: Profile) -> list[ToolSource]:
            """What a session in this department reaches (ADR 0023): everything for
            Ghost and for a department with no toolkits; else the toolkits' grants on
            the connected servers plus their skills. The lines' own tools always."""
            lines = conversations_source(conversations)
            if department.department:  # a topic line whose department is gone: nothing
                return [_without(lines, WIDENING)]
            if department.is_ghost or not department.toolkits:
                return [*tool_sources, lines]
            policy = gate.autonomy
            grants: dict[str, list[Grant]] = {}
            names: list[str] = []
            skill_names: set[str] = set()
            for name in department.toolkits:
                t = policy.toolkits.get(name)
                if t is None:
                    logger.warning(f"department {department.slug}: no toolkit {name}; skipped")
                    continue
                names.append(name)
                for server, g in t.grants.items():
                    grants.setdefault(server, []).append(g)
                skill_names |= set(t.skills)
            out: list[ToolSource] = []
            for server, gs in grants.items():
                live = mcp.servers.get(server)
                if live is not None:
                    out.append(source_of(live, merge(gs), "+".join(names)))
            out.append(skills.source(sorted(skill_names)))
            # A narrowed department cannot widen itself: no new lines, no toolkit changes,
            # no deletes from inside it; those happen from Ghost or the desktop.
            out.append(_without(lines, WIDENING))
            return out

        self._reach = reach
        pruned = conversations.prune()
        if pruned:
            logger.info(f"conversations: pruned {pruned} summarized turns past retention")
        self.state.servers = list(mcp.servers)
        events = Events()
        api = await self._start_api(gate, store, tool_sources, mcp, skills, templates)
        await self._start_phone()
        logger.info(f"autonomy policy: {autonomy.source}")
        logger.info(
            f"llm: {self.voice.llm_target().describe()}, thinking {self.voice.llm_thinking}"
        )
        for problem in self.voice.llm_problems():
            logger.error(f"llm: {problem}")
        logger.info(f"tts: {describe_tts(self.voice)}")
        for line in describe_mcp(mcp, autonomy):
            logger.info(line)
        logger.info(describe_skills(skills, set(mcp.servers)))
        logger.info(
            "confirmation classifier: "
            + (f"jev via {jev.provider}, regex fallback" if jev else "regex")
        )

        logger.info("loading models")
        models = await asyncio.to_thread(Models.load, self.voice)
        logger.info("models ready")
        self._models = models
        self.state.set_models_ready(True)
        self._bind_voices(conversations, models)

        ducker = Ducker(self.cfg.duck_apps, self.cfg.duck_level, self.rt / DUCK_STATE)
        restore_from_state(self.rt / DUCK_STATE)  # undo a previous crash
        self._ducker = ducker
        wake = self.build_wake(models)

        try:
            await self._passive(wake, models, tool_sources, gate, events, ducker)
        finally:
            wake.close()
            ducker.restore()
            if self.phone_listener:
                await self.phone_listener.stop()
            if api:
                await api.stop()
            await mcp.close()
            if jev:
                await jev.aclose()
            store.close()

    async def _passive(
        self,
        wake: WakeDetector,
        models: Models,
        tool_sources: Sequence[ToolSource],
        gate: WriteGate,
        events: Events,
        ducker: Ducker,
    ) -> None:
        """Passive until stopped: a wake phrase, the manual trigger or a phone call
        starts a session; a settings change rebuilds the listener."""
        while not self._stop.is_set():
            logger.info("passive: waiting for wake word")
            self.state.set_phase(Phase.PASSIVE)
            self._rearm.clear()
            self._start_in = GHOST  # a spoken wake lands in Ghost; a Call sets it
            wake_task = asyncio.ensure_future(wake.wait())
            stop_task = asyncio.ensure_future(self._stop.wait())
            rearm_task = asyncio.ensure_future(self._rearm.wait())
            phone_task = asyncio.ensure_future(self._phone_q.get())
            self._waiting = True
            try:
                await asyncio.wait(
                    {wake_task, stop_task, rearm_task, phone_task},
                    return_when=asyncio.FIRST_COMPLETED,
                )
            finally:
                self._waiting = False
            stop_task.cancel()
            rearm_task.cancel()
            phone = phone_task.result() if phone_task.done() else None
            phone_task.cancel()
            if phone is None and not self._phone_q.empty():  # accepted in the same tick
                phone = self._phone_q.get_nowait()
            phone_conn, phone_line = phone if phone is not None else (None, GHOST)
            # Decided before the cancel below, which makes a pending task done too.
            woke = wake_task.done()
            if self._stop.is_set() or phone is not None or not woke:
                wake_task.cancel()
                await asyncio.gather(wake_task, return_exceptions=True)  # releases the mic
            if self._chimed and (phone is not None or not woke):
                ducker.restore()  # the early chime played, then something else came first
                self._chimed = False
            if self._stop.is_set():
                if phone_conn is not None:
                    await _close_peer(phone_conn)
                break
            if phone_conn is not None:  # the phone takes the session; the desk mic is free
                await self._conversation(
                    models,
                    tool_sources,
                    gate,
                    events,
                    ducker,
                    None,
                    phone=phone_conn,
                    start_in=phone_line,
                )
                continue
            if not woke:  # settings changed: rebuild the listener
                continue
            if is_dictating(self.cfg.dictation_flag):
                logger.info("wake ignored: dictation is active")
                if self._chimed:
                    ducker.restore()
                    self._chimed = False
                continue
            await events.emit("wake")
            first = wake_task.result()
            await self._conversation(
                models, tool_sources, gate, events, ducker, first, start_in=self._start_in
            )

    def _bind_voices(self, conversations: Conversations, models: Models) -> None:
        """Once Kokoro is loaded, the operations validate voice ids against its list."""
        voices = self.voices()
        if voices and self.gate is not None:
            conversations._voices = tuple(voices)  # noqa: SLF001
            self.gate.register(CONVERSATIONS, conversations_source(conversations).operations)

    async def _start_api(
        self,
        gate: WriteGate,
        store: Store,
        sources: Sequence[ToolSource],
        mcp: Servers | None = None,
        skills: Skills | None = None,
        templates: Templates | None = None,
    ) -> ApiServer | None:
        socket_path = self.cfg.api_socket or default_socket_path()
        deps = Deps(
            state=self.state,
            control=self,
            gate=gate,
            store=store,
            sources=sources,
            mcp=mcp,
            skills=skills,
            templates=templates,
            policy_path=self.cfg.policy_path,
            settings_path=self.cfg.settings_path,
            env_path=self.cfg.env_path,
            vault_env_path=self.cfg.vault_env_path,
            infra_dir=self.cfg.infra_dir,
            cache_dir=self.voice.cache_dir,
            input_device=self.voice.input_device,
            output_device=self.voice.output_device,
            stt_model=self.voice.stt_model,
            conversations=self.conversations,
        )
        self._deps = deps
        server = ApiServer(create_app(deps), socket_path)
        try:
            await server.start()
        except Exception:  # noqa: BLE001 - the voice loop must run even if the API can't
            logger.exception("api failed to start; continuing without it")
            return None
        # Mirror the log into the event stream for the desktop app's log view.
        logger.add(
            lambda m: self.state.log(m.record["level"].name, m.record["message"]),
            level="INFO",
            format="{message}",
            filter=quiet_phone_teardown,
        )
        return server

    async def _start_phone(self) -> None:
        """The phone listener, when GHOST_PHONE_LISTEN is set (ADR 0016). It starts before
        the models load so /hello answers and /offer says not_ready meanwhile."""
        if not self.cfg.phone_listen.strip():
            return
        try:
            listener = PhoneListener(
                self.cfg.phone_listen,
                store=PhoneStore(self.cfg.data_dir / PHONE_STATE),
                host=self,
            )
            await listener.start()
        except (OSError, ValueError, RuntimeError) as e:
            logger.error(f"phone: listener not started on {self.cfg.phone_listen!r}: {e}")
            return
        self.phone_listener = listener

    async def _conversation(
        self,
        models: Models,
        sources: Sequence[ToolSource],
        gate: WriteGate,
        events: Events,
        ducker: Ducker,
        first_utterance: str | None = None,
        *,
        phone: Any = None,
        start_in: str = GHOST,
    ) -> None:
        """One session. With ``phone`` (a negotiated SmallWebRTCConnection) the audio is
        the phone's: no ducking, no desk chime, the thinking sound in its track, and the
        session's events go to it over RTVI (ADR 0016). ``start_in`` is the thread the
        session opens in: Ghost, or the one the phone called directly."""
        origin = "phone" if phone is not None else "desk"
        if phone is None and not self._chimed:
            ducker.duck()
            if self.cfg.chime:
                await asyncio.to_thread(chime, True, self.voice.output_device)
        self._chimed = False
        connected = asyncio.Event()
        peer_gone = asyncio.Event()
        transport_factory = (
            self._phone_transport(phone, connected, peer_gone) if phone is not None else None
        )

        async def on_turn(role: str, text: str) -> None:
            self.state.add_turn(role, text)
            if role == "user":
                self._refresh_pending()

        async def on_phase(phase: str) -> None:
            self.state.set_phase(Phase(phase), self.state.session_id)
            if phase == "listening":
                self._refresh_pending()

        async def on_conversation(slug: str) -> None:
            self.state.set_conversation(slug)
            self._refresh_pending()

        async def on_hang_up() -> None:
            self.state.set_ending()  # the phone and the desktop app show "Hanging up"
            if phone is not None:
                # From here the phone may close the call before the desk's last write lands.
                _PHONE_TEARDOWN["on"] = True

        session = build_session(
            self.voice,
            models=models,
            sources=sources,
            gate=gate,
            events=events,
            first_utterance=first_utterance,
            on_turn=on_turn,
            on_phase=on_phase,
            conversations=self.conversations,
            reach=self._reach,
            start_in=start_in,
            on_conversation=on_conversation,
            transport_factory=transport_factory,
            on_hang_up=on_hang_up,
            wake_phrases=parse_phrases(self.cfg.wake_phrases),
            wake_fuzzy=self.cfg.wake_fuzzy,
            origin=origin,
            latency_log=self.cfg.latency_path,
        )
        self.session = session
        self._origin = origin
        self.state.set_phase(Phase.LISTENING, session.session_id, origin=origin)
        self.state.set_conversation(start_in)
        if first_utterance:
            self.state.add_turn("user", first_utterance, via=self._first_via)
        self._first_via = "voice"
        logger.info(f"session {session.session_id} start ({origin})")
        await events.emit("session_start", session_id=session.session_id, origin=origin)
        waits = {asyncio.ensure_future(self._stop.wait())}
        helpers: list[asyncio.Task[Any]] = []
        events_q = None
        if phone is not None:
            events_q = self.state.bus.subscribe()
            helpers = self._phone_helpers(session, phone, events_q, connected, peer_gone)
            waits |= {
                asyncio.ensure_future(peer_gone.wait()),
                asyncio.ensure_future(self._phone_end.wait()),
            }
        try:
            await session.start(open=phone is None)  # type: ignore[attr-defined]
            run_task = asyncio.ensure_future(session.run())
            done, _ = await asyncio.wait({run_task, *waits}, return_when=asyncio.FIRST_COMPLETED)
            if run_task not in done and phone is not None and not self._stop.is_set():
                why = "phone ended it" if self._phone_end.is_set() else "phone went away"
                logger.info(f"session {session.session_id}: {why}")
                _PHONE_TEARDOWN["on"] = True
            if run_task not in done:
                # Cancel rather than end: for the phone an EndFrame would wait on writes
                # to a dead peer. Safe because each session forks its own processors
                # from Models, so a cancelled one can't leave the next one mute.
                await session.cancel()
                try:
                    await asyncio.wait_for(asyncio.shield(run_task), PHONE_WIND_DOWN_SECS)
                except TimeoutError:
                    # The pipeline is still draining to a dead peer; let it finish on its
                    # own so the desk is free for the next Start now.
                    logger.info(f"session {session.session_id}: still winding down; moving on")
        except Exception:
            logger.exception("session failed")
        finally:
            for t in [*waits, *helpers]:
                t.cancel()
            await asyncio.gather(*helpers, return_exceptions=True)
            if events_q is not None:
                self.state.bus.unsubscribe(events_q)
            if phone is not None:
                await _close_peer(phone)  # the session ending closes the peer
                self._phone_end.clear()
                _PHONE_TEARDOWN["on"] = False
            elif self.cfg.chime:
                await asyncio.to_thread(chime, False, self.voice.output_device)
            ducker.restore()
            self.session = None
            self._origin = None
            logger.info(f"session {session.session_id} end")
            await events.emit("session_end", session_id=session.session_id, origin=origin)

    def _phone_transport(self, conn: Any, connected: asyncio.Event, peer_gone: asyncio.Event):
        """A factory for the phone's SmallWebRTCTransport, given the thinking-sound mixer."""
        from pipecat.transports.base_transport import TransportParams
        from pipecat.transports.smallwebrtc.transport import SmallWebRTCTransport

        def factory(mixer: Any) -> SmallWebRTCTransport:
            transport = SmallWebRTCTransport(
                webrtc_connection=conn,
                params=TransportParams(
                    audio_in_enabled=True,
                    audio_out_enabled=True,
                    audio_out_sample_rate=TTS_SAMPLE_RATE,
                    audio_out_mixer=mixer,
                    # The phone closes the call itself after the goodbye; a write to the
                    # gone peer should fail fast, not hold the session for the default 10 s.
                    audio_out_write_timeout_secs=PHONE_WRITE_TIMEOUT_SECS,
                ),
            )

            @transport.event_handler("on_client_connected")
            async def _connected(_t: Any, _c: Any) -> None:
                path = _media_path(conn)
                logger.info(f"phone: connected{f' over {path}' if path else ''}")
                connected.set()

            @transport.event_handler("on_client_disconnected")
            async def _disconnected(_t: Any, _c: Any) -> None:
                peer_gone.set()

            for state in ("closed", "failed"):

                @conn.event_handler(state)
                async def _gone(_c: Any) -> None:
                    peer_gone.set()

            return transport

        return factory

    def _phone_helpers(
        self,
        session: GhostSession,
        conn: Any,
        events_q: asyncio.Queue[str],
        connected: asyncio.Event,
        peer_gone: asyncio.Event,
    ) -> list[asyncio.Task[Any]]:
        """Greet once the phone's audio is up, forward the desk's events to it once its
        data channel says client-ready, and notice when its keepalive stops."""
        ready = asyncio.Event()

        async def on_ready() -> None:
            ready.set()

        session.on_client_ready(on_ready)

        async def greet() -> None:
            # The phone says client-ready once its speaker route and the remote track are
            # up; greeting on the bare connection lost the first half-second ("...ost. What
            # do you need?"). An older app that never says it still gets greeted.
            await connected.wait()
            try:
                await asyncio.wait_for(ready.wait(), PHONE_READY_SECS)
            except TimeoutError:
                logger.info("phone: no client-ready; greeting anyway")
            await asyncio.sleep(PHONE_GREETING_LEAD_SECS)
            await session.open()  # type: ignore[attr-defined]

        async def forward() -> None:
            # The same JSON the control API's WebSocket sends; never logged, and the
            # log events are not sent at all (docs/phone-protocol.md).
            await ready.wait()
            backlog = []  # what happened before the channel opened, minus stale statuses
            while not events_q.empty():
                data = json.loads(events_q.get_nowait())
                if data.get("type") not in ("log", "status"):
                    backlog.append(data)
            for data in backlog:
                await session.send_server_message(data)
            snapshot = StatusEvent(status=self.status()).model_dump(mode="json")
            await session.send_server_message(snapshot)
            while True:
                data = json.loads(await events_q.get())
                if data.get("type") != "log":
                    await session.send_server_message(data)

        async def watchdog() -> None:
            try:
                await asyncio.wait_for(connected.wait(), PHONE_CONNECT_SECS)
            except TimeoutError:
                logger.info("phone: the peer never connected")
                peer_gone.set()
                return
            missed = 0
            while missed < PHONE_PING_GRACE_SECS:
                await asyncio.sleep(1)
                missed = 0 if conn.is_connected() else missed + 1
            peer_gone.set()

        return [asyncio.ensure_future(c) for c in (greet(), forward(), watchdog())]

    def _refresh_pending(self) -> None:
        """Publish the pending proposal when it appears or goes away."""
        if self.gate is None or self.state.session_id is None:
            return
        p = self.gate.pending(self.state.session_id)
        view = pending_view(p) if p else None
        if (view.id if view else None) != (self.state.pending.id if self.state.pending else None):
            self.state.set_pending(view)


def _media_path(conn: Any) -> str | None:
    """The nominated ICE pair as ``local -> remote`` (addresses and candidate types), for the
    log: it says whether the phone came in over the LAN or the tunnel (ADR 0020)."""
    try:
        ice = conn._pc.sctp.transport.transport  # RTCIceTransport, through DTLS and SCTP
        pair = ice._connection._nominated[1]
        lc, rc = pair.local_candidate, pair.remote_candidate
        return f"{lc.host}:{lc.port} ({lc.type}) -> {rc.host}:{rc.port} ({rc.type})"
    except Exception:  # noqa: BLE001 - private aiortc/aioice state; a missing path is fine
        return None


async def _close_peer(conn: Any) -> None:
    try:
        await conn.disconnect()
    except Exception:  # noqa: BLE001 - already closed
        logger.debug("phone: peer already closed")


def _granted_items(policy: AutonomyPolicy, toolkit: str, server: str) -> list[str] | None:
    """The scope items a toolkit grants on a server: its own list, else the server's whole
    allowlist; None when the toolkit does not grant the server."""
    t = policy.toolkits.get(toolkit)
    g = t.grant(server) if t else None
    if g is None:
        return None
    if g.items is not None:
        return list(g.items)
    spec = policy.mcp.get(server)
    return list(spec.scope.items) if spec and spec.scope else []


WIDENING = frozenset({"create", "set_toolkits", "delete"})


def _without(source: ToolSource, ops: frozenset[str]) -> ToolSource:
    return replace(source, operations=[o for o in source.operations if o.name not in ops])


def describe_skills(skills: Skills, servers: set[str]) -> str:
    items = []
    for s in skills.all():
        missing = s.missing(servers)
        where = "" if s.source == "ghost" else f" ({s.source})"
        note = f" [problem: {s.problem}]" if s.problem else ""
        if missing:
            note += f" [needs {', '.join(missing)}]"
        items.append(f"{s.name}{where}{note}")
    return f"skills: {', '.join(items) if items else 'none'} (in {skills.ghost_dir})"


def describe_mcp(mcp: Servers, autonomy: AutonomyPolicy) -> list[str]:
    lines = []
    for name, spec in mcp.specs.items():
        live = mcp.servers.get(name)
        if live is None:
            lines.append(f"  mcp {name} ({spec.transport}): not connected: {mcp.failed.get(name)}")
            continue
        offered = ", ".join(t.name for t in live.offered()) or "none"
        ops = {o.name: o for o in live.operations()}
        levels = autonomy.describe(name, {n: o.risk for n, o in ops.items()})
        writes = ", ".join(f"{n}={lv.value}" for n, lv in levels.items())
        undeclared = [
            t.name
            for t in live.tools
            if not t.read_only and t.name not in {w.tool for w in spec.writes.values()}
        ]
        line = f"  mcp {name} ({spec.transport}): reads {offered}; writes {writes or 'none'}"
        if undeclared:
            line += f"; not declared, not offered: {', '.join(undeclared)}"
        if spec.scope:
            line += (
                f" [scoped by {spec.scope.kind}: {', '.join(spec.scope.items) or 'no allowlist'}]"
            )
        lines.append(line)
    return lines


def _client_command(cmd: str, args: list[str]) -> int:
    """Talk to the running daemon over its socket; `wake` falls back to the wake file."""
    cfg = GhostdSettings()
    sock = cfg.api_socket or default_socket_path()
    text = " ".join(args).strip() or None
    if not sock.exists():
        if cmd == "wake":
            (runtime_dir() / WAKE_FILE).touch()
            print("wake requested (file; daemon API not running)")
            return 0
        print(f"ghostd is not running (no socket at {sock})", file=sys.stderr)
        return 1
    client = GhostClient(sock)
    try:
        if cmd == "wake":
            out = client.wake(text)
        elif cmd == "say":
            if not text:
                print("usage: ghostd say <text>", file=sys.stderr)
                return 2
            out = client.say(text)
        elif cmd == "sleep":
            out = client.sleep()
        else:
            out = client.status()
        import json

        print(json.dumps(out, indent=2))
        return 0 if out.get("ok", True) else 1
    finally:
        client.close()


def latency_command(args: list[str]) -> int:
    """``ghostd latency [--last N] [--file PATH] [--origin desk|phone] [--json]``: p50/p95
    per stage from the per-turn records, overall and per origin, then the last N turns."""
    import argparse

    from ghost_voice.latency import format_line, read_records, summarize, summary_table

    ap = argparse.ArgumentParser(prog="ghostd latency")
    ap.add_argument("--file", type=Path, default=None, help="records (default: data_dir)")
    ap.add_argument("--last", type=int, default=10, help="list the last N turns")
    ap.add_argument("--since", type=int, default=0, help="only the newest N records")
    ap.add_argument("--origin", choices=("desk", "phone"), default=None)
    ap.add_argument("--json", action="store_true", help="print the summary as JSON")
    a = ap.parse_args(args)
    path = a.file or GhostdSettings().latency_path
    records = read_records(path)
    if a.origin:
        records = [r for r in records if r.get("origin") == a.origin]
    if a.since:
        records = records[-a.since :]
    if not records:
        print(f"no latency records in {path}")
        return 1
    if a.json:
        out = {
            "all": summarize(records),
            **{
                o: summarize([r for r in records if r.get("origin") == o])
                for o in sorted({str(r.get("origin")) for r in records})
            },
        }
        print(json.dumps(out, indent=2))
        return 0
    print("\n".join(summary_table(records, f"all turns in {path}")))
    origins = sorted({str(r.get("origin")) for r in records})
    for origin in origins if len(origins) > 1 else []:
        print()
        print("\n".join(summary_table([r for r in records if r.get("origin") == origin], origin)))
    for kind in ("voice", "text", "opening"):
        sub = [r for r in records if r.get("kind") == kind]
        if sub and len(sub) != len(records):
            print()
            print("\n".join(summary_table(sub, f"{kind} turns")))
    if a.last:
        print(f"\nlast {min(a.last, len(records))} turns:")
        for r in records[-a.last :]:
            print(f"  {r.get('ts', '')[11:19]} {format_line(r).removeprefix('latency: ')}")
    return 0


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
    logger.add(
        sys.stderr,
        level="INFO",
        format="{time:HH:mm:ss.SSS} {level:<7} {message}",
        filter=quiet_phone_teardown,
    )

    if cmd == "run":
        cfg = GhostdSettings()
        voice = VoiceSettings()  # type: ignore[call-arg]
        if voice.log_level.upper() != "INFO":
            logger.remove()
            logger.add(
                sys.stderr,
                level=voice.log_level.upper(),
                format="{time:HH:mm:ss.SSS} {level:<7} {name}: {message}",
                filter=quiet_phone_teardown,
            )
        debug.enable(voice.debug_audio)
        asyncio.run(Daemon(cfg, voice).run())
    elif cmd in {"wake", "say", "sleep", "status"}:
        sys.exit(_client_command(cmd, argv[1:]))
    elif cmd == "latency":
        sys.exit(latency_command(argv[1:]))
    elif cmd == "perf":
        from ghostd.perf.cli import main as perf_main

        sys.exit(perf_main(argv[1:]))
    elif cmd == "restore-volumes":
        restore_from_state(runtime_dir() / DUCK_STATE)
    elif cmd == "devices":
        import sounddevice as sd

        print(sd.query_devices())
    elif cmd == "check":
        cfg = GhostdSettings()
        problems = []
        try:
            voice = VoiceSettings()  # type: ignore[call-arg]
        except Exception as e:  # noqa: BLE001 - pydantic-settings reports what is wrong
            # Field names and messages only: pydantic's input values could be secrets.
            errs = getattr(e, "errors", lambda: [])()
            problems += [
                f"voice settings: {err.get('loc', ['?'])[0]}: {err.get('msg', '')}" for err in errs
            ] or ["voice settings could not be read"]
            voice = VoiceSettings.model_construct()  # type: ignore[call-arg]
        else:
            print(f"llm: {voice.llm_target().describe()}, thinking {voice.llm_thinking}")
            problems += voice.llm_problems()
            print(f"tts: {describe_tts(voice)}")
            problems += tts_problems(voice)
            if cfg.wake_mode == "onnx":
                model, address = wake_model_paths(cfg, voice)
                check = "Parakeet checks each hit" if cfg.wake_verify else "no check"
                threshold = cfg.detector_threshold
                print(f"wake: trained detector {model}, threshold {threshold:g}, {check}")
                check = "Parakeet checks each hit" if cfg.address_verify else "no check"
                for p in address:
                    print(f"wake: and {p}, threshold {cfg.address_threshold:g}, {check}")
                if not model.exists():
                    problems.append(f"GHOST_WAKE_MODE=onnx but there is no model at {model}")
            else:
                print(f"wake: {cfg.wake_mode}")
        autonomy = AutonomyPolicy()
        try:
            autonomy = AutonomyPolicy.load(cfg.policy_path)
            print(f"autonomy policy: {autonomy.source}")
        except ValueError as e:
            problems.append(f"policy: {e}")
        if autonomy.mcp:

            async def probe() -> list[str]:
                mcp = Servers(autonomy.mcp)
                await mcp.connect()
                try:
                    return describe_mcp(mcp, autonomy)
                finally:
                    await mcp.close()

            print("mcp servers:")
            for line in asyncio.run(probe()):
                print(line)
                if "not connected" in line:
                    problems.append(line.strip())
        print(describe_skills(Skills(cfg.skills_dir, cfg.home_skills_dir), set(autonomy.mcp)))
        try:
            jev = build_jev(cfg)
            how = f"jev via {jev.provider} (regex fallback)" if jev else "regex"
            print(f"confirmation: {how}")
        except ValueError as e:
            problems.append(str(e))
        print("\n".join(problems) if problems else "config ok")
        sys.exit(1 if problems else 0)
    else:
        print(
            "usage: ghostd [run|wake|say <text>|sleep|status|latency|perf|restore-volumes|"
            "devices|check]"
            f"  (got {cmd!r})"
        )
        sys.exit(2)


if __name__ == "__main__":
    cli()
