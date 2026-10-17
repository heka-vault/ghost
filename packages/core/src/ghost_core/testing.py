"""Helpers for ability authors' tests: no audio, no Pipecat, no keys."""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
from typing import Any

from ghost_core.ability import Ability, Contributions
from ghost_core.context import GhostContext
from ghost_core.loader import LoadedAbility, check
from ghost_core.store import Store


def fake_context(tmp_path: Path, ability: str = "test", env: Mapping[str, str] | None = None):
    return GhostContext.build(
        ability,
        data_root=tmp_path / "data",
        cache_root=tmp_path / "cache",
        store=Store(),
        env=dict(env or {}),
    )


async def load_for_test(
    ability: Ability, tmp_path: Path, env: Mapping[str, str] | None = None
) -> LoadedAbility:
    """Instantiate an ability the way the loader would, with settings from ``env``.
    Settings classes read the process environment, so pass ``monkeypatch``-ed values
    or a config class that accepts them."""
    settings = ability.config() if ability.config else None
    instance = await ability.setup(fake_context(tmp_path, ability.name, env), settings)
    contributions = instance.contributions()
    problems = check(ability, contributions)
    assert not problems, problems
    return LoadedAbility(ability, settings, instance, contributions)


async def call_read_tool(c: Contributions, name: str, args: dict[str, Any]) -> dict[str, Any]:
    tool = next(t for t in c.read_tools if t.name == name)
    return await tool.handler(args)
