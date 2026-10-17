from __future__ import annotations

import pytest
from fakes import FakeWriter, Issue, register_fake
from ghost_core import Store, WriteGate


@pytest.fixture
def target() -> Issue:
    return Issue(owner="acme", repo="app", number=42)


@pytest.fixture
def writer(target) -> FakeWriter:
    w = FakeWriter()
    w.seed(target)
    return w


@pytest.fixture
def gate(writer) -> WriteGate:
    g = WriteGate(Store())
    register_fake(g, writer)
    return g
