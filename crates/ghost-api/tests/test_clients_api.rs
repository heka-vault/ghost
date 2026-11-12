//! Settings' outside tools (ADR 0048): a client is made and its token shown once, listed
//! without it, and removed. A temp store throughout.

mod common;

use common::World;
use serde_json::{Value, json};

#[tokio::test]
async fn a_client_is_made_listed_and_removed_and_its_token_is_shown_once() {
    let mut w = World::new();
    w.clients_listening = vec!["127.0.0.1:47824".into()];
    let c = w.client();
    let empty = c.get("/clients").await;
    assert_eq!(empty.status, 200, "{}", empty.text);
    assert_eq!(empty["clients"], json!([]));
    assert_eq!(empty["listening"], json!(["127.0.0.1:47824"]));
    assert_eq!(empty["problem"], Value::Null);

    let made = c
        .post("/clients", Some(json!({"name": "claude-code"})))
        .await;
    assert_eq!(made.status, 200, "{}", made.text);
    let token = made["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("ghost.claude-code."));
    assert_eq!(made["client"]["name"], "claude-code");
    assert_eq!(made["client"]["title"], "Claude Code");
    assert_eq!(made["client"]["last_used_at"], Value::Null);
    // Ready to paste: the command for Claude Code and the config other tools read.
    assert_eq!(
        made["claude_code"],
        format!(
            "claude mcp add --transport http ghost http://127.0.0.1:47824/mcp --header \"Authorization: Bearer {token}\""
        )
    );
    let config: Value = serde_json::from_str(made["config_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        config["mcpServers"]["ghost"]["url"],
        "http://127.0.0.1:47824/mcp"
    );
    assert_eq!(made["clients"]["clients"][0]["name"], "claude-code");

    // Never again: not in the list, and only its hash in the store.
    let listed = c.get("/clients").await;
    assert_eq!(listed["clients"][0]["title"], "Claude Code");
    assert!(!listed.text.contains(&token));
    let row = w.store.get_client("claude-code").unwrap().unwrap();
    assert!(!row.token_hash.contains(&token) && row.token_hash.len() == 64);

    // The same name again, and a name that will not do.
    let again = c
        .post("/clients", Some(json!({"name": "claude-code"})))
        .await;
    assert_eq!(again.status, 409, "{}", again.text);
    let bad = c
        .post("/clients", Some(json!({"name": "Claude Code"})))
        .await;
    assert_eq!(bad.status, 422, "{}", bad.text);
    assert_eq!(c.post("/clients", Some(json!({}))).await.status, 422);

    let gone = c.delete("/clients/claude-code").await;
    assert_eq!(gone.status, 200, "{}", gone.text);
    assert_eq!(gone["clients"], json!([]));
    assert_eq!(c.delete("/clients/claude-code").await.status, 404);
    assert!(w.store.get_client("claude-code").unwrap().is_none());
}

#[tokio::test]
async fn a_tool_that_only_starts_commands_gets_a_command_and_no_token_in_its_config() {
    let w = World::new();
    let c = w.client();
    let made = c
        .post("/clients", Some(json!({"name": "codex", "command": true})))
        .await;
    assert_eq!(made.status, 200, "{}", made.text);
    let token = made["token"].as_str().unwrap();
    assert!(
        made["claude_code"]
            .as_str()
            .unwrap()
            .ends_with("mcp serve --client codex")
    );
    assert!(!made["config_json"].as_str().unwrap().contains(token));
    // The token is where the bridge reads it, for the operator's eyes only.
    let file = w.tmp.path().join("clients/codex.token");
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), token);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(c.delete("/clients/codex").await.status, 200);
    assert!(!file.exists());
}

#[tokio::test]
async fn inside_an_appimage_a_command_tool_is_given_the_address_and_told_why() {
    let mut w = World::new();
    w.in_appimage = true;
    let c = w.client();
    let made = c
        .post("/clients", Some(json!({"name": "codex", "command": true})))
        .await;
    assert_eq!(made.status, 200, "{}", made.text);
    let token = made["token"].as_str().unwrap();
    assert!(made["note"].as_str().unwrap().contains("AppImage"));
    assert!(made["claude_code"].as_str().unwrap().contains("--transport http"));
    assert!(made["config_json"].as_str().unwrap().contains(token));
    assert!(!w.tmp.path().join("clients/codex.token").exists());
    // Outside one, no note.
    let w = World::new();
    let made = w
        .client()
        .post("/clients", Some(json!({"name": "cursor"})))
        .await;
    assert_eq!(made["note"], Value::Null);
}
