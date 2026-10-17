"""A tiny write model for API tests. Its own module so the codec can import the types
back by name (``api_fakes:Doc``), unlike ``conftest``, which exists once per package."""

from __future__ import annotations

from ghost_core import Change, Snapshot, Target


class Doc(Target, frozen=True):
    name: str

    def key(self):
        return f"doc:{self.name}"

    def scope(self):
        return "docs"


class DocSnap(Snapshot, frozen=True):
    tags: tuple[str, ...]

    def spoken(self):
        return "tags " + (", ".join(self.tags) or "none")


class Tag(Change, frozen=True):
    tag: str

    def describe(self):
        return f"add {self.tag}"

    def invert(self, before, marker):
        return Untag(tag=self.tag)


class Untag(Change, frozen=True):
    tag: str

    def describe(self):
        return f"remove {self.tag}"


class FakeWriter:
    def __init__(self):
        self.tags: list[str] = []
        self.version = 0

    async def fetch(self, target):
        return DocSnap(version=str(self.version), tags=tuple(self.tags))

    async def apply(self, target, change, key):
        match change:
            case Tag(tag=t):
                self.tags.append(t)
            case Untag(tag=t):
                self.tags.remove(t)
        self.version += 1
