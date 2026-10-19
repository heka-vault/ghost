"""Run scripted conversations through ``build_session`` and collect their timelines.

Each run gets a fresh temporary state directory: its own SQLite, conversations, mock
GitHub and latency file, so runs are independent and the operator's state is never
read or written. The models (Parakeet, Kokoro, Smart Turn) are loaded once per process.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import shutil
import tempfile
import time
from dataclasses import dataclass, field
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any

from ghost_core import (
    AutonomyPolicy,
    ConfirmationClassifier,
    Events,
    GhostContext,
    RegexClassifier,
    Store,
    WriteGate,
    discover,
    load,
)
from ghost_core.conversations import ABILITY as CONVERSATIONS
from ghost_core.conversations import GHOST, Conversations
from ghost_core.jev import JevClassifier, JevClient
from ghost_voice import VoiceSettings, build_session, debug
from ghost_voice import llm as llm_module
from ghost_voice import prompt as prompt_module
from ghost_voice.config import LLM_ENDPOINTS
from ghost_voice.llm import GhostAnthropicLLMService
from ghost_voice.pipeline import TTS_SAMPLE_RATE, Models, conversations_ability
from loguru import logger
from pipecat.transports.base_transport import TransportParams

from ghostd.config import GhostdSettings
from ghostd.perf.audio import PAUSE_MARK, Mouth, ScriptedTransport, Voice
from ghostd.perf.scenarios import Line, Scenario

# What the harness can turn off to measure a change, each one applied only while a run
# builds and runs its session (the process runs one session at a time):
#   tts_clauses     Kokoro gets whole sentences again (GHOST_TTS_CLAUSES=false)
#   issue_comments  get_issue without the latest comments (GHOST_GITHUB_ISSUES_ISSUE_COMMENTS=0)
#   prompt_lines    the system prompt without the lead-short and parallel-lookup lines
#   system_cache    no cache marker on the system prompt
#   shared_client   a new AsyncAnthropic per session, as before
ABLATIONS = ("tts_clauses", "issue_comments", "prompt_lines", "system_cache", "shared_client")
# A plausible record of earlier sessions, for --history: generic, like the seed issues.
HISTORY = (
    ("user", "What needs my sign-off?"),
    ("call", "list_awaiting_signoff()"),
    ("assistant", "Three: the session cache choice, dark mode, and rotating the webhook."),
    ("user", "What's the cache one about?"),
    ("call", "get_issue(number=142, repo=ghost-mock/app)"),
    ("assistant", "Redis versus Postgres for the session cache; it blocks two others."),
    ("user", "Leave it for now. Anything else urgent?"),
    ("assistant", "The webhook rotation is a security fix, so that one."),
)


def thinking_for(model: str, requested: str | None) -> str | None:
    """Haiku rejects ``between_tools`` (HTTP 400 on both endpoints, 2026-10-19): compare it
    with thinking off unless a mode was asked for explicitly."""
    if requested is None and "haiku" in model:
        return "disabled"
    return requested


@dataclass
class RunConfig:
    model: str = "claude-sonnet-5-5"
    endpoint: str = "proxy"  # proxy: ANTHROPIC_BASE_URL; openrouter: its Anthropic API
    thinking: str | None = None  # None: Ghost's default (GHOST_LLM_THINKING)
    transport: str = "desk"  # desk: scripted devices; webrtc: aiortc peer on loopback
    history: int = 0  # earlier turns seeded into the ghost conversation's record
    jev: bool = True
    overrides: dict[str, str] = field(default_factory=dict)  # VoiceSettings fields
    ablate: frozenset[str] = frozenset()  # names from ABLATIONS

    @property
    def label(self) -> str:
        parts = [self.model, self.endpoint]
        if self.ablate:
            parts.append(
                "before" if set(ABLATIONS) <= self.ablate else "-" + "-".join(sorted(self.ablate))
            )
        thinking = thinking_for(self.model, self.thinking)
        if thinking:
            parts.append(f"thinking={thinking}")
        if self.transport != "desk":
            parts.append(self.transport)
        if self.history:
            parts.append(f"history={self.history}")
        if not self.jev:
            parts.append("regex")
        parts += [f"{k}={v}" for k, v in sorted(self.overrides.items())]
        return " ".join(parts)


@dataclass
class LineResult:
    text: str
    records: list[dict[str, Any]]
    heard: float | None  # word match of what STT heard against the script, 0..1
    problems: list[str]
    audible: float | None = None  # last frame of the line -> Ghost loud at the speaker/phone


@dataclass
class RunResult:
    scenario: str
    config: str
    run: int
    lines: list[LineResult]
    opening: dict[str, Any] | None
    records: list[dict[str, Any]]
    problems: list[str]
    wall: float
    wake_to_voice: float | None

    def to_json(self) -> dict[str, Any]:
        return {
            "scenario": self.scenario,
            "config": self.config,
            "run": self.run,
            "wall": round(self.wall, 2),
            "wake_to_voice": self.wake_to_voice,
            "problems": self.problems,
            "heard": [ln.heard for ln in self.lines],
            "audible": [ln.audible for ln in self.lines],
            "records": self.records,
        }


def endpoint_host(cfg: RunConfig) -> str:
    """Where the model calls go, without any credential."""
    return make_settings(cfg).llm_target().describe()


def parse_ablations(spec: str) -> list[frozenset[str]]:
    """``none,all,tts_clauses,issue_comments+prompt_lines`` -> one set per variant."""
    variants = []
    for part in [p.strip() for p in spec.split(",") if p.strip()] or ["none"]:
        if part == "none":
            variants.append(frozenset())
        elif part == "all":
            variants.append(frozenset(ABLATIONS))
        else:
            names = frozenset(n.strip() for n in part.split("+"))
            unknown = names - set(ABLATIONS)
            if unknown:
                raise SystemExit(f"unknown ablation {sorted(unknown)}; known: {ABLATIONS}")
            variants.append(names)
    return variants


def make_settings(cfg: RunConfig) -> VoiceSettings:
    base = VoiceSettings()  # type: ignore[call-arg]
    update: dict[str, Any] = {"input_device": None, "output_device": None, "llm_model": cfg.model}
    thinking = thinking_for(cfg.model, cfg.thinking)
    if thinking:
        update["llm_thinking"] = thinking
    if cfg.endpoint not in LLM_ENDPOINTS:
        raise SystemExit(f"unknown endpoint {cfg.endpoint!r} ({', '.join(LLM_ENDPOINTS)})")
    update["llm_endpoint"] = cfg.endpoint
    if "tts_clauses" in cfg.ablate:
        update["tts_clauses"] = False
    for k, v in cfg.overrides.items():
        if k not in VoiceSettings.model_fields:
            raise SystemExit(f"unknown setting {k!r}")
        current = getattr(base, k)
        if isinstance(current, bool):
            update[k] = v.lower() in {"1", "true", "yes", "on"}
        elif isinstance(current, (int, float)):
            update[k] = type(current)(v)
        else:
            update[k] = v
    settings = base.model_copy(update=update)
    problems = settings.llm_problems()
    if problems:
        raise SystemExit("; ".join(problems))
    return settings


class Ablated:
    """Applies a run's ablations that are not settings, and puts everything back."""

    def __init__(self, ablate: frozenset[str]) -> None:
        self._ablate = ablate
        self._undo: list[Any] = []

    def __enter__(self) -> Ablated:
        if "issue_comments" in self._ablate:
            old = os.environ.get("GHOST_GITHUB_ISSUES_ISSUE_COMMENTS")
            os.environ["GHOST_GITHUB_ISSUES_ISSUE_COMMENTS"] = "0"
            self._undo.append(lambda: _restore_env("GHOST_GITHUB_ISSUES_ISSUE_COMMENTS", old))
        if "prompt_lines" in self._ablate:
            lead, parallel = prompt_module.LEAD_RULE, prompt_module.PARALLEL_RULE
            prompt_module.LEAD_RULE = prompt_module.PARALLEL_RULE = ""

            def undo_prompt() -> None:
                prompt_module.LEAD_RULE, prompt_module.PARALLEL_RULE = lead, parallel

            self._undo.append(undo_prompt)
        if "system_cache" in self._ablate:
            GhostAnthropicLLMService.cache_system = False
            self._undo.append(lambda: setattr(GhostAnthropicLLMService, "cache_system", True))
        if "shared_client" in self._ablate:
            llm_module.SHARE_CLIENT = False
            self._undo.append(lambda: setattr(llm_module, "SHARE_CLIENT", True))
        return self

    def __exit__(self, *exc: object) -> None:
        for undo in reversed(self._undo):
            undo()


def _restore_env(key: str, value: str | None) -> None:
    if value is None:
        os.environ.pop(key, None)
    else:
        os.environ[key] = value


def _words(text: str) -> list[str]:
    return re.findall(r"[a-z0-9']+", text.lower().replace(PAUSE_MARK.strip(), " "))


def heard_ratio(expected: str, heard: str) -> float:
    """How much of the scripted line STT got, word by word (1.0 is every word)."""
    a, b = _words(expected), _words(heard)
    if not a:
        return 1.0
    m = SequenceMatcher(a=a, b=b, autojunk=False)
    return sum(x.size for x in m.get_matching_blocks()) / max(len(a), len(b))


class Bench:
    """Models and the utterance renderer, shared by every run in the process."""

    def __init__(self, out: Path, cache: Path, *, show_text: bool = False) -> None:
        self.out = out
        self.cache = cache
        self.show_text = show_text
        base = VoiceSettings()  # type: ignore[call-arg]
        t = time.monotonic()
        self.models = Models.load(base)
        self.load_secs = time.monotonic() - t
        self.voice = Voice(self.models.tts.kokoro, cache / "utterances")
        self.gdsettings = GhostdSettings()

    # --- one run -----------------------------------------------------------------

    async def run(self, scenario: Scenario, cfg: RunConfig, index: int) -> RunResult:
        with Ablated(cfg.ablate):
            return await self._run(scenario, cfg, index)

    async def _run(self, scenario: Scenario, cfg: RunConfig, index: int) -> RunResult:
        settings = make_settings(cfg)
        utterances = [self.voice.render(line.text) for line in scenario.lines]
        tmp = Path(tempfile.mkdtemp(prefix="ghost-perf-"))
        problems: list[str] = []
        records: list[dict[str, Any]] = []
        got = asyncio.Event()
        heard: list[str] = []

        def sink(rec: dict[str, Any]) -> None:
            records.append(rec)
            got.set()

        async def on_turn(role: str, text: str) -> None:
            if role == "user":
                heard.append(text)  # in memory only, scored against the script
            if self.show_text:
                print(f"    {role}: {text}")

        env = await self._env(tmp, settings, cfg)
        mouth = Mouth()
        connected = asyncio.Event()
        peer: Any = None
        built: list[Any] = []
        if cfg.transport == "webrtc":
            from ghostd.perf.webrtc import LoopbackPhone

            peer = LoopbackPhone(mouth)
            transport_factory = await peer.transport_factory(connected)
            ear: Any = peer
        else:

            def transport_factory(mixer: Any) -> ScriptedTransport:
                t = ScriptedTransport(
                    TransportParams(
                        audio_in_enabled=True,
                        audio_out_enabled=True,
                        audio_out_sample_rate=TTS_SAMPLE_RATE,
                        audio_out_mixer=mixer,
                    ),
                    mouth,
                )
                built.append(t)
                return t

            ear = None

        began = time.monotonic()
        session = build_session(
            settings,
            models=self.models,
            abilities=env["abilities"],
            gate=env["gate"],
            events=Events(),
            transport_factory=transport_factory,
            on_turn=on_turn,
            conversations=env["conversations"],
            start_in=GHOST,
            origin="phone" if cfg.transport == "webrtc" else "desk",
            latency_log=self.out / "latency.jsonl",
            on_latency=sink,
        )
        await session.start(open=peer is None)  # type: ignore[attr-defined]
        if ear is None:
            ear = built[0]
        run_task = asyncio.ensure_future(session.run())
        lines: list[LineResult] = []
        wake_to_voice = None
        try:
            if peer is not None:
                await asyncio.wait_for(connected.wait(), 20)
                await session.open()  # type: ignore[attr-defined]
            opening = await self._wait(records, got, lambda r: r["kind"] == "opening", 25)
            if opening is None:
                problems.append("no greeting")
            else:
                wake_to_voice = round(
                    (opening.get("build") or 0.0) + (opening.get("reply") or 0.0), 3
                )
            for line, u in zip(scenario.lines, utterances, strict=True):
                await asyncio.sleep(0.8)  # the operator heard the end and starts talking
                n0, h0 = len(records), len(heard)
                done = asyncio.ensure_future(mouth.say(u).wait())
                # A goodbye can end the session before the line's last frames are sent.
                await asyncio.wait({done, run_task}, return_when=asyncio.FIRST_COMPLETED)
                done.cancel()
                spoke_at = time.monotonic()
                if line.ends:
                    try:
                        await asyncio.wait_for(asyncio.shield(run_task), 30)
                    except TimeoutError:
                        problems.append(f"line {len(lines) + 1}: session did not end")
                else:
                    rec = await self._wait(
                        records, got, lambda r: r["kind"] == "voice" and r["spoke"], 45, n0
                    )
                    if rec is None:
                        problems.append(f"line {len(lines) + 1}: no spoken reply")
                    await asyncio.sleep(0.3)  # let usage and late records land
                said = " ".join(heard[h0:])
                result = LineResult(
                    text=line.text,
                    records=records[n0:],
                    heard=heard_ratio(line.text, said) if said else None,
                    problems=check_line(line, records[n0:], ended=run_task.done()),
                    audible=_r3(ear.loud_after(spoke_at)),
                )
                problems += [f"line {len(lines) + 1}: {p}" for p in result.problems]
                lines.append(result)
                if run_task.done():
                    break
            if scenario.check:
                issue_problem = scenario.check(_mock_issues(tmp))
                if issue_problem:
                    problems.append(issue_problem)
        except Exception as e:  # noqa: BLE001 - report it, keep the batch going
            logger.exception("perf: run failed")
            problems.append(f"run failed: {type(e).__name__}")
        finally:
            if not run_task.done():
                await session.end()
                try:
                    await asyncio.wait_for(asyncio.shield(run_task), 20)
                except TimeoutError:
                    await session.cancel()
                    await asyncio.gather(run_task, return_exceptions=True)
            if peer is not None:
                await peer.close()
            await self._close_env(env)
            shutil.rmtree(tmp, ignore_errors=True)
        return RunResult(
            scenario=scenario.name,
            config=cfg.label,
            run=index,
            lines=lines,
            opening=next((r for r in records if r["kind"] == "opening"), None),
            records=records,
            problems=problems,
            wall=time.monotonic() - began,
            wake_to_voice=wake_to_voice,
        )

    async def _wait(
        self,
        records: list[dict[str, Any]],
        got: asyncio.Event,
        want,
        within: float,
        start: int = 0,
    ) -> dict[str, Any] | None:
        deadline = time.monotonic() + within
        while True:
            for r in records[start:]:
                if want(r):
                    return r
            left = deadline - time.monotonic()
            if left <= 0:
                return None
            got.clear()
            try:
                await asyncio.wait_for(got.wait(), left)
            except TimeoutError:
                return None

    # --- an isolated Ghost -----------------------------------------------------------

    async def _env(self, tmp: Path, settings: VoiceSettings, cfg: RunConfig) -> dict[str, Any]:
        os.environ["GHOST_GITHUB_MOCK"] = "1"
        os.environ.pop("GHOST_REPOS", None)  # the mock's own repositories
        store = Store(tmp / "ghost.db")
        autonomy = AutonomyPolicy.load(self.gdsettings.infra_dir / "policy.example.toml")

        def make_context(name: str) -> GhostContext:
            return GhostContext.build(
                name,
                data_root=tmp / "data",
                cache_root=tmp / "cache",
                store=store,
                env=os.environ,
                debug_audio=debug.audio,
            )

        abilities = await load(
            ["github_issues"],
            discovered=discover(tmp / "abilities"),
            make_context=make_context,
            passive_allowed=autonomy.passive,
        )
        jev = None
        if cfg.jev:
            jev = JevClient.from_env(
                "auto",
                {
                    "TYPESAFE_API_KEY": os.environ.get("TYPESAFE_API_KEY", ""),
                    "OPENROUTER_API_KEY": os.environ.get("OPENROUTER_API_KEY", ""),
                },
                timeout=max(self.gdsettings.jev_deadline_secs, 0.1),
            )
        classifier: ConfirmationClassifier = (
            JevClassifier(
                jev,
                fallback=RegexClassifier(),
                threshold=self.gdsettings.jev_threshold,
                deadline=self.gdsettings.jev_deadline_secs,
            )
            if jev
            else RegexClassifier()
        )
        gate = WriteGate(store, autonomy=autonomy, classifier=classifier)
        for a in abilities:
            gate.register(a.name, a.contributions.operations)
        conversations = Conversations(
            store,
            tmp / "conversations",
            default_voice=settings.tts_voice,
            default_speed=settings.tts_speed,
        )
        gate.register(CONVERSATIONS, conversations_ability(conversations).contributions.operations)
        for i in range(cfg.history):
            role, text = HISTORY[i % len(HISTORY)]
            conversations.record(GHOST, "earlier", role, text)
        return {
            "store": store,
            "abilities": abilities,
            "gate": gate,
            "conversations": conversations,
            "jev": jev,
        }

    async def _close_env(self, env: dict[str, Any]) -> None:
        for a in env["abilities"]:
            await a.instance.aclose()
        if env["jev"]:
            await env["jev"].aclose()
        env["store"].close()


def _r3(x: float | None) -> float | None:
    return None if x is None else round(x, 3)


def _mock_issues(tmp: Path) -> dict[str, Any]:
    path = tmp / "data" / "github_issues" / "mock-github.json"
    if not path.exists():
        return {}
    return json.loads(path.read_text()).get("issues", {})


def tools_used(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [t for r in records for t in ((r.get("tools") or {}).get("calls") or [])]


def check_line(line: Line, records: list[dict[str, Any]], *, ended: bool) -> list[str]:
    out = []
    used = tools_used(records)
    names = {t["name"] for t in used}
    missing = line.tools - names
    if missing:
        out.append(f"expected {sorted(missing)}, got {sorted(names) or 'no tools'}")
    if line.status and line.status not in {t.get("status") for t in used}:
        got = sorted({str(t.get("status")) for t in used if t["name"] == "change"})
        out.append(f"expected status {line.status}, got {got or 'none'}")
    if line.intercepted and not any(r.get("intercepted") for r in records):
        out.append("expected the quick intents to take it")
    if line.ends and not ended:
        out.append("expected the session to end")
    if line.ends and not any(r.get("spoke") for r in records):
        out.append("hung up without a spoken goodbye")
    return out
