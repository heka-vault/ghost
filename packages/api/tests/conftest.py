from __future__ import annotations

from pathlib import Path

import pytest
from api_fakes import Doc, FakeWriter, Tag
from fastapi.testclient import TestClient
from ghost_api import DaemonState, Deps, Phase, create_app
from ghost_core import (
    Ability,
    AutonomyPolicy,
    ChangeSet,
    Contributions,
    LoadedAbility,
    Operation,
    Prepared,
    ReadTool,
    SimpleInstance,
    Store,
    WriteGate,
)


class FakeControl:
    """Records calls; a real gate sits behind confirm/cancel."""

    def __init__(self, state: DaemonState, gate: WriteGate, policy_path: Path):
        self.state = state
        self.gate = gate
        self.policy_path = policy_path
        self.calls: list[tuple] = []
        self.discovered = {}

    async def wake(self, first_utterance):
        self.calls.append(("wake", first_utterance))
        if self.state.phase is not Phase.PASSIVE:
            return False
        self.state.set_phase(Phase.LISTENING, "sess1")
        return True

    async def sleep(self):
        self.calls.append(("sleep",))
        if self.state.session_id is None:
            return False
        self.state.set_phase(Phase.PASSIVE)
        return True

    async def say(self, text):
        self.calls.append(("say", text))
        if self.state.phase is Phase.STARTING:
            return False
        if self.state.session_id is None:
            await self.wake(text)
        self.state.add_turn("user", text, via="text")
        return True

    async def confirm_pending(self):
        if not self.state.session_id:
            return None
        out = await self.gate.confirm_pending(self.state.session_id, source="ui")
        if out:
            self.state.set_pending(None)
        return out

    async def cancel_pending(self):
        if not self.state.session_id:
            return False
        p = self.gate.cancel(self.state.session_id, "ui")
        if p:
            self.state.set_pending(None)
        return p is not None

    def apply_settings(self, values):
        self.applied = [k for k in values if k.startswith("GHOST_WAKE_")]
        return list(self.applied)

    def reload_policy(self):
        policy = AutonomyPolicy.load(self.policy_path)
        self.gate.set_autonomy(policy)
        self.calls.append(("reload_policy",))
        return policy

    def discovered_abilities(self):
        return self.discovered


def make_ability(writer) -> LoadedAbility:
    async def echo(args):
        return {"echo": args}

    async def prepare(args):
        return Prepared(Doc(name=args["name"]), ChangeSet(steps=(Tag(tag="flagged"),)))

    ops = [
        Operation(
            "flag",
            "Flag it.",
            {"name": {"type": "string"}},
            ["name"],
            prepare=prepare,
            writer=writer,
            risk="low",
        ),
        Operation(
            "nuke",
            "Dangerous.",
            {"name": {"type": "string"}},
            ["name"],
            prepare=prepare,
            writer=writer,
            risk="high",
        ),
    ]
    c = Contributions(
        prompt="P", read_tools=[ReadTool("echo", "Echo.", {}, [], echo)], operations=ops
    )

    async def setup(ctx, s):
        return SimpleInstance(c)

    ab = Ability("fake", "1.2.3", "A fake ability", setup)
    return LoadedAbility(ab, None, SimpleInstance(c), c)


@pytest.fixture
def world(tmp_path: Path):
    store = Store()
    writer = FakeWriter()
    loaded = make_ability(writer)
    policy_path = tmp_path / "policy.toml"
    gate = WriteGate(store, autonomy=AutonomyPolicy.load(policy_path))
    gate.register("fake", loaded.contributions.operations)
    state = DaemonState(version="test", wake_mode="file")
    control = FakeControl(state, gate, policy_path)
    control.discovered = {
        "fake": loaded.ability,
        "other": Ability("other", "0.1", "not enabled", loaded.ability.setup),
    }
    infra = tmp_path / "infra" / "pipewire"
    infra.mkdir(parents=True)
    (infra / "install.sh").write_text("#!/bin/sh\necho installed $1\n")
    (infra / "install.sh").chmod(0o755)
    deps = Deps(
        state=state,
        control=control,
        gate=gate,
        store=store,
        abilities=[loaded],
        policy_path=policy_path,
        settings_path=tmp_path / "settings.env",
        env_path=tmp_path / "env",
        vault_env_path=tmp_path / "no-vault",
        infra_dir=tmp_path / "infra",
        cache_dir=tmp_path / "cache",
        input_device="ghost.mic",
        output_device=None,
        stt_model="parakeet",
    )
    app = create_app(deps)
    client = TestClient(app)
    return client, state, control, gate, writer, loaded, deps
