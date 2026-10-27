from __future__ import annotations

from pathlib import Path

import pytest
from api_fakes import Doc, FakeWriter, Tag
from fastapi.testclient import TestClient
from ghost_api import DaemonState, Deps, Phase, create_app
from ghost_core import (
    AutonomyPolicy,
    ChangeSet,
    Operation,
    Prepared,
    ReadTool,
    Store,
    ToolSource,
    WriteGate,
)
from ghost_core.conversations import SOURCE as CONVERSATIONS
from ghost_core.conversations import Conversations
from ghost_core.folder import GhostFolder
from ghost_core.outputs import Outputs
from ghost_core.rules import SOURCE as RULES
from ghost_core.rules import Rules
from ghost_core.waiting import Items, WaitingList


class FakeControl:
    """Records calls; a real gate sits behind confirm/cancel."""

    def __init__(self, state: DaemonState, gate: WriteGate, policy_path: Path):
        self.state = state
        self.gate = gate
        self.policy_path = policy_path
        self.calls: list[tuple] = []

    async def wake(self, first_utterance, conversation=None):
        call = ("wake", first_utterance)
        self.calls.append(call if conversation is None else (*call, conversation))
        if self.state.phase is not Phase.PASSIVE:
            return False
        self.state.set_phase(Phase.LISTENING, "sess1")
        self.state.set_conversation(conversation or "ghost")
        return True

    async def sleep(self):
        self.calls.append(("sleep",))
        if self.state.session_id is None:
            return False
        self.state.set_phase(Phase.PASSIVE)
        return True

    async def say(self, text, conversation=None):
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

    async def confirm_proposal(self, proposal_id):
        return await self.gate.confirm_id(proposal_id, source="ui")

    async def cancel_proposal(self, proposal_id):
        return self.gate.cancel_id(proposal_id, "ui") is not None

    def apply_settings(self, values):
        self.applied = [k for k in values if k.startswith("GHOST_WAKE_")]
        return list(self.applied)

    def reload_policy(self):
        policy = AutonomyPolicy.load(self.policy_path)
        self.gate.set_autonomy(policy)
        self.calls.append(("reload_policy",))
        return policy

    # conversations (ADR 0014)
    async def switch_conversation(self, slug):
        self.calls.append(("switch", slug))
        if not self.state.session_id:
            return False
        self.state.set_conversation(slug)
        return True

    async def compact_conversation(self, slug):
        self.calls.append(("compact", slug))
        return True

    def voices(self):
        return ["af_heart", "am_adam", "bf_emma"]

    async def sample_voice(self, voice, speed, text):
        self.calls.append(("sample", voice, speed, text))
        return True

    # the paired phone (ADR 0016)
    phone_address: str | None = None
    phone_device = None

    def phone_pairing(self):
        from datetime import UTC, datetime, timedelta

        from ghost_api import PhonePairing

        self.calls.append(("phone_pairing",))
        if self.phone_address is None:
            return None
        return PhonePairing(
            code="123456",
            address=self.phone_address,
            expires_at=datetime.now(UTC) + timedelta(seconds=120),
        )

    def phone_view(self):
        from ghost_api import PhoneView

        return PhoneView(address=self.phone_address, device=self.phone_device)

    async def forget_phone(self):
        self.calls.append(("forget_phone",))
        had = self.phone_device is not None
        self.phone_device = None
        return had


def make_source(writer) -> ToolSource:
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
    return ToolSource(
        name="fake",
        prompt="A fake app. It echoes.",
        read_tools=[ReadTool("echo", "Echo.", {}, [], echo)],
        operations=ops,
    )


@pytest.fixture
def world(tmp_path: Path):
    store = Store()
    writer = FakeWriter()
    loaded = make_source(writer)
    policy_path = tmp_path / "policy.toml"
    # ADR 0034: everything runs unless a rule asks; this world asks before a nuke.
    policy_path.write_text('[permissions]\nask = ["fake.nuke"]\n')
    gate = WriteGate(store, autonomy=AutonomyPolicy.load(policy_path))
    gate.register("fake", loaded.operations)
    config = tmp_path / "config"
    folder = GhostFolder(config)
    folder.ensure_layout()
    conversations = Conversations(
        store,
        config,
        default_voice="af_heart",
        voices=("af_heart", "am_adam", "bf_emma"),
    )
    gate.register(CONVERSATIONS, conversations.operations())
    rules = Rules(config)
    gate.register(RULES, rules.operations())
    outputs = Outputs(config)
    gate.register("outputs", outputs.operations())
    state = DaemonState(version="test", wake_mode="file")
    control = FakeControl(state, gate, policy_path)
    infra = tmp_path / "infra" / "pipewire"
    infra.mkdir(parents=True)
    (infra / "install.sh").write_text("#!/bin/sh\necho installed $1\n")
    (infra / "install.sh").chmod(0o755)
    deps = Deps(
        state=state,
        control=control,
        gate=gate,
        store=store,
        sources=[loaded],
        policy_path=policy_path,
        settings_path=tmp_path / "settings.env",
        env_path=tmp_path / "env",
        vault_env_path=tmp_path / "no-vault",
        infra_dir=tmp_path / "infra",
        cache_dir=tmp_path / "cache",
        input_device="ghost.mic",
        output_device=None,
        stt_model="parakeet",
        conversations=conversations,
        folder=folder,
        rules=rules,
        outputs=outputs,
        waiting=WaitingList(Items(store), changes=gate.waiting),
    )
    app = create_app(deps)
    client = TestClient(app)
    return client, state, control, gate, writer, loaded, deps
