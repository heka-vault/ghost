"""The apps routes (ADR 0034): what is connected and what each tool may do, the catalog,
connecting and disconnecting, and the permissions. No network: connecting is stubbed."""

from __future__ import annotations

import json

from ghost_core.apps import App, Apps, AppSpec, ToolInfo, load_apps
from ghost_core.catalog import Catalog
from ghost_core.policy import AutonomyPolicy

LINEAR = {
    "server": {
        "name": "app.linear/linear",
        "title": "Linear",
        "description": "Issues and projects.",
        "version": "1.0.0",
        "remotes": [{"type": "streamable-http", "url": "https://mcp.linear.app/mcp"}],
    }
}
GITHUB = {
    "server": {
        "name": "io.github.github/github-mcp-server",
        "title": "GitHub",
        "description": "Repos and issues.",
        "version": "2.0.3",
        "remotes": [
            {
                "type": "streamable-http",
                "url": "https://api.githubcopilot.com/mcp/",
                "headers": [{"name": "Authorization", "description": "Token", "isSecret": True}],
            }
        ],
    }
}


def with_apps(world, tmp_path, monkeypatch):
    """A connected github app by hand, a catalog with two entries, and connecting stubbed:
    an app that is added just records itself as not connected."""
    client, state, control, gate, writer, loaded, deps = world
    apps = Apps({}, environ={})
    live = App(AppSpec(name="github", title="GitHub", command="x"), environ={})
    live.tools = [
        ToolInfo("issue_read", "Get an issue. Then do this.", {}, read_only=True),
        ToolInfo("issue_write", "Change an issue.", {}, read_only=False),
    ]
    live._session = object()  # noqa: SLF001 - connected, for the listing
    apps.specs["github"] = live.spec
    apps.apps["github"] = live
    started: list[AppSpec] = []

    async def start(spec, *, interactive=False):
        started.append(spec)
        apps.failed[spec.name] = "not reachable"
        return None

    monkeypatch.setattr(apps, "_start", start)
    deps.apps = apps

    async def fetch(url):
        return {"servers": [] if "updated_since" in url else [LINEAR, GITHUB], "metadata": {}}

    deps.catalog = Catalog(tmp_path / "catalog.json", fetch=fetch)
    monkeypatch.setenv("HOME", str(tmp_path))
    return client, deps, started


def test_apps_list_each_tool_with_what_the_permissions_say(world, tmp_path, monkeypatch):
    client, deps, _ = with_apps(world, tmp_path, monkeypatch)
    (github,) = client.get("/apps").json()
    assert github["state"] == "connected" and github["kind"] == "local"
    actions = {a["name"]: a for a in github["actions"]}
    assert actions["issue_read"] == {
        "name": "issue_read",
        "description": "Get an issue.",
        "reads": True,
        "level": "allow",
        "rule": "github.issue_read",
        "conditional": False,
    }
    assert actions["issue_write"]["level"] == "allow"  # open by default
    r = client.put("/permissions", json={"set": {"rule": "github.issue_write", "level": "ask"}})
    assert r.status_code == 200 and "github.issue_write" in r.json()["ask"]
    client.put("/permissions", json={"block": ["github.*(repo!=site)"]})
    actions = {a["name"]: a for a in client.get("/apps/github").json()["actions"]}
    assert actions["issue_write"]["level"] == "ask" and actions["issue_write"]["conditional"]
    assert client.get("/apps/nope").status_code == 404


def test_permissions_keep_the_rest_of_the_file_and_refuse_a_bad_rule(world, tmp_path, monkeypatch):
    client, deps, _ = with_apps(world, tmp_path, monkeypatch)
    deps.policy_path.write_text('# mine\n\n[routes]\nenabled = ["digest"]\n')
    r = client.put("/permissions", json={"mode": "read-only", "allow": ["github.issue_write"]})
    assert r.status_code == 200 and r.json()["mode"] == "read-only"
    text = deps.policy_path.read_text()
    assert text.startswith("# mine") and 'enabled = ["digest"]' in text
    assert deps.gate.autonomy.resolve("github", "issue_write").value == "announce"
    assert deps.gate.autonomy.resolve("github", "delete_repo").value == "deny"
    bad = client.put("/permissions", json={"block": ["github.(oops"]})
    assert bad.status_code == 422 and "is not a rule" in bad.json()["detail"]
    assert client.get("/permissions").json()["allow"] == ["github.issue_write"]
    view = client.get("/policy").json()
    assert view["permissions"]["mode"] == "read-only"


def test_the_catalog_lists_first_party_apps_and_searches(world, tmp_path, monkeypatch):
    client, deps, _ = with_apps(world, tmp_path, monkeypatch)
    page = client.get("/catalog").json()
    assert [e["title"] for e in page["entries"]] == ["GitHub", "Linear"]
    gh = page["entries"][0]
    assert gh["suggested_name"] == "github-2"  # github is taken
    assert gh["options"][0]["kind"] == "key" and gh["options"][0]["settings"][0]["secret"]
    assert client.get("/catalog", params={"search": "projects"}).json()["total"] == 1
    entry = client.get("/catalog/app.linear/linear").json()
    assert entry["options"][0]["label"] == "Sign in"


def test_connecting_keeps_the_key_out_of_the_file_and_never_shows_it(world, tmp_path, monkeypatch):
    client, deps, started = with_apps(world, tmp_path, monkeypatch)
    client.get("/catalog")
    r = client.post(
        "/apps",
        json={
            "catalog": "io.github.github/github-mcp-server",
            "option": "remote-0",
            "name": "work-github",
            "values": {"Authorization": "ghp_secret_value"},
        },
    )
    assert r.status_code == 200, r.text
    assert "ghp_secret_value" not in r.text
    (app,) = r.json()["apps"]
    assert app["name"] == "work-github" and app["kind"] == "key" and app["state"] == "failed"
    assert app["settings"] == ["GHOST_APP_WORK_GITHUB_AUTHORIZATION"]
    saved = load_apps(deps.policy_path.parent / "apps.json").apps["work-github"]
    assert saved.headers == {"Authorization": "${GHOST_APP_WORK_GITHUB_AUTHORIZATION}"}
    apps_env = deps.env_path.parent / "apps.env"
    assert "Bearer ghp_secret_value" in apps_env.read_text()
    assert not deps.env_path.exists() or "ghp_secret" not in deps.env_path.read_text()
    assert [s.name for s in started] == ["work-github"]
    again = client.post(
        "/apps",
        json={
            "catalog": "io.github.github/github-mcp-server",
            "option": "remote-0",
            "name": "work-github",
            "values": {"Authorization": "x"},
        },
    )
    assert again.status_code == 409
    missing = client.post(
        "/apps", json={"catalog": "io.github.github/github-mcp-server", "option": "remote-0"}
    )
    assert missing.status_code == 422 and "needed" in missing.json()["detail"]


def test_a_pasted_config_and_a_url_connect_too(world, tmp_path, monkeypatch):
    client, deps, started = with_apps(world, tmp_path, monkeypatch)
    pasted = json.dumps(
        {
            "mcpServers": {
                "mobbin": {
                    "type": "http",
                    "url": "http://proxy/mcp",
                    "headers": {"Authorization": "Bearer " + "k" * 40},
                }
            }
        }
    )
    r = client.post("/apps", json={"config": pasted})
    assert r.status_code == 200 and ("k" * 40) not in r.text
    saved = load_apps(deps.policy_path.parent / "apps.json").apps
    assert saved["mobbin"].headers == {"Authorization": "${GHOST_APP_MOBBIN_AUTHORIZATION}"}
    r = client.post("/apps", json={"url": "https://mcp.notion.com/mcp", "name": "notion"})
    assert r.status_code == 200 and r.json()["apps"][0]["kind"] == "sign_in"
    assert client.post("/apps", json={"url": "ftp://x"}).status_code == 422
    assert client.post("/apps", json={}).status_code == 422


def test_disconnecting_takes_it_out_of_the_file_and_the_tools(world, tmp_path, monkeypatch):
    client, deps, _ = with_apps(world, tmp_path, monkeypatch)
    client.post("/apps", json={"url": "https://mcp.notion.com/mcp", "name": "notion"})
    assert "notion" in load_apps(deps.policy_path.parent / "apps.json").apps
    client.post(
        "/apps",
        json={"url": "https://mcp.example.com/mcp", "name": "example", "values": {"key": "k1"}},
    )
    apps_env = deps.env_path.parent / "apps.env"
    assert "GHOST_APP_EXAMPLE_KEY" in apps_env.read_text()
    assert client.delete("/apps/example").json()["ok"] is True
    assert "GHOST_APP_EXAMPLE_KEY" not in apps_env.read_text()  # its keys go with it
    assert client.delete("/apps/notion").json()["ok"] is True
    assert "notion" not in load_apps(deps.policy_path.parent / "apps.json").apps
    assert "notion" not in deps.apps.specs
    assert client.delete("/apps/notion").status_code == 404


def test_the_status_and_settings_name_the_apps(world, tmp_path, monkeypatch):
    client, deps, _ = with_apps(world, tmp_path, monkeypatch)
    assert client.get("/status").json()["apps"] == ["github"]
    deps.apps.specs["proxy"] = AppSpec(name="proxy", url="http://x", headers={"A": "${PROXY_KEY}"})
    secrets = {s["name"]: s["set"] for s in client.get("/settings").json()["secrets"]}
    assert secrets["PROXY_KEY"] is False


def test_without_apps_the_routes_say_so(world):
    client, *_ = world
    assert client.get("/apps").status_code == 503
    assert AutonomyPolicy().resolve("x", "y").value == "announce"
