"""What the active session may touch through a source, as the source sees it (ADR 0019,
ADR 0024).

A department's declaration of a server is the only scope: its ``scope.items`` are the
repositories, directories or workspaces the server may touch there. Scope items are
compared against ``Target.scope()``, the same string a policy target rule matches, so the
gate can refuse a write outside it without knowing the source.

The pipeline hands an ``ActiveScope`` to a scoped source on every call as the ``_scope``
argument of read tools and ``prepare``; the source never holds it. ``items`` is None for
every session now (the allowlist is the server's own); what the scope carries is
``owners``: which departments cover an item the active one does not, so a request about
it can be routed there or offered (``OutOfScope``).
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from typing import Any

from ghost_core.policy import PolicyError

SCOPE_ARG = "_scope"


class OutOfScope(PolicyError):
    """An item is outside what this department reaches. Safe to show the model: it names
    the department(s) that cover the item so Ghost can offer to transfer."""

    def __init__(self, ability: str, item: str, owners: Iterable[str] = ()) -> None:
        self.ability = ability
        self.item = item
        self.owners = tuple(owners)
        where = f"; it belongs to the {self.owners[0]} department" if self.owners else ""
        super().__init__(f"{item} is outside this department{where}")


def _no_owners(_item: str) -> list[str]:
    return []


@dataclass(frozen=True)
class ActiveScope:
    """What the active session may touch for one source. ``items`` None means unscoped
    (the server's own allowlist is the limit). ``reads_wider`` lets reads leave the
    scope, writes never. ``owners`` names the departments that cover an item."""

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
        """The one item, when exactly one is covered; tools may stop asking for it."""
        return self.items[0] if self.items is not None and len(self.items) == 1 else None

    def covers(self, item: str) -> bool:
        return self.items is None or item.strip().lower() in {i.lower() for i in self.items}

    def narrow(self, items: Iterable[str]) -> list[str]:
        """The source's allowlist cut down to what is covered: what a list tool shows by
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
