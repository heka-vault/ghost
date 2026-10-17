import pytest
from ghost_core import (
    Ability,
    AbilityError,
    Contributions,
    GhostContext,
    ReadTool,
    SimpleInstance,
    Store,
    discover,
    load,
)
from ghost_core.loader import check

LOCAL = """
from ghost_core import Ability, Contributions, ReadTool, SimpleInstance

async def setup(ctx, settings):
    async def hello(args):
        ctx.store.set("greeted", "yes")
        return {"hi": ctx.ability, "data_dir": str(ctx.data_dir)}
    tools = [ReadTool("hello", "Hi.", {}, [], hello)]
    return SimpleInstance(Contributions(prompt="HELLO", read_tools=tools))

ability = Ability(name="hello", version="0.1", description="says hi", setup=setup)
"""


def make_context(tmp_path, store):
    def inner(name):
        return GhostContext.build(
            name, data_root=tmp_path / "d", cache_root=tmp_path / "c", store=store, env={}
        )

    return inner


async def test_local_directory_ability_loads_and_gets_a_context(tmp_path):
    (tmp_path / "abilities" / "hello").mkdir(parents=True)
    (tmp_path / "abilities" / "hello" / "__init__.py").write_text(LOCAL)
    found = discover(tmp_path / "abilities")
    assert "hello" in found and found["hello"].description == "says hi"
    store = Store()
    [loaded] = await load(["hello"], discovered=found, make_context=make_context(tmp_path, store))
    assert loaded.contributions.prompt == "HELLO"
    out = await loaded.contributions.read_tools[0].handler({})
    assert out["hi"] == "hello" and out["data_dir"].endswith("d/hello")
    assert store.kv_get("hello", "greeted") == "yes"


async def test_unknown_and_passive(tmp_path):
    async def setup(ctx, s):
        return SimpleInstance(Contributions())

    passive = Ability("ears", "1", "hears idle audio", setup, passive=True)
    with pytest.raises(AbilityError, match="unknown ability"):
        await load(["nope"], discovered={}, make_context=make_context(tmp_path, Store()))
    with pytest.raises(AbilityError, match="passive"):
        await load(
            ["ears"], discovered={"ears": passive}, make_context=make_context(tmp_path, Store())
        )
    [ok] = await load(
        ["ears"],
        discovered={"ears": passive},
        make_context=make_context(tmp_path, Store()),
        passive_allowed=["ears"],
    )
    assert ok.name == "ears"


async def test_setup_errors_name_the_ability(tmp_path):
    async def setup(ctx, s):
        raise ValueError("GITHUB_TOKEN is required")

    bad = Ability("gh", "1", "", setup)
    with pytest.raises(AbilityError, match="gh: GITHUB_TOKEN"):
        await load(["gh"], discovered={"gh": bad}, make_context=make_context(tmp_path, Store()))


def test_check_catches_shadowing_and_duplicates():
    async def h(a):
        return {}

    c = Contributions(read_tools=[ReadTool("change", "", {}, [], h), ReadTool("x", "", {}, [], h)])
    problems = check(Ability("a", "1", "", None), c)  # type: ignore[arg-type]
    assert any("shadows" in p for p in problems)
    c = Contributions(read_tools=[ReadTool("x", "", {}, [], h), ReadTool("x", "", {}, [], h)])
    assert any("duplicate" in p for p in check(Ability("a", "1", "", None), c))  # type: ignore[arg-type]


def test_installed_entry_point_is_discovered():
    found = discover()
    assert "github_issues" in found  # the in-repo ability is installed in the workspace
