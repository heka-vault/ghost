"""The active thread's scope, as an ability sees it (ADR 0019).

A thread (ADR 0014's conversation) may be scoped per ability to a list of items: the
repositories, directories or workspaces it may touch. An ability declares how it can
be scoped (``Contributions.scope_kind``) and what the whole allowlist is
(``Contributions.scope_items``); a thread's scope is a subset. Scope items are
compared against ``Target.scope()``, the same string a policy target rule matches, so
the gate can refuse a write outside the thread without knowing the ability.

The pipeline hands an ``ActiveScope`` to a scoped ability on every call as the
``_scope`` argument of read tools and ``prepare``; the ability never holds it, so a
switch between threads needs no hook and two sessions cannot see each other's scope.
Ghost (line zero) is unscoped: ``items`` is None and every check passes.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from typing import Any

from ghost_core.policy import PolicyError

SCOPE_ARG = "_scope"


class OutOfScope(PolicyError):
    """An item is outside the active thread. Safe to show the model: it names the
    thread(s) that cover the item so Ghost can offer to switch."""

    def __init__(self, ability: str, item: str, owners: Iterable[str] = ()) -> None:
        self.ability = ability
        self.item = item
        self.owners = tuple(owners)
        where = f"; it belongs to the {self.owners[0]} thread" if self.owners else ""
        super().__init__(f"{item} is outside this thread{where}")


def _no_owners(_item: str) -> list[str]:
    return []


@dataclass(frozen=True)
class ActiveScope:
    """What the active thread may touch for one ability. ``items`` None means
    unscoped (Ghost, or a thread with no scope for this ability). ``reads_wider`` is
    the operator's cross-department-answers setting: reads may leave the scope,
    writes never. ``owners`` names the threads that cover an item, for the message."""

    ability: str
    thread: str
    items: tuple[str, ...] | None = None
    reads_wider: bool = False
    owners: Callable[[str], list[str]] = field(default=_no_owners, compare=False, repr=False)

    @property
    def scoped(self) -> bool:
        return self.items is not None

    @property
    def only(self) -> str | None:
        """The one item, when the thread covers exactly one; tools may stop asking for it."""
        return self.items[0] if self.items is not None and len(self.items) == 1 else None

    def covers(self, item: str) -> bool:
        return self.items is None or item.strip().lower() in {i.lower() for i in self.items}

    def narrow(self, items: Iterable[str]) -> list[str]:
        """The ability's allowlist cut down to this thread: what a list tool shows by
        default. Order is kept."""
        return [i for i in items if self.covers(i)]

    def check_write(self, item: str) -> None:
        if not self.covers(item):
            raise OutOfScope(self.ability, item, self.owners(item))

    def check_read(self, item: str) -> None:
        if not self.reads_wider:
            self.check_write(item)


def scope_of(args: dict[str, Any]) -> ActiveScope | None:
    """The scope the pipeline put in a tool's arguments, if any."""
    s = args.get(SCOPE_ARG)
    return s if isinstance(s, ActiveScope) else None
