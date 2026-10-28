//! The apps routes (ADR 0034): what is connected and what each tool may do, the catalog,
//! connecting and disconnecting, and the permissions. No network: connecting is stubbed.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::{FakeApps, World, by};
use ghost_core::apps::{AppSpec, ToolInfo, load_apps};
use ghost_core::catalog::{Catalog, CatalogOptions, Fetch, FetchError};
use ghost_core::levels::Autonomy;
use ghost_core::passwords::{MemoryKeyring, Passwords};
use ghost_core::policy::AutonomyPolicy;
use serde_json::{Map, Value, json};

fn linear() -> Value {
    json!({
        "server": {
            "name": "app.linear/linear",
            "title": "Linear",
            "description": "Issues and projects.",
            "version": "1.0.0",
            "remotes": [{"type": "streamable-http", "url": "https://mcp.linear.app/mcp"}],
        }
    })
}

fn github() -> Value {
    json!({
        "server": {
            "name": "io.github.github/github-mcp-server",
            "title": "GitHub",
            "description": "Repos and issues.",
            "version": "2.0.3",
            "remotes": [
                {
                    "type": "streamable-http",
                    "url": "https://api.githubcopilot.com/mcp/",
                    "headers": [{"name": "Authorization", "description": "Token", "isSecret": true}],
                }
            ],
        }
    })
}

/// The registry, faked: `servers` on the first page, none since.
struct FakeFetch(Vec<Value>);

#[async_trait]
impl Fetch for FakeFetch {
    async fn fetch(&self, url: &str) -> Result<Value, FetchError> {
        let servers = if url.contains("updated_since") {
            Vec::new()
        } else {
            self.0.clone()
        };
        Ok(json!({"servers": servers, "metadata": {}}))
    }
}

fn catalog_of(w: &World, name: &str, servers: Vec<Value>) -> Arc<Catalog> {
    Arc::new(Catalog::with_options(
        w.path().join(name),
        CatalogOptions {
            fetch: Some(Arc::new(FakeFetch(servers))),
            ..Default::default()
        },
    ))
}

/// A connected github app by hand, a catalog with two entries, and connecting stubbed: an
/// app that is added just records itself as not connected.
fn with_apps(w: &mut World) -> Arc<FakeApps> {
    let mut spec = AppSpec::runs("github", "x");
    spec.title = "GitHub".into();
    let apps = Arc::new(FakeApps::connected(
        spec,
        vec![
            ToolInfo::new(
                "issue_read",
                "Get an issue. Then do this.",
                Map::new(),
                true,
            ),
            ToolInfo::new("issue_write", "Change an issue.", Map::new(), false),
        ],
    ));
    w.apps = Some(apps.clone());
    w.catalog = Some(catalog_of(w, "catalog.json", vec![linear(), github()]));
    apps
}

fn apps_file(w: &World) -> std::path::PathBuf {
    w.policy_path.parent().unwrap().join("apps.json")
}

#[tokio::test]
async fn apps_list_each_tool_with_what_the_permissions_say() {
    let mut w = World::new();
    with_apps(&mut w);
    let c = w.client();
    let listed = c.get("/apps").await;
    let items = listed.json.as_array().unwrap();
    assert_eq!(items.len(), 1);
    let github = &items[0];
    assert_eq!(github["state"], "connected");
    assert_eq!(github["kind"], "local");
    let actions = by(&github["actions"], "name");
    assert_eq!(
        *actions["issue_read"],
        json!({
            "name": "issue_read",
            "description": "Get an issue.",
            "reads": true,
            "level": "allow",
            "rule": "github.issue_read",
            "conditional": false,
        })
    );
    assert_eq!(actions["issue_write"]["level"], "allow"); // open by default
    let r = c
        .put(
            "/permissions",
            json!({"set": {"rule": "github.issue_write", "level": "ask"}}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert!(
        r["ask"]
            .as_array()
            .unwrap()
            .contains(&json!("github.issue_write"))
    );
    c.put("/permissions", json!({"block": ["github.*(repo!=site)"]}))
        .await;
    let view = c.get("/apps/github").await;
    let actions = by(&view["actions"], "name");
    assert_eq!(actions["issue_write"]["level"], "ask");
    assert_eq!(actions["issue_write"]["conditional"], true);
    assert_eq!(c.get("/apps/nope").await.status, 404);
}

#[tokio::test]
async fn permissions_keep_the_rest_of_the_file_and_refuse_a_bad_rule() {
    let mut w = World::new();
    with_apps(&mut w);
    let c = w.client();
    std::fs::write(
        &w.policy_path,
        "# mine\n\n[routes]\nenabled = [\"digest\"]\n",
    )
    .unwrap();
    let r = c
        .put(
            "/permissions",
            json!({"mode": "read-only", "allow": ["github.issue_write"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["mode"], "read-only");
    let text = std::fs::read_to_string(&w.policy_path).unwrap();
    assert!(text.starts_with("# mine"));
    assert!(text.contains("enabled = [\"digest\"]"));
    assert_eq!(
        w.gate.autonomy().resolve("github", "issue_write", None),
        Autonomy::Announce
    );
    assert_eq!(
        w.gate.autonomy().resolve("github", "delete_repo", None),
        Autonomy::Deny
    );
    let bad = c
        .put("/permissions", json!({"block": ["github.(oops"]}))
        .await;
    assert_eq!(bad.status, 422);
    assert!(bad.detail().contains("is not a rule"));
    assert_eq!(
        c.get("/permissions").await["allow"],
        json!(["github.issue_write"])
    );
    let view = c.get("/policy").await;
    assert_eq!(view["permissions"]["mode"], "read-only");
}

#[tokio::test]
async fn the_catalog_lists_first_party_apps_and_searches() {
    let mut w = World::new();
    with_apps(&mut w);
    let c = w.client();
    let page = c.get("/catalog").await;
    assert_eq!(
        common::strings(&page["entries"], "title"),
        vec!["GitHub".to_string(), "Linear".to_string()]
    );
    let gh = &page["entries"][0];
    assert_eq!(gh["suggested_name"], "github-2"); // github is taken
    assert_eq!(gh["options"][0]["kind"], "key");
    assert_eq!(gh["options"][0]["settings"][0]["secret"], true);
    assert_eq!(c.get("/catalog?search=projects").await["total"], 1);
    let entry = c.get("/catalog/app.linear/linear").await;
    assert_eq!(entry["options"][0]["label"], "Sign in");
}

#[tokio::test]
async fn connecting_keeps_the_key_out_of_the_file_and_never_shows_it() {
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    c.get("/catalog").await;
    let r = c
        .post(
            "/apps",
            Some(json!({
                "catalog": "io.github.github/github-mcp-server",
                "option": "remote-0",
                "name": "work-github",
                "values": {"Authorization": "ghp_secret_value"},
            })),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains("ghp_secret_value"));
    let app = &r["apps"][0];
    assert_eq!(app["name"], "work-github");
    assert_eq!(app["kind"], "key");
    assert_eq!(app["state"], "failed");
    assert_eq!(
        app["settings"],
        json!(["GHOST_APP_WORK_GITHUB_AUTHORIZATION"])
    );
    let saved = load_apps(&apps_file(&w));
    let saved = saved.get("work-github").unwrap();
    assert_eq!(
        saved.headers.get("Authorization").unwrap(),
        "${GHOST_APP_WORK_GITHUB_AUTHORIZATION}"
    );
    // Into the vault (ADR 0036), sealed; no plaintext file holds it.
    assert_eq!(
        w.vault
            .value("GHOST_APP_WORK_GITHUB_AUTHORIZATION")
            .unwrap()
            .as_str(),
        "Bearer ghp_secret_value"
    );
    let raw = std::fs::read(w.vault.path()).unwrap();
    assert!(!raw.windows(10).any(|win| win == b"ghp_secret"));
    assert!(!w.env_path.parent().unwrap().join("apps.env").exists());
    assert!(!w.env_path.exists());
    assert_eq!(
        apps.started()
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>(),
        vec!["work-github".to_string()]
    );
    let again = c
        .post(
            "/apps",
            Some(json!({
                "catalog": "io.github.github/github-mcp-server",
                "option": "remote-0",
                "name": "work-github",
                "values": {"Authorization": "x"},
            })),
        )
        .await;
    assert_eq!(again.status, 409);
    let missing = c
        .post(
            "/apps",
            Some(json!({"catalog": "io.github.github/github-mcp-server", "option": "remote-0"})),
        )
        .await;
    assert_eq!(missing.status, 422);
    assert!(missing.detail().contains("needed"));
}

#[tokio::test]
async fn a_pasted_config_and_a_url_connect_too() {
    let mut w = World::new();
    with_apps(&mut w);
    let c = w.client();
    let pasted = json!({
        "mcpServers": {
            "mobbin": {
                "type": "http",
                "url": "http://proxy/mcp",
                "headers": {"Authorization": format!("Bearer {}", "k".repeat(40))},
            }
        }
    })
    .to_string();
    let r = c.post("/apps", Some(json!({"config": pasted}))).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains(&"k".repeat(40)));
    let saved = load_apps(&apps_file(&w));
    assert_eq!(
        saved
            .get("mobbin")
            .unwrap()
            .headers
            .get("Authorization")
            .unwrap(),
        "${GHOST_APP_MOBBIN_AUTHORIZATION}"
    );
    let r = c
        .post(
            "/apps",
            Some(json!({"url": "https://mcp.notion.com/mcp", "name": "notion"})),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r["apps"][0]["kind"], "sign_in");
    assert_eq!(
        c.post("/apps", Some(json!({"url": "ftp://x"})))
            .await
            .status,
        422
    );
    assert_eq!(c.post("/apps", Some(json!({}))).await.status, 422);
}

#[tokio::test]
async fn disconnecting_takes_it_out_of_the_file_and_the_tools() {
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    c.post(
        "/apps",
        Some(json!({"url": "https://mcp.notion.com/mcp", "name": "notion"})),
    )
    .await;
    assert!(load_apps(&apps_file(&w)).get("notion").is_some());
    c.post(
        "/apps",
        Some(json!({"url": "https://mcp.example.com/mcp", "name": "example", "values": {"key": "k1"}})),
    )
    .await;
    assert!(w.vault.contains("GHOST_APP_EXAMPLE_KEY"));
    assert_eq!(c.delete("/apps/example").await["ok"], true);
    assert!(!w.vault.contains("GHOST_APP_EXAMPLE_KEY")); // its keys go with it
    assert_eq!(c.delete("/apps/notion").await["ok"], true);
    assert!(load_apps(&apps_file(&w)).get("notion").is_none());
    assert!(!apps.has_spec("notion"));
    assert_eq!(c.delete("/apps/notion").await.status, 404);
}

#[tokio::test]
async fn the_status_and_settings_name_the_apps() {
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    assert_eq!(c.get("/status").await["apps"], json!(["github"]));
    let mut proxy = AppSpec::at("proxy", "http://x");
    proxy.headers.insert("A".into(), "${PROXY_KEY}".into());
    apps.push_spec(proxy);
    let settings = c.get("/settings").await;
    let secrets = by(&settings["secrets"], "name");
    assert_eq!(secrets["PROXY_KEY"]["set"], false);
}

#[tokio::test]
async fn without_apps_the_routes_say_so() {
    let w = World::new();
    let c = w.client();
    assert_eq!(c.get("/apps").await.status, 503);
    assert_eq!(
        AutonomyPolicy::default().resolve("x", "y", None),
        Autonomy::Announce
    );
}

fn skills_entry() -> Value {
    json!({
        "server": {
            "name": "io.github.someone/skills",
            "title": "Skills",
            "version": "1",
            "remotes": [{"type": "streamable-http", "url": "https://skills.example/mcp"}],
        }
    })
}

#[tokio::test]
async fn an_app_never_takes_the_name_of_ghosts_own_tools() {
    let mut w = World::new();
    let apps = with_apps(&mut w);
    w.catalog = Some(catalog_of(&w, "catalog2.json", vec![skills_entry()]));
    let c = w.client();
    c.get("/catalog").await;
    let entry = c.get("/catalog/io.github.someone/skills").await;
    assert_eq!(entry["suggested_name"], "skills-app");
    let r = c
        .post(
            "/apps",
            Some(json!({"catalog": "io.github.someone/skills", "option": "remote-0"})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["apps"][0]["name"], "skills-app");
    for name in ["skills", "conversations", "waiting"] {
        let r = c
            .post(
                "/apps",
                Some(json!({"url": "https://x.example/mcp", "name": name})),
            )
            .await;
        assert_eq!(r.status, 422, "{name}");
        assert!(r.detail().contains("Ghost's own"));
        assert!(!apps.has_spec(name));
    }
    let pasted = json!({"mcpServers": {"outputs": {"url": "https://x.example/mcp"}}}).to_string();
    let r = c.post("/apps", Some(json!({"config": pasted}))).await;
    assert_eq!(r.status, 422);
    assert!(r.detail().contains("Ghost's own"));
}

#[tokio::test]
async fn disconnecting_forgets_only_that_apps_own_keys() {
    // `github-2` is what the catalog suggests beside `github`; disconnecting `github` used
    // to delete every GHOST_APP_GITHUB_* key, `github-2`'s too.
    let mut w = World::new();
    with_apps(&mut w);
    let c = w.client();
    for (name, key) in [("work", "k1"), ("work-2", "k2")] {
        let r = c
            .post(
                "/apps",
                Some(json!({"url": "https://mcp.example.com/mcp", "name": name, "values": {"key": key}})),
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.text);
    }
    w.vault
        .set("GHOST_APP_WORK_MINE", "kept-by-the-operator")
        .unwrap();
    assert_eq!(c.delete("/apps/work").await.status, 200);
    let left = w.vault.names();
    assert!(!left.contains(&"GHOST_APP_WORK_KEY".to_string()));
    assert!(left.contains(&"GHOST_APP_WORK_2_KEY".to_string())); // another app's
    assert!(left.contains(&"GHOST_APP_WORK_MINE".to_string())); // the entry never named it
}

#[tokio::test]
async fn disconnecting_while_the_keyring_is_locked_changes_nothing() {
    // Its passwords would stay behind in a vault Ghost can't open: refused, said why.
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    let r = c
        .post(
            "/apps",
            Some(json!({"url": "https://mcp.example.com/mcp", "name": "work", "values": {"key": "k1"}})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    w.keyring.set_locked(true);
    assert!(!w.vault.open());
    let r = c.delete("/apps/work").await;
    assert_eq!(r.status, 423);
    assert!(r.detail().contains("locked"));
    assert!(apps.has_spec("work"));
    w.keyring.set_locked(false);
    assert!(w.vault.open());
    assert_eq!(c.delete("/apps/work").await.status, 200);
    assert!(!w.vault.names().contains(&"GHOST_APP_WORK_KEY".to_string()));
    // An app with no passwords of Ghost's disconnects whatever the keyring says.
    w.keyring.set_locked(true);
    assert!(!w.vault.open());
    assert_eq!(c.delete("/apps/github").await.status, 200);
}

#[tokio::test]
async fn two_apps_never_share_key_names() {
    // `my-app` and `my_app` keep their keys under the same names.
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    let url = "https://mcp.example.com/mcp";
    let first = c
        .post(
            "/apps",
            Some(json!({"url": url, "name": "my-app", "values": {"key": "k1"}})),
        )
        .await;
    assert_eq!(first.status, 200);
    let again = c
        .post(
            "/apps",
            Some(json!({"url": url, "name": "my_app", "values": {"key": "k2"}})),
        )
        .await;
    assert_eq!(again.status, 409);
    assert!(again.detail().contains("my-app"));
    assert_eq!(
        w.vault.value("GHOST_APP_MY_APP_KEY").unwrap().as_str(),
        "Bearer k1"
    );
    assert!(!apps.has_spec("my_app"));
}

#[tokio::test]
async fn connecting_never_rewrites_entries_ghost_could_not_read() {
    // apps.json is edited one entry at a time: an entry Ghost can't read (a bad name, a
    // key it doesn't know) stays as the operator wrote it, and so does a key at the top.
    let mut w = World::new();
    with_apps(&mut w);
    let c = w.client();
    let path = apps_file(&w);
    let theirs = json!({
        "Bad Name": {"url": "https://bad.example/mcp"},
        "later": {"command": "later-app", "disabled": true},
        "both": {"url": "https://x", "command": "y"},
    });
    std::fs::write(&path, json!({"apps": theirs, "note": "mine"}).to_string()).unwrap();
    let url = "https://mcp.notion.com/mcp";
    assert_eq!(
        c.post("/apps", Some(json!({"url": url, "name": "notion"})))
            .await
            .status,
        200
    );
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["note"], "mine");
    let mut others = raw["apps"].as_object().unwrap().clone();
    others.remove("notion");
    assert_eq!(Value::Object(others), theirs);
    assert_eq!(raw["apps"]["notion"]["url"], url);
    assert_eq!(c.delete("/apps/notion").await.status, 200);
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw, json!({"apps": theirs, "note": "mine"}));
}

#[tokio::test]
async fn a_file_that_is_not_json_is_never_overwritten() {
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    let path = apps_file(&w);
    let broken = "{\"apps\": {\"github\": {\"command\": \"github-mcp-server\",}}"; // a stray comma
    std::fs::write(&path, broken).unwrap();
    let r = c
        .post(
            "/apps",
            Some(json!({"url": "https://mcp.notion.com/mcp", "name": "notion"})),
        )
        .await;
    assert_eq!(r.status, 409);
    assert!(r.detail().contains("apps.json"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    assert!(apps.started().is_empty());
    assert!(!apps.has_spec("notion"));
    let r = c.delete("/apps/github").await;
    assert_eq!(r.status, 409);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    assert!(apps.has_spec("github")); // still connected: nothing changed
}

#[tokio::test]
async fn a_pasted_config_names_the_entries_it_leaves_out() {
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    let pasted = json!({
        "mcpServers": {
            "good": {"url": "https://good.example/mcp"},
            "Bad Name": {"url": "https://bad.example/mcp"},
        }
    })
    .to_string();
    let r = c.post("/apps", Some(json!({"config": pasted}))).await;
    assert_eq!(r.status, 422);
    assert!(r.detail().contains("Bad Name"));
    assert!(apps.started().is_empty());
    assert!(!apps.has_spec("good"));
    let noted =
        json!({"mcpServers": {"good": {"url": "https://good.example/mcp", "x": 1}}}).to_string();
    let r = c.post("/apps", Some(json!({"config": noted}))).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(
        r["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str().unwrap().contains('x'))
    );
}

#[tokio::test]
async fn a_key_in_a_pasted_configs_arguments_or_address_is_moved_too() {
    // docs/apps.md: Ghost moves any key written into a pasted config into its private
    // settings. Arguments and the address's query stayed in apps.json as plain text.
    let mut w = World::new();
    with_apps(&mut w);
    let c = w.client();
    let pasted = json!({
        "mcpServers": {
            "brave": {"command": "npx", "args": ["-y", "brave-search", "--api-key", "BSAx1"]},
            "maps": {"url": "https://maps.example/mcp?key=AIzaSy123&lang=en"},
        }
    })
    .to_string();
    let r = c.post("/apps", Some(json!({"config": pasted}))).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains("BSAx1") && !r.text.contains("AIzaSy123"));
    assert_eq!(r["problems"], json!([])); // moved, nothing left to fix
    let path = apps_file(&w);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("BSAx1") && !text.contains("AIzaSy123"));
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let saved = load_apps(&path);
    assert_eq!(
        saved.get("brave").unwrap().args.last().unwrap(),
        "${GHOST_APP_BRAVE_API_KEY}"
    );
    assert_eq!(
        saved.get("maps").unwrap().url.as_deref().unwrap(),
        "https://maps.example/mcp?key=${GHOST_APP_MAPS_KEY}&lang=en"
    );
    assert_eq!(
        w.vault.value("GHOST_APP_BRAVE_API_KEY").unwrap().as_str(),
        "BSAx1"
    );
    assert_eq!(
        w.vault.value("GHOST_APP_MAPS_KEY").unwrap().as_str(),
        "AIzaSy123"
    );
    let view = c.get("/apps/maps").await;
    assert_eq!(view["settings"], json!(["GHOST_APP_MAPS_KEY"]));
    assert_eq!(view["kind"], "key");
}

#[tokio::test]
async fn a_key_waits_for_a_locked_keyring_and_nothing_changes() {
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    // A vault on disk (a password already kept), whose keyring is locked now.
    w.vault
        .set("SOMEBODY_ELSES_KEY", "kept-0123456789")
        .unwrap();
    w.keyring.set_locked(true);
    assert!(!w.vault.open());
    let r = c
        .post(
            "/apps",
            Some(json!({"url": "https://mcp.example.com/mcp", "name": "example", "values": {"key": "k1"}})),
        )
        .await;
    assert_eq!(r.status, 423);
    assert!(r.detail().contains("keyring"));
    assert!(load_apps(&apps_file(&w)).get("example").is_none());
    assert!(apps.started().is_empty());
}

#[tokio::test]
async fn a_refused_key_leaves_nothing_half_connected() {
    // The vault's checks run before apps.json changes: a key it won't take refuses the
    // whole connection, and never echoes the value.
    let mut w = World::new();
    let apps = with_apps(&mut w);
    let c = w.client();
    for bad in ["k".repeat(16_001), "two\nlines-0123456789".to_string()] {
        let r = c
            .post(
                "/apps",
                Some(json!({"url": "https://mcp.example.com/mcp", "name": "example", "values": {"key": bad}})),
            )
            .await;
        assert_eq!(r.status, 422);
        assert!(!r.text.contains(&bad));
        assert!(load_apps(&apps_file(&w)).get("example").is_none());
        assert!(w.vault.names().is_empty());
    }
    assert!(apps.started().is_empty());
}

#[tokio::test]
async fn the_catalog_says_who_publishes_each_app_and_where_it_runs() {
    // #37: the six "posthog" results fold under PostHog's own; each says its publisher, and
    // the gateway that isn't PostHog's says whose it is.
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ghost-core/tests/fixtures/catalog_posthog.json");
    let six: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(fixture).unwrap()).unwrap();
    let mut w = World::new();
    with_apps(&mut w);
    w.catalog = Some(catalog_of(&w, "catalog3.json", six));
    let c = w.client();
    c.get("/catalog").await;
    let page = c.get("/catalog?search=posthog").await;
    assert_eq!(page["total"], 2);
    assert_eq!(page["entries"].as_array().unwrap().len(), 2);
    let own = &page["entries"][0];
    let engageable = &page["entries"][1];
    assert_eq!(own["name"], "io.github.PostHog/mcp");
    assert_eq!(
        own["publisher"],
        json!({"name": "PostHog", "verified": "github"})
    );
    assert_eq!(
        own["maker"],
        json!({"product": "PostHog", "evidence": ["publisher", "remote"]})
    );
    assert_eq!(own["product"], "PostHog");
    assert_eq!(own["product_maker"], Value::Null);
    assert!(
        own["updated_at"]
            .as_str()
            .unwrap()
            .starts_with("2026-01-07T16:34:40")
    );
    assert_eq!(own["options"][0]["host"], "mcp.posthog.com");
    assert_eq!(own["options"][0]["host_owner"], "maker");
    let others = by(&own["others"], "name");
    assert_eq!(others.len(), 4);
    assert!(others.values().all(|o| o["others"] == json!([])));
    let gateway = others["io.github.pipeworx-io/posthog"];
    assert_eq!(gateway["maker"], Value::Null);
    assert_eq!(gateway["product"], "PostHog");
    assert_eq!(gateway["product_maker"], "io.github.PostHog/mcp");
    assert_eq!(gateway["options"][0]["host"], "gateway.pipeworx.io");
    assert_eq!(gateway["options"][0]["host_owner"], "publisher");
    let armory = others["com.mcparmory/posthog"];
    assert_eq!(
        armory["publisher"],
        json!({"name": "mcparmory.com", "verified": "domain"})
    );
    let shape: Vec<(Value, Value, Value)> = armory["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            (
                o["registry"].clone(),
                o["package"].clone(),
                o["host"].clone(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (json!("pypi"), json!("mcparmory-posthog"), Value::Null),
            (
                json!("oci"),
                json!("ghcr.io/mcparmory/posthog:1.0.5"),
                Value::Null
            ),
        ]
    );
    assert_eq!(engageable["product"], Value::Null);
    assert_eq!(engageable["others"], json!([]));
    let one = c.get("/catalog/io.github.pipeworx-io/posthog").await;
    assert_eq!(one["product"], "PostHog");
    assert_eq!(one["others"], json!([]));
}

/// A vault over the same file with a keyring that stays locked.
#[allow(dead_code)]
fn locked_twin(w: &World) -> Arc<Passwords> {
    let twin = Passwords::new(w.vault.path(), Arc::new(MemoryKeyring::locked()));
    twin.open();
    Arc::new(twin)
}
