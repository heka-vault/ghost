import asyncio

import pytest
from ghost_core import Event, Events, GhostContext, MissingSecret, Store


def test_context_kv_and_secrets(tmp_path):
    store = Store()
    ctx = GhostContext.build(
        "notes", data_root=tmp_path / "d", cache_root=tmp_path / "c", store=store, env={"K": "v"}
    )
    assert ctx.data_dir.is_dir() and ctx.data_dir.name == "notes"
    ctx.store.set("a", "1")
    assert ctx.store.get("a") == "1" and ctx.store.items() == {"a": "1"}
    ctx.store.delete("a")
    assert ctx.store.get("a") is None
    assert ctx.secrets.get("K") == "v" and ctx.secrets.has("K") and not ctx.secrets.has("X")
    with pytest.raises(MissingSecret):
        ctx.secrets.get("X")
    other = GhostContext.build(
        "other", data_root=tmp_path / "d", cache_root=tmp_path / "c", store=store, env={}
    )
    assert other.store.items() == {}  # namespaced


async def test_events_fan_out_with_deadline():
    seen = []

    async def fast(e: Event):
        seen.append(e.kind)

    async def slow(e: Event):
        await asyncio.sleep(1)

    async def broken(e: Event):
        raise RuntimeError("x")

    ev = Events([fast, slow, broken], timeout=0.05)
    await ev.emit("wake", session_id="s")
    assert seen == ["wake"]
