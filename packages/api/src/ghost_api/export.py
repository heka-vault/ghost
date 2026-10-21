"""Write the OpenAPI document: ``uv run python -m ghost_api.export > packages/api/openapi.json``.

The committed file is what ``packages/client-ts`` is generated from; a test fails when
it is stale."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

from ghost_api.app import Deps, create_app
from ghost_api.state import DaemonState


class _NoControl:
    async def wake(self, first_utterance):
        return False

    async def sleep(self):
        return False

    async def say(self, text):
        return False

    async def confirm_pending(self):
        return None

    async def cancel_pending(self):
        return False

    def reload_policy(self):
        raise NotImplementedError

    async def switch_conversation(self, slug):
        return False

    async def compact_conversation(self, slug):
        return False

    def voices(self):
        return []

    async def sample_voice(self, voice, speed, text):
        return False

    def phone_pairing(self):
        return None

    def phone_view(self):
        from ghost_api.models import PhoneView

        return PhoneView(address=None, device=None)

    async def forget_phone(self):
        return False


def openapi() -> dict[str, Any]:
    import tempfile

    from ghost_core import Store, WriteGate
    from ghost_core.conversations import Conversations

    store = Store()
    deps = Deps(
        state=DaemonState(version="schema", wake_mode="file"),
        control=_NoControl(),
        gate=WriteGate(store),
        store=store,
        sources=[],
        skills=None,
        templates=None,
        conversations=Conversations(store, Path(tempfile.mkdtemp()) / "conversations"),
        policy_path=Path("/dev/null"),
        settings_path=Path("/dev/null"),
        env_path=Path("/dev/null"),
        vault_env_path=Path("/dev/null"),
        infra_dir=Path("/dev/null"),
        cache_dir=Path("/dev/null"),
        input_device=None,
        output_device=None,
        stt_model="",
    )
    return create_app(deps).openapi()


def main() -> None:
    json.dump(openapi(), sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
