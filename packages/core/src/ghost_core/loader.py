"""Find and load abilities (ADR 0012).

Discovery: the ``ghost.abilities`` entry point group of every installed
distribution, plus ``<local_dir>/<name>/__init__.py`` for one-offs. Both must export
an ``Ability`` named ``ability``. Enabling is explicit: an installed package should
not get a microphone and a token by accident.
"""

from __future__ import annotations

import importlib.metadata
import importlib.util
import sys
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from loguru import logger

from ghost_core.ability import Ability, AbilityInstance, Contributions
from ghost_core.context import GhostContext

ENTRY_POINT_GROUP = "ghost.abilities"


class AbilityError(Exception):
    pass


@dataclass
class LoadedAbility:
    ability: Ability
    settings: Any
    instance: AbilityInstance
    contributions: Contributions

    @property
    def name(self) -> str:
        return self.ability.name


def discover(local_dir: Path | None = None) -> dict[str, Ability]:
    found: dict[str, Ability] = {}
    for ep in importlib.metadata.entry_points(group=ENTRY_POINT_GROUP):
        try:
            obj = ep.load()
        except Exception as e:  # noqa: BLE001 - one broken package must not hide the rest
            logger.warning(f"ability {ep.name!r} failed to import: {type(e).__name__}: {e}")
            continue
        if not isinstance(obj, Ability):
            logger.warning(f"entry point {ep.name!r} is not an Ability; skipped")
            continue
        found[ep.name] = obj
    if local_dir and local_dir.is_dir():
        for init in sorted(local_dir.glob("*/__init__.py")):
            name = init.parent.name
            mod_name = f"ghost_local_abilities.{name}"
            spec = importlib.util.spec_from_file_location(
                mod_name, init, submodule_search_locations=[str(init.parent)]
            )
            if spec is None or spec.loader is None:
                continue
            module = importlib.util.module_from_spec(spec)
            sys.modules[mod_name] = module  # so the codec can import its types back
            try:
                spec.loader.exec_module(module)
            except Exception as e:  # noqa: BLE001
                logger.warning(f"local ability {name!r} failed to import: {type(e).__name__}: {e}")
                continue
            obj = getattr(module, "ability", None)
            if isinstance(obj, Ability):
                found[name] = obj  # a local copy shadows an installed one
            else:
                logger.warning(f"local ability {name!r} does not export `ability`; skipped")
    return found


def check(ability: Ability, contributions: Contributions) -> list[str]:
    """Problems with an ability's declaration. Empty means fine."""
    problems: list[str] = []
    names = [t.name for t in contributions.read_tools]
    if len(set(names)) != len(names):
        problems.append("duplicate read tool names")
    for reserved in ("change", "hang_up", "hold"):
        if reserved in names:
            problems.append(f"read tool {reserved!r} shadows a Ghost tool")
    ops = [o.name for o in contributions.operations]
    if len(set(ops)) != len(ops):
        problems.append("duplicate operation names")
    for o in contributions.operations:
        if o.risk not in ("low", "medium", "high"):
            problems.append(f"operation {o.name!r}: risk must be low, medium or high")
    return problems


async def load(
    names: Sequence[str],
    *,
    discovered: dict[str, Ability],
    make_context: Callable[[str], GhostContext],
    passive_allowed: Iterable[str] = (),
) -> list[LoadedAbility]:
    """Instantiate settings from the environment, run ``setup``, collect contributions.
    Raises ``AbilityError`` naming the ability on any problem so startup fails loudly."""
    passive_ok = set(passive_allowed)
    out: list[LoadedAbility] = []
    for name in names:
        ability = discovered.get(name)
        if ability is None:
            raise AbilityError(f"unknown ability {name!r}; installed: {sorted(discovered)}")
        if ability.passive and name not in passive_ok:
            raise AbilityError(
                f"{name!r} hears idle audio (passive=True); list it under "
                "[abilities] passive in the policy file to allow that"
            )
        try:
            settings = ability.config() if ability.config else None
        except Exception as e:  # noqa: BLE001 - pydantic-settings raises its own types
            raise AbilityError(f"{name}: settings: {e}") from None
        try:
            instance = await ability.setup(make_context(name), settings)
            contributions = instance.contributions()
        except (ValueError, KeyError) as e:
            raise AbilityError(f"{name}: {e}") from None
        problems = check(ability, contributions)
        if problems:
            raise AbilityError(f"{name}: " + "; ".join(problems))
        out.append(LoadedAbility(ability, settings, instance, contributions))
    return out
