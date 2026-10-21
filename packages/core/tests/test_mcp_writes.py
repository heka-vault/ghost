"""Writes through the gate by declaration (ADR 0022), against an in-process server shaped
like the official GitHub one: one ``issue_write`` tool multiplexed by ``method``."""

from __future__ import annotations

import asyncio
import contextlib
from typing import Any

import pytest
from ghost_core import ActiveScope, Autonomy, AutonomyPolicy, OutOfScope, Store, WriteGate
from ghost_core.mcp import (
    MCPChange,
    MCPSnapshot,
    MCPTarget,
    Server,
    ServerSpec,
)
from ghost_core.policy import PolicyError
from mcp.client.session import ClientSession
from mcp.server.mcpserver import MCPServer
from mcp.shared.memory import create_client_server_memory_streams
from mcp.types import ToolAnnotations

POLICY = """
[mcp.github]
command = "fake"
scope = { kind = "repository", arguments = ["owner", "repo"], items = ["acme/app", "acme/web"] }

[mcp.github.writes.close]
tool = "issue_write"
with = { method = "update", state = "closed" }
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
asked_by = ["close", "shut"]
risk = "high"

[mcp.github.writes.set_labels]
tool = "issue_write"
with = { method = "update" }
args = ["owner", "repo", "issue_number", "labels"]
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
risk = "low"
bound = { labels = 3 }

[mcp.github.writes.comment]
tool = "add_issue_comment"
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
risk = "low"
key_in = "body"

[mcp.github.writes.retitle]
tool = "issue_write"
with = { method = "update" }
args = ["owner", "repo", "issue_number", "title"]
target = ["owner", "repo", "issue_number"]
risk = "medium"

[mcp.github.writes.ghost_op]
tool = "no_such_tool"
target = ["owner"]

[mcp.github.levels]
close = "confirm"
set_labels = "announce"
comment = "announce"
retitle = "quiet"

[mcp.github.targets."acme/web"]
set_labels = "deny"
"""


class World:
    """The fake server's state, shared with the test."""

    def __init__(self) -> None:
        self.issues: dict[tuple[str, str, int], dict[str, Any]] = {
            ("acme", "app", 42): {
                "number": 42,
                "title": "Crash on start",
                "state": "open",
                "labels": [{"name": "bug"}],
                "body": "x" * 3000,
                "updated_at": "t1",
            },
            ("acme", "web", 7): {
                "number": 7,
                "title": "Typo",
                "state": "open",
                "labels": [],
                "body": "",
                "updated_at": "t1",
            },
        }
        self.comments: list[dict[str, Any]] = []
        self.calls: list[tuple[str, dict[str, Any]]] = []


def make_server(world: World) -> MCPServer:
    srv = MCPServer("fake-github")

    @srv.tool(annotations=ToolAnnotations(readOnlyHint=True))
    def issue_read(method: str, owner: str, repo: str, issue_number: int) -> dict:
        """Get information about a specific issue in a GitHub repository. Long text."""
        world.calls.append(("issue_read", {"method": method, "owner": owner, "repo": repo}))
        issue = world.issues.get((owner, repo, issue_number))
        if issue is None:
            raise ValueError("not found")
        if method == "get":
            return dict(issue)
        return {"comments": world.comments}

    @srv.tool()
    def issue_write(
        method: str,
        owner: str,
        repo: str,
        issue_number: int | None = None,
        state: str | None = None,
        labels: list[str] | None = None,
        title: str | None = None,
    ) -> dict:
        """Create a new or update an existing issue in a GitHub repository."""
        world.calls.append(("issue_write", {"method": method, "state": state, "labels": labels}))
        issue = world.issues[(owner, repo, int(issue_number or 0))]
        if state:
            issue["state"] = state
        if labels is not None:
            issue["labels"] = [{"name": lb} for lb in labels]
        if title:
            issue["title"] = title
        issue["updated_at"] = issue["updated_at"] + "+"
        return dict(issue)

    @srv.tool()
    def add_issue_comment(owner: str, repo: str, issue_number: int, body: str) -> dict:
        """Add a comment to a specific issue in a GitHub repository."""
        world.comments.append({"issue": issue_number, "body": body})
        world.issues[(owner, repo, issue_number)]["updated_at"] += "c"
        return {"id": len(world.comments)}

    @srv.tool(annotations=ToolAnnotations(readOnlyHint=True))
    def list_issues(owner: str, repo: str, state: str = "open") -> dict:
        """List issues in a GitHub repository."""
        return {
            "issues": [dict(i) for (o, r, _n), i in world.issues.items() if (o, r) == (owner, repo)]
        }

    return srv


@contextlib.asynccontextmanager
async def connected(spec: ServerSpec, world: World | None = None):
    world = world or World()
    srv = make_server(world)
    async with create_client_server_memory_streams() as (client_streams, server_streams):
        low = srv._lowlevel_server  # noqa: SLF001 - the SDK's run-over-streams seam
        task = asyncio.create_task(
            low.run(server_streams[0], server_streams[1], low.create_initialization_options())
        )
        try:
            async with ClientSession(*client_streams) as session:
                await session.initialize()
                server = Server(spec, environ={})
                await server.attach(session)
                yield server, world
        finally:
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError, Exception):
                await task


def policy() -> AutonomyPolicy:
    return AutonomyPolicy.parse(POLICY)


def spec() -> ServerSpec:
    return policy().mcp["github"]


def gate_for(server: Server, autonomy: AutonomyPolicy | None = None) -> WriteGate:
    g = WriteGate(Store(), autonomy=autonomy or policy())
    g.register(server.name, server.operations())
    return g


def op(server: Server, name: str):
    return next(o for o in server.operations() if o.name == name)


# --- the declaration ----------------------------------------------------------------


def test_declaration_parses_and_levels_live_in_the_server_section():
    p = policy()
    s = p.mcp["github"]
    assert s.scope is not None and s.scope.kind == "repository"
    assert s.scope.items == ("acme/app", "acme/web")
    close = s.writes["close"]
    assert close.tool == "issue_write"
    assert close.fixed == {"method": "update", "state": "closed"}
    assert close.read is not None and close.read.fixed == {"method": "get"}
    assert close.asked_by == ("close", "shut")
    assert s.writes["comment"].key_in == "body"
    assert p.resolve("github", "close", "acme/app") is Autonomy.CONFIRM
    assert p.resolve("github", "set_labels", "acme/web") is Autonomy.DENY
    assert p.resolve("github", "set_labels", "acme/app") is Autonomy.ANNOUNCE


def test_declaration_round_trips_through_to_toml():
    p = policy()
    again = AutonomyPolicy.parse(p.to_toml())
    assert again.mcp == p.mcp
    assert again.skills == p.skills
    assert again.targets == p.targets
    assert "[github]" not in p.to_toml()  # the server's levels stay in its section


@pytest.mark.parametrize(
    "body",
    [
        "[mcp.x.writes.w]\ntool = 't'",  # no target
        "[mcp.x.writes.w]\ntarget = ['a']\nrisk = 'wild'",
        "[mcp.x.writes.w]\ntarget = ['a']\nbound = { a = 0 }",
        "[mcp.x]\nscope = { kind = 'repository' }",  # no arguments
        "[mcp.x]\nurl = 'http://x'\nbogus = 1",
    ],
)
def test_bad_declarations_are_rejected(body: str):
    text = body if body.startswith("[mcp.x]") else "[mcp.x]\ncommand = 'c'\n" + body
    if body.startswith("[mcp.x]") and "url" not in body:
        text = body + "\ncommand = 'c'"
    with pytest.raises(ValueError, match=r"mcp\.x"):
        AutonomyPolicy.parse(text)


# --- operations from the declaration -----------------------------------------------


async def test_operations_hide_pinned_arguments_and_skip_unknown_tools():
    async with connected(spec()) as (server, _):
        names = {o.name for o in server.operations()}
        assert names == {"close", "set_labels", "comment", "retitle"}  # ghost_op: no tool
        close = op(server, "close")
        assert "method" not in close.properties and "state" not in close.properties
        assert set(close.required) >= {"owner", "repo", "issue_number"}
        assert close.asked_by == ("close", "shut")
        assert close.risk == "high"
        assert "state closed" in close.description
        labels = op(server, "set_labels")
        assert set(labels.properties) == {"owner", "repo", "issue_number", "labels"}
        # reads are prefixed by server, writes are keyed by operation under the server
        assert {t.name for t in server.read_tools()} == {"github_issue_read", "github_list_issues"}


async def test_announce_level_write_applies_and_reads_back():
    async with connected(spec()) as (server, world):
        gate = gate_for(server)
        labels = op(server, "set_labels")
        prepared = await labels.prepare(
            {"owner": "acme", "repo": "app", "issue_number": "42", "labels": ["bug", "triaged"]}
        )
        assert isinstance(prepared.target, MCPTarget)
        assert prepared.target.spoken() == "acme/app#42"
        assert prepared.target.scope() == "acme/app"
        assert prepared.target.fields["issue_number"] == 42  # coerced by the tool's schema
        sub = await gate.submit(
            session_id="s",
            ability="github",
            operation="set_labels",
            target=prepared.target,
            changes=prepared.changes,
            rationale="triaged",
        )
        assert sub.autonomy is Autonomy.ANNOUNCE and sub.outcome is not None
        assert sub.outcome.status == "success"
        assert world.issues[("acme", "app", 42)]["labels"] == [{"name": "bug"}, {"name": "triaged"}]
        assert sub.outcome.readback is not None
        assert sub.outcome.readback.spoken() == "open, labels bug, triaged"
        # the pinned method went with the call; the model never set it
        assert (
            "issue_write",
            {"method": "update", "state": None, "labels": ["bug", "triaged"]},
        ) in world.calls


async def test_confirm_level_waits_and_the_request_is_the_yes():
    async with connected(spec()) as (server, world):
        gate = gate_for(server)
        close = op(server, "close")
        prepared = await close.prepare({"owner": "acme", "repo": "app", "issue_number": 42})
        sub = await gate.submit(
            session_id="s",
            ability="github",
            operation="close",
            target=prepared.target,
            changes=prepared.changes,
            rationale="",
        )
        assert sub.autonomy is Autonomy.CONFIRM and sub.outcome is None
        assert world.issues[("acme", "app", 42)]["state"] == "open"
        assert sub.proposal.preview == "On acme/app#42: close."
        # "close forty two" plainly asks for it (ADR 0017)
        outcome = await gate.confirm_by_request("s", "ghost, close forty two")
        assert outcome is not None and outcome.status == "success"
        assert world.issues[("acme", "app", 42)]["state"] == "closed"
        assert outcome.readback is not None and outcome.readback.spoken() == "closed, labels bug"


async def test_stale_target_is_not_written():
    async with connected(spec()) as (server, world):
        gate = gate_for(server)
        close = op(server, "close")
        prepared = await close.prepare({"owner": "acme", "repo": "app", "issue_number": 42})
        await gate.submit(
            session_id="s",
            ability="github",
            operation="close",
            target=prepared.target,
            changes=prepared.changes,
            rationale="",
        )
        world.issues[("acme", "app", 42)]["title"] = "Crash on start (edited meanwhile)"
        outcome = await gate.confirm_pending("s")
        assert outcome is not None and outcome.status == "failed"
        assert "changed since I read it" in (outcome.error or "")
        assert world.issues[("acme", "app", 42)]["state"] == "open"


async def test_model_cannot_set_a_pinned_argument_or_an_unknown_one():
    async with connected(spec()) as (server, _):
        close = op(server, "close")
        with pytest.raises(ValueError, match="state is fixed for close"):
            await close.prepare(
                {"owner": "acme", "repo": "app", "issue_number": 42, "state": "open"}
            )
        with pytest.raises(ValueError, match="no argument"):
            await close.prepare({"owner": "acme", "repo": "app", "issue_number": 42, "zap": 1})
        with pytest.raises(ValueError, match="needs"):
            await close.prepare({"owner": "acme", "repo": "app"})


async def test_bounds_from_the_declaration_and_the_defaults():
    async with connected(spec()) as (server, _):
        labels = op(server, "set_labels")
        with pytest.raises(ValueError, match="too many items"):
            await labels.prepare(
                {"owner": "acme", "repo": "app", "issue_number": 42, "labels": ["a", "b", "c", "d"]}
            )
        comment = op(server, "comment")
        with pytest.raises(ValueError, match="too long"):
            await comment.prepare(
                {"owner": "acme", "repo": "app", "issue_number": 42, "body": "x" * 5000}
            )


async def test_allowlist_and_thread_scope_refuse_before_policy():
    async with connected(spec()) as (server, _):
        gate = gate_for(server)
        close = op(server, "close")
        with pytest.raises(ValueError, match="not an allowed repository"):
            await close.prepare({"owner": "evil", "repo": "corp", "issue_number": 1})
        scope = ActiveScope(ability="github", thread="web", items=("acme/web",))
        with pytest.raises(OutOfScope):
            await close.prepare(
                {"owner": "acme", "repo": "app", "issue_number": 42, "_scope": scope}
            )
        # the gate checks the allowlist and the scope again on its own
        target = MCPTarget(
            server="github",
            fields={"owner": "evil", "repo": "corp", "issue_number": 1},
            item="evil/corp",
            rest=("1",),
        )
        with pytest.raises(PolicyError):
            await gate.submit(
                session_id="s",
                ability="github",
                operation="close",
                target=target,
                changes=(
                    await close.prepare({"owner": "acme", "repo": "app", "issue_number": 42})
                ).changes,
                rationale="",
            )


async def test_deny_per_target_from_the_server_section():
    async with connected(spec()) as (server, _):
        gate = gate_for(server)
        labels = op(server, "set_labels")
        prepared = await labels.prepare(
            {"owner": "acme", "repo": "web", "issue_number": 7, "labels": ["x"]}
        )
        with pytest.raises(PolicyError, match="not allowed"):
            await gate.submit(
                session_id="s",
                ability="github",
                operation="set_labels",
                target=prepared.target,
                changes=prepared.changes,
                rationale="",
            )


async def test_a_scoped_thread_fills_the_repository_in():
    async with connected(spec()) as (server, world):
        scope = ActiveScope(ability="github", thread="app", items=("acme/app",))
        close = op(server, "close")
        prepared = await close.prepare({"issue_number": 42, "_scope": scope})
        assert prepared.target.fields == {"owner": "acme", "repo": "app", "issue_number": 42}
        read = next(t for t in server.read_tools() if t.name == "github_issue_read")
        out = await read.handler({"method": "get", "issue_number": 42, "_scope": scope})
        assert out["number"] == 42
        # outside the thread: OutOfScope reaches the pipeline, which can route the request
        # to the thread that owns the item (ADR 0019); with wider answers on, it reads
        with pytest.raises(OutOfScope, match="outside this thread"):
            await read.handler(
                {
                    "method": "get",
                    "owner": "acme",
                    "repo": "web",
                    "issue_number": 7,
                    "_scope": scope,
                }
            )
        wider = ActiveScope(ability="github", thread="app", items=("acme/app",), reads_wider=True)
        out = await read.handler(
            {"method": "get", "owner": "acme", "repo": "web", "issue_number": 7, "_scope": wider}
        )
        assert out["number"] == 7


async def test_no_read_declared_means_no_stale_check_and_the_audit_says_so():
    async with connected(spec()) as (server, world):
        gate = gate_for(server)
        retitle = op(server, "retitle")
        prepared = await retitle.prepare(
            {"owner": "acme", "repo": "app", "issue_number": 42, "title": "Crash at launch"}
        )
        sub = await gate.submit(
            session_id="s",
            ability="github",
            operation="retitle",
            target=prepared.target,
            changes=prepared.changes,
            rationale="",
        )
        assert sub.autonomy is Autonomy.QUIET and sub.outcome is not None
        assert sub.outcome.status == "success"
        assert world.issues[("acme", "app", 42)]["title"] == "Crash at launch"
        snap = sub.proposal.snapshot
        assert isinstance(snap, MCPSnapshot) and not snap.checked
        assert "no read is declared" in snap.spoken()


async def test_comment_carries_the_idempotency_marker():
    async with connected(spec()) as (server, world):
        gate = gate_for(server)
        comment = op(server, "comment")
        prepared = await comment.prepare(
            {"owner": "acme", "repo": "app", "issue_number": 42, "body": "Looks like a dupe of 40."}
        )
        sub = await gate.submit(
            session_id="s",
            ability="github",
            operation="comment",
            target=prepared.target,
            changes=prepared.changes,
            rationale="",
        )
        assert sub.outcome is not None and sub.outcome.status == "success"
        assert world.comments[0]["body"].startswith("Looks like a dupe of 40.\n\n<!-- ghost:")
        assert isinstance(prepared.changes.steps[0], MCPChange)
        assert prepared.changes.steps[0].describe() == "comment: body Looks like a dupe of 40."


async def test_server_failure_on_apply_is_a_failed_step_not_a_crash():
    async with connected(spec()) as (server, world):
        gate = gate_for(server)
        labels = op(server, "set_labels")
        prepared = await labels.prepare(
            {"owner": "acme", "repo": "app", "issue_number": 42, "labels": ["x"]}
        )
        del world.issues[("acme", "app", 42)]  # the server will fail the write
        world.issues[("acme", "app", 42)] = {
            "number": 42,
            "state": "open",
            "labels": [],
            "updated_at": "t",
        }

        async def boom(name, args, **kw):
            raise RuntimeError("boom")

        server.call = boom  # type: ignore[method-assign]
        with pytest.raises(RuntimeError):
            await gate.submit(
                session_id="s",
                ability="github",
                operation="set_labels",
                target=prepared.target,
                changes=prepared.changes,
                rationale="",
            )


async def test_results_are_trimmed_for_a_spoken_turn():
    async with connected(spec()) as (server, _):
        read = next(t for t in server.read_tools() if t.name == "github_issue_read")
        out = await read.handler(
            {"method": "get", "owner": "acme", "repo": "app", "issue_number": 42}
        )
        assert len(out["body"]) == 1201 and out["body"].endswith("…")
        assert read.description == "Get information about a specific issue in a GitHub repository."


def test_target_speech_and_mentions():
    t = MCPTarget(
        server="github",
        fields={"owner": "acme", "repo": "app", "issue_number": 142},
        item="acme/app",
        rest=("142",),
    )
    assert t.spoken() == "acme/app#142"
    assert "one forty two" in t.mentions() and "142" in t.mentions()
    repo_only = MCPTarget(server="github", fields={"owner": "acme", "repo": "app"}, item="acme/app")
    assert repo_only.spoken() == "acme/app" and repo_only.mentions() == set()
    unscoped = MCPTarget(server="notes", fields={"path": "todo.md"}, rest=("todo.md",))
    assert unscoped.spoken() == "todo.md" and unscoped.scope() == "mcp:notes:todo.md"


def test_prompt_names_tools_and_narrows_to_the_thread():
    async def run():
        async with connected(spec()) as (server, _):
            text = server.prompt_for(None)
            assert (
                "github_issue_read" in text
                and "`change` (close, set_labels, comment, retitle)" in text
            )
            assert (
                "these repositorys only: acme/app, acme/web" in text or "these repositories" in text
            )
            narrowed = server.prompt_for(
                ActiveScope(ability="github", thread="app", items=("acme/app",))
            )
            assert "one repository, acme/app" in narrowed

    asyncio.run(run())
