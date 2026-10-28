"""Passive -> wake -> conversation -> sleep, forever."""

from __future__ import annotations

import asyncio
import json
import os
import signal
import sys
import time
from collections.abc import Awaitable, Callable, Sequence
from datetime import UTC, datetime, timedelta
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
    WaitingView,
    audit_page,
    create_app,
    default_socket_path,
    outcome_view,
    pending_view,
    status_snapshot,
)
from ghost_api import waiting_view as build_waiting_view
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
from ghost_core.apps import APPS_FILE, Apps, load_apps
from ghost_core.catalog import Catalog
from ghost_core.conversations import GHOST, Conversations, Profile, conversations_source
from ghost_core.conversations import SOURCE as CONVERSATIONS
from ghost_core.folder import GhostFolder
from ghost_core.hooks import HookIntake, run_fixtures
from ghost_core.jev import JevClassifier, JevClient, watch_decider
from ghost_core.outputs import SOURCE as OUTPUTS
from ghost_core.outputs import Outputs
from ghost_core.passwords import Passwords
from ghost_core.routes import RouteEngine, RoutePolicy, SpokenDigest
from ghost_core.rules import SOURCE as RULES
from ghost_core.rules import Rules, rules_source
from ghost_core.signin import SignIns
from ghost_core.skill_catalog import SkillCatalog
from ghost_core.skills import SOURCE as SKILLS
from ghost_core.skills import Skills
from ghost_core.waiting import Item, Items, WaitingList, waiting_source
from ghost_core.watches import WatchRunner
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

from ghostd import build as ghost_build
from ghostd.audio import Ducker, chime, restore_from_state
from ghostd.config import GhostdSettings, runtime_dir
from ghostd.deciders import model_decider
from ghostd.dictation import is_dictating
from ghostd.hooks import HooksListener
from ghostd.passwords import VAULT_RETRY_SECS, in_use, open_passwords, passwords_cli, redacting
from ghostd.passwords import problems as password_problems
from ghostd.phone import PhoneListener, PhoneStore, Refusal
from ghostd.routes import (
    GREETING_SECS,
    REPLY_WINDOW_SECS,
    SNOOZE_MINUTES,
    DeskRoute,
    DesktopRoute,
    PhoneRoute,
    context_note,
    digest_note,
    is_not_now,
)
from ghostd.wake import FileWakeDetector, SpokenWakeDetector, WakeDetector

WAKE_FILE = "wake"
DUCK_STATE = "duck.json"
DEFAULT_WAKE_FUZZY = GhostdSettings.model_fields["wake_fuzzy"].default
PHONE_STATE = "phone.json"
WAITING_REFRESH_SECS = 15  # how often Ghost's own items are re-read and the routes tick
AT_DESK_SECS = 15 * 60  # a desk session this recent means the operator is at the desk
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
def declared_urgency(folder: GhostFolder) -> dict[str, str]:
    """Each watch's and hook's declared urgency, by its items' source."""
    out = {w.source: w.urgency for w in folder.watches.values()}
    out.update({h.source: h.urgency for h in folder.hooks.values()})
    return out


def describe_routes(p: RoutePolicy) -> str:
    quiet = f", quiet {p.quiet_hours[0]:%H:%M}-{p.quiet_hours[1]:%H:%M}" if p.quiet_hours else ""
    return f"routes: {', '.join(p.enabled)}; escalate after {p.escalate_after} min{quiet}"


def describe_watches(folder: GhostFolder) -> list[str]:
    """One line per watch for the log and ``ghostd check``."""
    out = []
    for w in folder.watches.values():
        steps = "check" + (", jev" if w.jev else "") + (", model" if w.model else "")
        out.append(f"watch {w.name}: {w.app}.{w.read} every {w.every}s, {steps}, {w.response}")
    return out


def check_folder(config_dir: Path) -> list[str]:
    """``ghostd check`` for Ghost's folder (ADR 0035): the watches and hooks, each hook's
    fixtures and secret, and what in the folder doesn't read or is no longer read (an
    older layout's department.json, memory.md, docs/, departments/). Prints the lines;
    returns the problems."""
    folder = GhostFolder(config_dir)
    problems: list[str] = []
    print(f"folder: {folder.path}")
    for problem in folder.problems:
        print(f"  problem: {problem}")
        problems.append(problem)
    for line in describe_watches(folder):
        print(line)
    for h in folder.hooks.values():
        failures = run_fixtures(h)
        print(
            f"hook {h.name}: {h.verify}, {len(h.events)} event(s), "
            + ("fixtures pass" if not failures else f"{len(failures)} fixture(s) fail")
        )
        problems += [f"hook {h.name}: {f}" for f in failures]
        if not os.environ.get(h.secret_name()):
            problems.append(f"hook {h.name}: password {h.secret_name()} is not set")
    return problems


def ghost_problems(
    apps: Apps, folder: GhostFolder, passwords: list[tuple[str, str]] | None = None
) -> list[tuple[str, str]]:
    """Ghost's own trouble, for the waiting list (ADR 0029): a keyring to unlock or
    passwords still in plain files (ADR 0036), an app that isn't connected or wants the
    operator to sign in again, a file in Ghost's folder that doesn't read or is no longer
    read (ADR 0035)."""
    out: list[tuple[str, str]] = list(passwords or [])
    for name, spec in apps.specs.items():
        state = apps.state(name)
        if state == "connected":
            continue
        if state == "needs_sign_in":
            out.append((f"app:{name}", f"{spec.shown} needs you to sign in again"))
        else:
            why = apps.problem(name) or "not connected"
            out.append((f"app:{name}", f"{spec.shown} isn't connected: {why}"))
    if folder.problems:
        out.append(("folder", f"Ghost's folder has a problem: {folder.problems[0]}"))
    return out


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

    def __init__(
        self, cfg: GhostdSettings, voice: VoiceSettings, passwords: Passwords | None = None
    ) -> None:
        self.cfg = cfg
        self.voice = voice
        self.passwords = passwords  # ADR 0036; opened in cli(), or in run()
        self._jevs: list[JevClient] = []  # closed when the daemon stops
        self.rt = runtime_dir()
        self._stop = asyncio.Event()
        self.state = DaemonState(
            version=ghost_build.version(), wake_mode=cfg.wake_mode, build=ghost_build.detect()
        )
        self.session: GhostSession | None = None
        self.gate: WriteGate | None = None
        self.waiting: WaitingList | None = None  # ADR 0029; set in run()
        self.watch_runner: WatchRunner | None = None  # ADR 0028; set in run()
        self.hook_intake: HookIntake | None = None  # ADR 0027; set in run()
        self.routes: RouteEngine | None = None  # ADR 0031; set in run()
        self._last_desk_session: float | None = None
        self._phone_route = PhoneRoute()  # ADR 0031: the app's list, while it is open
        self._first_via = "voice"  # how the wake-time utterance arrived
        # The running session's turn handler, for a typed turn (the desktop's chat input):
        # it answers the digest and desk voice the way a spoken one does.
        self._heard: Callable[[str, str], Awaitable[None]] | None = None
        self._start_in = GHOST  # the line a manual wake opens the session in (a Call)
        self._manual: FileWakeDetector | None = None
        self._listener: WakeListener | None = None
        self._rearm = asyncio.Event()  # set when the passive listener must be rebuilt
        self._ducker: Ducker | None = None
        self._chimed = False  # the detector's early chime played; the session won't repeat it
        self.conversations: Conversations | None = None
        self.skills: Skills | None = None
        self.folder: GhostFolder | None = None  # ADR 0035; set in run()
        self.rules: Rules | None = None
        self.outputs: Outputs | None = None
        self.apps: Apps | None = None
        self.signins: SignIns | None = None
        self.catalog: Catalog | None = None
        self.skill_catalog: SkillCatalog | None = None
        self._reach: Callable[[Profile], list[ToolSource]] | None = None
        self._models: Models | None = None
        self._deps: Deps | None = None
        # The phone (ADR 0016). An accepted offer's connection goes on the queue; the
        # passive loop takes it, releases the desk mic and runs the session on it.
        self.phone_listener: PhoneListener | None = None
        self._phone_q: asyncio.Queue[Any] = asyncio.Queue()
        self._phone_end = asyncio.Event()
        self._waiting = False  # the passive loop is waiting for a wake or a phone
        # ADR 0032: Ghost has something to raise at the desk: (items, policy, result future).
        self._raise_q: asyncio.Queue[Any] = asyncio.Queue()
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
            if target is None or target.retired:
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
        if self._heard is not None:
            await self._heard("user", text)
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

    async def confirm_proposal(self, proposal_id: str):
        """A Confirm on a change on the waiting list (ADR 0030). When it is the one the live
        session is waiting on, the session hears the result too."""
        if self.gate is None:
            return None
        live = self.gate.pending(self.state.session_id) if self.state.session_id else None
        outcome = await self.gate.confirm_id(proposal_id, source="ui")
        if outcome is None:
            return None
        self.state.outcome(outcome_view(outcome), "ui")
        if live is not None and live.id == proposal_id:
            self.state.set_pending(None)
            if self.session:
                await self.session.system_note(
                    "[system] The operator confirmed from the desktop app. Verified result: "
                    f"{outcome.summary()} Say what happened in a few words."
                )
        self._refresh_waiting_now()
        return outcome

    async def cancel_proposal(self, proposal_id: str) -> bool:
        if self.gate is None:
            return False
        live = self.gate.pending(self.state.session_id) if self.state.session_id else None
        if self.gate.cancel_id(proposal_id, "cancelled from the waiting list") is None:
            return False
        if live is not None and live.id == proposal_id:
            self.state.set_pending(None)
        self._refresh_waiting_now()
        return True

    def _on_signin_url(self, app: str, url: str) -> None:
        """An app is waiting for the operator to sign in: the desktop opens the address."""
        self.state.publish_apps(changed=app, sign_in_url=url)

    def _refresh_waiting_now(self) -> None:
        if self.waiting is not None:
            self.waiting.refresh()

    async def _refresh_waiting(self, waiting: WaitingList) -> None:
        """Re-read Ghost's own items now and then (a change's time running out, a server
        that went away; watches sync their own), and let the routes send what is due
        (ADR 0031)."""
        while True:
            await asyncio.sleep(WAITING_REFRESH_SECS)
            try:
                waiting.refresh()
                if self.routes is not None:
                    await self.routes.tick()
            except Exception:  # noqa: BLE001 - the list must not take the daemon down
                logger.exception("waiting: refresh failed")

    def _can_raise(self) -> bool:
        """Desk voice may start (ADR 0032): the desk is idle, nothing else is about to
        start, and dictation is off."""
        return (
            self.state.models_ready
            and self._waiting
            and self.session is None
            and self.state.phase is Phase.PASSIVE
            and self._phone_q.empty()
            and self._raise_q.empty()
            and not is_dictating(self.cfg.dictation_flag)
        )

    async def _reply_window(
        self,
        session: Any,
        raised: tuple[list[Item], RoutePolicy, asyncio.Future[str]],
        replied: asyncio.Event,
        first_reply: list[str],
    ) -> None:
        """Wait for the operator's reply. Silence: not heard, the session ends quietly and
        nothing heard was kept or sent. "Not now": snoozed. Anything else: heard."""
        items, _policy, fut = raised
        try:
            await asyncio.wait_for(replied.wait(), REPLY_WINDOW_SECS + GREETING_SECS)
        except TimeoutError:
            logger.info("desk voice: no reply; handing on")
            if not fut.done():
                fut.set_result("not_heard")
            await session.cancel()
            return
        if is_not_now(first_reply[0] if first_reply else ""):
            until = datetime.now(UTC) + timedelta(minutes=SNOOZE_MINUTES)
            if self.waiting is not None:
                for i in items:
                    self.waiting.items.snooze(i.key, until)
            if not fut.done():
                fut.set_result("delivered")
            return
        if not fut.done():
            fut.set_result("heard")

    def _at_desk(self) -> bool:
        """The operator is at the desk: a desk session ran lately, or the desktop app is
        connected to the event stream."""
        recent = (
            self._last_desk_session is not None
            and time.monotonic() - self._last_desk_session < AT_DESK_SECS
        )
        return recent or self.state.bus.subscribers > 0

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
        """The file, and Ghost's folder with it (ADR 0035): watches.json and hooks.json
        edited by hand apply too."""
        policy = AutonomyPolicy.load(self.cfg.policy_path)
        if self.gate is not None:
            self.gate.set_autonomy(policy)
        if self.routes is not None:
            self.routes.set_policy(policy.routes or RoutePolicy())
        if self.folder is not None:
            self.folder.refresh()  # the watches and hooks follow
        logger.info(f"policy reloaded from {policy.source}")
        return policy

    def apply_settings(self, values: dict[str, str | None]) -> list[str]:
        """Apply what can change without a restart; return those keys. The wake phrases
        and the near-miss threshold are picked up by rebuilding the passive listener; the
        crossing-streams toggles (ADR 0019) by the next session, which reads them."""
        applied: list[str] = []
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

    def waiting_view(self) -> WaitingView:
        """The phone's read of the waiting list; the fetch is the phone route's delivery."""
        assert self._deps is not None
        view = build_waiting_view(self._deps)
        self._phone_route.fetched()
        return view

    def acknowledge_item(self, key: str) -> bool:
        return self.waiting is not None and self.waiting.acknowledge(key)

    def snooze_item(self, key: str, minutes: int) -> bool:
        if self.waiting is None:
            return False
        return self.waiting.snooze(key, datetime.now(UTC) + timedelta(minutes=minutes))

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

    def agents(self) -> list[ConversationView]:
        """The agents a phone call can start with: Ghost first, retired ones left out."""
        if self.conversations is None:
            return []
        profiles = self.conversations.list(include_retired=False)
        return [conversation_view(self.conversations, p, self.state.conversation) for p in profiles]

    async def start_phone(self, connection: Any, conversation: str | None = None) -> Refusal | None:
        line = conversation or GHOST
        if line != GHOST:
            profile = self.conversations.get(line) if self.conversations else None
            if profile is None or profile.retired:
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

    # --- passwords (ADR 0036) --------------------------------------------------

    def _classifier(self, jev: JevClient | None) -> ConfirmationClassifier:
        if jev is None:
            return RegexClassifier()
        if self.passwords is not None:
            self.passwords.note_used(
                "OPENROUTER_API_KEY" if jev.provider == "openrouter" else "TYPESAFE_API_KEY"
            )
        return JevClassifier(
            jev,
            fallback=RegexClassifier(),
            threshold=self.cfg.jev_threshold,
            deadline=self.cfg.jev_deadline_secs,
            on_decision=debug.audio,
        )

    async def _wait_for_vault(self) -> None:
        """The keyring was locked at start: try again every few seconds until it opens.
        "Unlock your keyring" is on the waiting list meanwhile."""
        vault = self.passwords
        while vault is not None and not vault.is_open:
            try:
                await asyncio.wait_for(self._stop.wait(), VAULT_RETRY_SECS)
                return
            except TimeoutError:
                pass
            if not vault.is_open and await asyncio.to_thread(vault.open):
                await self.passwords_opened()

    async def passwords_opened(self) -> None:
        """The vault opened after the keyring was locked: Ghost's own settings are read
        again (the model connection, the voice), Jev joins when it has a key, and the apps
        that waited for a password connect. A watch's model decider follows at the next
        start."""
        logger.info("passwords: the vault is open")
        try:
            self.voice = VoiceSettings()  # type: ignore[call-arg]
        except Exception as e:  # noqa: BLE001 - names only; a value never reaches the log
            logger.error(f"passwords: settings could not be read again ({type(e).__name__})")
        try:
            jev = build_jev(GhostdSettings())
        except ValueError as e:
            logger.error(f"passwords: {e}")
            jev = None
        if jev is not None and self.gate is not None:
            self._jevs.append(jev)
            self.gate.set_classifier(self._classifier(jev))
        if self.apps is not None:
            for name in list(self.apps.failed):
                if await self.apps.revive(name):
                    self.state.publish_apps(changed=name)
        self._refresh_waiting_now()

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
        logger.info(ghost_build.describe(self.state.version, self.state.build))

        self.cfg.db_path.parent.mkdir(parents=True, exist_ok=True)
        store = Store(self.cfg.db_path)
        autonomy = AutonomyPolicy.load(self.cfg.policy_path)
        # ADR 0035: one folder, Ghost's. ADR 0034: the apps are Ghost's, connected now, and
        # every agent reaches them.
        folder = GhostFolder(self.cfg.config_dir)
        folder.ensure_layout()
        self.folder = folder
        for problem in folder.problems:
            logger.warning(f"folder: {problem}")
        apps_file = load_apps(self.cfg.config_dir / APPS_FILE)
        for problem in apps_file.problems:
            logger.warning(f"apps: {problem}")
        if self.passwords is None:
            self.passwords = open_passwords(self.cfg)
        passwords = self.passwords
        signins = SignIns(
            self.cfg.data_dir / "apps", port=self.cfg.signin_port, on_url=self._on_signin_url
        )
        self.signins = signins
        # ADR 0036: an app's ${NAME} is filled from the vault when it connects.
        apps = Apps(apps_file.apps, environ=passwords.environ(), auth_for=signins.auth_for)
        self.apps = apps
        await apps.connect()
        self.catalog = Catalog(self.cfg.catalog_path)
        # #35: skills.sh search and GitHub fetches for the Skills page, never for a session.
        self.skill_catalog = SkillCatalog(self.cfg.catalog_path.parent / "skill-previews")
        skills = Skills(self.cfg.skills_dir, self.cfg.home_skills_dir)  # ADR 0035
        self.skills = skills
        # What Ghost's own sessions may call: every connected app's tools, and the skills
        # (the index, read_skill, add and remove).
        tool_sources: list[ToolSource] = [*apps.sources(), skills.source()]
        jev = build_jev(self.cfg)
        gate = WriteGate(
            store,
            autonomy=autonomy,
            classifier=self._classifier(jev),
            on_decision=debug.audio,
        )
        for a in tool_sources:  # every app's changes go through the gate
            gate.register(a.name, a.operations)
        # And skills.install, which the desktop's catalog prepares and the model is not offered.
        gate.register(SKILLS, skills.gate_operations())
        self.gate = gate
        registered = {a.name for a in apps.sources()}

        def sync_apps() -> None:
            """An app was connected, disconnected or came back: the gate follows, and so
            does the next turn's list of tools."""
            now = {src.name: src for src in apps.sources()}
            for name in registered - set(now):
                gate.unregister(name)
            for name, src in now.items():
                gate.register(name, src.operations)
            registered.clear()
            registered.update(now)
            self.state.apps = sorted(now)
            self._refresh_waiting_now()

        apps.on_change.append(sync_apps)

        async def read_for(app: str, tool: str, args: dict[str, Any]) -> Any:
            """A read the daemon makes (a watch's look, a webhook's check, ADR 0027/0028):
            one of the app's read-only tools, unless the operator's permissions block it."""
            if gate.blocks_read(app, tool, args):
                raise ValueError(f"{app}.{tool} is blocked by the operator's permissions")
            return await apps.read_for(app, tool, args)

        conversations = Conversations(
            store,
            self.cfg.config_dir,
            default_voice=self.voice.tts_voice,
            default_speed=self.voice.tts_speed,
            retention=timedelta(days=self.cfg.turn_retention_days),
        )
        gate.register(CONVERSATIONS, conversations_source(conversations).operations)
        self.conversations = conversations
        rules = Rules(self.cfg.config_dir)  # ADR 0035: "add a rule", verbatim
        gate.register(RULES, rules.operations())
        self.rules = rules
        outputs = Outputs(self.cfg.config_dir)  # ADR 0033: what Ghost makes
        gate.register(OUTPUTS, outputs.operations())
        self.outputs = outputs
        # ADR 0029: what needs the operator, with the changes waiting for a yes (ADR 0030).
        waiting = WaitingList(
            Items(store),
            changes=gate.waiting,
            problems=lambda: ghost_problems(
                apps, folder, password_problems(self.passwords, self.cfg)
            ),
            on_change=lambda: self.state.set_waiting(waiting.count()),
        )
        self.waiting = waiting
        # ADR 0028: the watches, polled by the daemon; Jev and a model only where declared.
        watch_runner = WatchRunner(
            waiting.items,
            lambda w: read_for(w.app, w.read, dict(w.args)),
            jev=watch_decider(jev) if jev else None,
            model=None if self.voice.llm_problems() else model_decider(self.voice.llm_target()),
            on_change=lambda: self.state.set_waiting(waiting.count()),
        )
        self.watch_runner = watch_runner
        # ADR 0027: webhooks, signed and parsed by declaration, checked through read_for.
        hook_intake = HookIntake(
            waiting.items,
            store,
            read=read_for,
            environ=passwords.environ(),
            on_change=lambda: self.state.set_waiting(waiting.count()),
        )
        self.hook_intake = hook_intake
        # ADR 0031: how Ghost reaches the operator, by the operator's [routes].
        self.routes = RouteEngine(
            waiting.items,
            policy=autonomy.routes or RoutePolicy(),
            urgency_of=lambda item: declared_urgency(folder).get(item.source, "notify"),
            at_desk=self._at_desk,
            on_change=lambda: self.state.set_waiting(waiting.count()),
        )
        self.routes.add_route(DeskRoute(self._can_raise, self._raise_q))
        self.routes.add_route(DesktopRoute(waiting.acknowledge))
        self.routes.add_route(self._phone_route)

        def reach(p: Profile) -> list[ToolSource]:
            """What a session with any agent reaches (ADR 0034, ADR 0035): every connected
            app (the operator's permissions apply in the gate), the skills, the agents' own
            tools, the rules, the outputs and the waiting list."""
            out: list[ToolSource] = list(apps.sources())
            out.append(skills.source())
            out.append(conversations_source(conversations))
            out.append(rules_source(rules))
            out.append(outputs.source())
            out.append(waiting_source(waiting))
            return out

        self._reach = reach
        pruned = conversations.prune()
        if pruned:
            logger.info(f"conversations: pruned {pruned} summarized turns past retention")
        self.state.apps = sorted(registered)
        events = Events()
        api = await self._start_api(gate, store, apps.sources(), skills)
        await self._start_phone()
        waiting.refresh()
        self.state.set_waiting(waiting.count())
        refresher = asyncio.ensure_future(self._refresh_waiting(waiting))
        # ADR 0036: a locked keyring is waited for, not a failure.
        vault_waiter = None if passwords.is_open else asyncio.ensure_future(self._wait_for_vault())

        def follow_watches() -> None:
            """The folder was re-read: the declared watches and hooks again."""
            watched = list(folder.watches.values())
            for w in watched:
                if w.app not in apps.specs:
                    logger.warning(f"watch {w.name}: reads {w.app}, which isn't connected")
            watch_runner.set_watches(watched)
            for problem in hook_intake.set_hooks(list(folder.hooks.values())):
                logger.warning(f"hooks: {problem}")

        follow_watches()
        folder.on_refresh.append(follow_watches)
        for line in describe_watches(folder):
            logger.info(line)
        hooks_listener = await self._start_hooks(hook_intake)
        if hooks_listener is not None and self._deps is not None:
            self._deps.hooks_listening = list(hooks_listener.addresses)
        logger.info(f"autonomy policy: {autonomy.source}")
        logger.info(describe_routes(self.routes.policy))

        async def on_event(event: Any) -> None:
            if event.kind == "quiet" and event.data.get("until") is not None:
                self.routes.quiet_until(event.data["until"])
                logger.info(f"routes: quiet until {event.data['until']:%H:%M}")

        events.add(on_event)
        logger.info(
            f"llm: {self.voice.llm_target().describe()}, thinking {self.voice.llm_thinking}"
        )
        for problem in self.voice.llm_problems():
            logger.error(f"llm: {problem}")
        logger.info(f"tts: {describe_tts(self.voice)}")
        for line in describe_apps(apps, gate):
            logger.info(line)
        logger.info(describe_skills(skills, set(apps.specs)))
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
            refresher.cancel()
            if vault_waiter is not None:
                vault_waiter.cancel()
            await watch_runner.close()
            for route in self.routes.routes.values():
                if hasattr(route, "close"):
                    await route.close()
            if hooks_listener is not None:
                await hooks_listener.stop()
            wake.close()
            ducker.restore()
            if self.phone_listener:
                await self.phone_listener.stop()
            if api:
                await api.stop()
            await signins.close()
            await apps.close()
            if jev:
                await jev.aclose()
            for late in self._jevs:
                await late.aclose()
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
            raise_task = asyncio.ensure_future(self._raise_q.get())
            self._waiting = True
            try:
                await asyncio.wait(
                    {wake_task, stop_task, rearm_task, phone_task, raise_task},
                    return_when=asyncio.FIRST_COMPLETED,
                )
            finally:
                self._waiting = False
            stop_task.cancel()
            rearm_task.cancel()
            raised = raise_task.result() if raise_task.done() else None
            raise_task.cancel()
            if raised is not None and (phone_task.done() or wake_task.done()):
                raised[2].set_result("unavailable")  # the operator came first
                raised = None
            phone = phone_task.result() if phone_task.done() else None
            phone_task.cancel()
            if phone is None and not self._phone_q.empty():  # accepted in the same tick
                phone = self._phone_q.get_nowait()
            phone_conn, phone_line = phone if phone is not None else (None, GHOST)
            # Decided before the cancel below, which makes a pending task done too.
            woke = wake_task.done()
            if raised is not None and (
                self._stop.is_set() or is_dictating(self.cfg.dictation_flag)
            ):
                raised[2].set_result("unavailable")
                raised = None
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
            if raised is not None:  # ADR 0032: Ghost greets at the desk and waits
                await self._conversation(
                    models, tool_sources, gate, events, ducker, None, raised=raised
                )
                continue
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
        skills: Skills | None = None,
    ) -> ApiServer | None:
        socket_path = self.cfg.api_socket or default_socket_path()
        deps = Deps(
            state=self.state,
            control=self,
            gate=gate,
            store=store,
            sources=sources,
            apps=self.apps,
            catalog=self.catalog,
            skill_catalog=self.skill_catalog,
            signins=self.signins,
            skills=skills,
            folder=self.folder,
            rules=self.rules,
            outputs=self.outputs,
            waiting=self.waiting,
            watches=self.watch_runner,
            hooks=self.hook_intake,
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
            passwords=self.passwords,
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

    async def _start_hooks(self, intake: HookIntake) -> HooksListener | None:
        """The webhook listener, when GHOST_HOOKS_LISTEN is set (ADR 0027)."""
        if not self.cfg.hooks_listen.strip():
            return None
        try:
            listener = HooksListener(self.cfg.hooks_listen, intake)
            await listener.start()
        except (OSError, ValueError, RuntimeError) as e:
            logger.error(f"hooks: listener not started on {self.cfg.hooks_listen!r}: {e}")
            return None
        return listener

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
        raised: tuple[list[Item], RoutePolicy, asyncio.Future[str]] | None = None,
    ) -> None:
        """One session. With ``phone`` (a negotiated SmallWebRTCConnection) the audio is
        the phone's: no ducking, no desk chime, the thinking sound in its track, and the
        session's events go to it over RTVI (ADR 0016). ``start_in`` is the thread the
        session opens in: Ghost, or the one the phone called directly. With ``raised``
        Ghost started it (ADR 0032): it greets with the operator's phrase, the items wait
        in the context, and a reply within the window is the acknowledgment; silence ends
        it with nothing kept."""
        origin = "phone" if phone is not None else "desk"
        if self.passwords is not None:  # "last used" on the Passwords page (ADR 0036)
            self.passwords.note_used(*in_use(self.voice))
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
            await heard(role, text)

        async def heard(role: str, text: str) -> None:
            if digest is not None:
                digest.on_turn(role, text)  # the greeting went out; the operator answered
            if role == "user":
                self._refresh_pending()
                if raised is not None and not replied.is_set():
                    first_reply.append(text)
                    replied.set()

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

        # ADR 0031: a bare wake in Ghost opens with what is waiting, said before anything
        # else (the digest); a request in the same breath is answered instead. The greeting
        # going out delivers them; the operator's answer acknowledges only the titles said
        # (ADR 0029), so a voice that failed or "and two more" leaves them alerting.
        waiting_now = (
            self.routes.digest()
            if self.routes is not None
            and start_in == GHOST
            and not first_utterance
            and raised is None
            else []
        )
        digest = (
            SpokenDigest(self.routes, waiting_now)
            if waiting_now and self.routes is not None
            else None
        )
        replied = asyncio.Event()
        first_reply: list[str] = []
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
            **(
                {"greeting": digest.text, "context_note": digest_note()}
                if digest is not None
                else {}
            ),
            **(
                {"greeting": raised[1].greeting, "context_note": context_note(raised[0])}
                if raised is not None
                else {}
            ),
        )
        self.session = session
        self._heard = heard
        if origin == "desk":
            self._last_desk_session = time.monotonic()
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
        if raised is not None:
            self.state.set_phase(Phase.ASKING, session.session_id, origin=origin)
            helpers.append(
                asyncio.ensure_future(self._reply_window(session, raised, replied, first_reply))
            )
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
            if raised is not None and not raised[2].done():
                raised[2].set_result("not_heard")
            if events_q is not None:
                self.state.bus.unsubscribe(events_q)
            if phone is not None:
                await _close_peer(phone)  # the session ending closes the peer
                self._phone_end.clear()
                _PHONE_TEARDOWN["on"] = False
            elif self.cfg.chime:
                await asyncio.to_thread(chime, False, self.voice.output_device)
            ducker.restore()
            # A change the session was still waiting on stays on the waiting list (ADR 0030).
            if gate.set_aside(session.session_id, "session ended") is not None:
                self._refresh_waiting_now()
            self.session = None
            self._heard = None
            self._origin = None
            if origin == "desk":
                self._last_desk_session = time.monotonic()
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


def describe_apps(apps: Apps, gate: WriteGate | None = None) -> list[str]:
    """Each app as connected: its reads and changes, and the changes Ghost's own
    permissions block or ask about (ADR 0034)."""
    lines = []
    policy = gate.autonomy if gate is not None else AutonomyPolicy()
    for name, spec in apps.specs.items():
        live = apps.apps.get(name)
        if live is None or not live.connected:
            lines.append(f"  app {name} ({spec.transport}): not connected: {apps.problem(name)}")
            continue
        reads = len(live.reads())
        changes = live.changes()
        careful = []
        for t in changes:
            level = policy.resolve(name, t.name)
            if level.value in ("deny", "confirm"):
                careful.append(f"{t.name}={'block' if level.value == 'deny' else 'ask'}")
        line = f"  app {name} ({spec.transport}): {reads} reads, {len(changes)} changes"
        if careful:
            line += f"; {', '.join(careful)}"
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


def check_passwords(vault: Passwords, cfg: GhostdSettings) -> list[str]:
    """``ghostd check`` for the vault (ADR 0036). Passwords still in plain files are said,
    not a problem: a build is deployed before its import."""
    print(f"passwords: {len(vault)} in {vault.path} ({vault.state})")
    out = []
    if not vault.is_open:
        out.append(f"passwords: {vault.problem}")
    for key, title in password_problems(vault, cfg):
        if key == "passwords:import":
            print(f"passwords: still in plain files; run `ghostd passwords import` ({title})")
    return out


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
    # No log line carries a password: each message is checked against the vault's values
    # (ADR 0036). diagnose=False: a traceback never prints its frames' variables (a
    # webhook's raw body, a password); the journal gets the stack and the message only.
    logger.configure(patcher=redacting)  # type: ignore[arg-type]
    logger.add(
        sys.stderr,
        level="INFO",
        format="{time:HH:mm:ss.SSS} {level:<7} {message}",
        filter=quiet_phone_teardown,
        diagnose=False,
    )

    if cmd == "run":
        # The vault first (ADR 0036): Ghost's settings read its passwords from this
        # process's environment, so they are read after it.
        passwords = open_passwords(GhostdSettings())
        cfg = GhostdSettings()
        voice = VoiceSettings()  # type: ignore[call-arg]
        if voice.log_level.upper() != "INFO":
            logger.remove()
            logger.add(
                sys.stderr,
                level=voice.log_level.upper(),
                format="{time:HH:mm:ss.SSS} {level:<7} {name}: {message}",
                filter=quiet_phone_teardown,
                diagnose=False,
            )
        debug.enable(voice.debug_audio)
        asyncio.run(Daemon(cfg, voice, passwords).run())
    elif cmd in {"wake", "say", "sleep", "status"}:
        sys.exit(_client_command(cmd, argv[1:]))
    elif cmd == "latency":
        sys.exit(latency_command(argv[1:]))
    elif cmd == "perf":
        from ghostd.perf.cli import main as perf_main

        open_passwords(GhostdSettings())  # the model and Jev keys (ADR 0036)
        sys.exit(perf_main(argv[1:]))
    elif cmd == "passwords":
        sys.exit(passwords_cli(argv[1:], GhostdSettings()))
    elif cmd == "restore-volumes":
        restore_from_state(runtime_dir() / DUCK_STATE)
    elif cmd == "devices":
        import sounddevice as sd

        print(sd.query_devices())
    elif cmd in {"version", "--version"}:
        print(ghost_build.describe(ghost_build.version(), ghost_build.detect()))
    elif cmd == "check":
        vault = open_passwords(GhostdSettings())
        cfg = GhostdSettings()
        print(f"version: {ghost_build.describe(ghost_build.version(), ghost_build.detect())}")
        problems = check_passwords(vault, cfg)
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
        problems += check_folder(cfg.config_dir)
        apps_file = load_apps(cfg.config_dir / APPS_FILE)
        for problem in apps_file.problems:
            print(f"  problem: {problem}")
            problems.append(problem)
        if apps_file.apps:

            async def probe() -> list[str]:
                signins = SignIns(cfg.data_dir / "apps", port=cfg.signin_port)
                apps = Apps(apps_file.apps, environ=vault.environ(), auth_for=signins.auth_for)
                await apps.connect()
                gate = WriteGate(Store(), autonomy=autonomy)
                try:
                    return describe_apps(apps, gate)
                finally:
                    await apps.close()

            print("apps:")
            for line in asyncio.run(probe()):
                print(line)
                if "not connected" in line:
                    problems.append(line.strip())
        skills = Skills(cfg.skills_dir, cfg.home_skills_dir)
        print(describe_skills(skills, set(apps_file.apps)))
        problems += [f"skill {s.name}: {s.problem}" for s in skills.all() if s.problem]
        try:
            jev = build_jev(cfg)
            how = f"jev via {jev.provider} (regex fallback)" if jev else "regex"
            print(f"confirmation: {how}")
        except ValueError as e:
            problems.append(str(e))
        print("\n".join(problems) if problems else "config ok")
        sys.exit(1 if problems else 0)
    elif cmd == "push":
        sys.exit(push_cli(argv[1:]))
    else:
        print(
            "usage: ghostd [run|wake|say <text>|sleep|status|latency|perf|restore-volumes|"
            "devices|check|version|passwords|push <hook> <event> [json]]"
            f"  (got {cmd!r})"
        )
        sys.exit(2)


def push_cli(args: list[str]) -> int:
    """Send a signed delivery to the local webhook listener (ADR 0027), for a local
    sender: ``ghostd push jobs done '{"job": "backup", "ok": false}'`` at the end of a
    long job. The hook must use ``hmac``; its secret comes from the environment."""
    import time
    import uuid

    import httpx
    from ghost_core.hooks import H_EVENT, H_ID, H_SIG, H_TIME, sign_hmac

    if len(args) < 2 or "/" in args[0]:
        print("usage: ghostd push <hook> <event> [json]", file=sys.stderr)
        return 2
    cfg = GhostdSettings()
    open_passwords(cfg)  # the hook's password (ADR 0036)
    hook = GhostFolder(cfg.config_dir).hooks.get(args[0])
    if hook is None or hook.verify != "hmac":
        print(f"no hmac hook {args[0]} is declared", file=sys.stderr)
        return 2
    secret = os.environ.get(hook.secret_name(), "")
    if not secret:
        print(f"{hook.secret_name()} is not set", file=sys.stderr)
        return 2
    if not cfg.hooks_listen.strip():
        print("GHOST_HOOKS_LISTEN is not set", file=sys.stderr)
        return 2
    body = (args[2] if len(args) > 2 else "{}").encode()
    ts, delivery = str(int(time.time())), uuid.uuid4().hex
    address = cfg.hooks_listen.split(",")[0].strip()
    try:
        r = httpx.post(
            f"http://{address}{hook.path}",
            content=body,
            headers={
                "content-type": "application/json",
                H_TIME: ts,
                H_ID: delivery,
                H_EVENT: args[1],
                H_SIG: sign_hmac(secret, ts, delivery, body),
            },
            timeout=10,
        )
    except httpx.HTTPError as e:
        print(
            f"could not reach the hooks listener on {address} ({type(e).__name__})", file=sys.stderr
        )
        return 1
    print("accepted" if r.status_code < 400 else f"refused ({r.status_code})")
    return 0 if r.status_code < 400 else 1


if __name__ == "__main__":
    cli()
