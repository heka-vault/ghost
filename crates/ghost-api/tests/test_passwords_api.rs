//! The Passwords page's routes (ADR 0036): names and metadata out, values only in. A fake
//! keyring and temp files throughout.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::{FakeApps, World, by};
use ghost_core::apps::AppSpec;
use ghost_core::envfile::read_env;
use serde_json::{Value, json};

const TOKEN: &str = "sk-ant-api03-thisIsTheOperatorsToken";

fn rows(view: &Value) -> BTreeMap<String, &Value> {
    by(&view["passwords"], "name")
}

#[tokio::test]
async fn the_list_names_ghosts_own_and_never_a_value() {
    let w = World::new();
    let c = w.client();
    let v = c.get("/passwords").await;
    assert_eq!(v["vault"]["state"], "open");
    assert!(
        v["vault"]["path"]
            .as_str()
            .unwrap()
            .ends_with("passwords.vault")
    );
    let own = rows(&v.json)["ANTHROPIC_AUTH_TOKEN"];
    assert_eq!(own["set"], false);
    assert_eq!(own["ghost"], true);
    assert_eq!(own["used_by"], json!(["The model connection"]));
    assert_eq!(v["importable"], json!([]));

    let r = c
        .put("/passwords/ANTHROPIC_AUTH_TOKEN", json!({"value": TOKEN}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains(TOKEN));
    // The model connection reads its key by name at each session (ADR 0037): no restart.
    assert_eq!(r["restart_required"], false);
    let row = rows(&r["passwords"])["ANTHROPIC_AUTH_TOKEN"];
    assert_eq!(row["set"], true);
    assert!(row["updated"].is_string());
    assert_eq!(row["last_used"], Value::Null);
    assert!(!c.get("/passwords").await.text.contains(TOKEN));
    assert!(!c.get("/settings").await.text.contains(TOKEN));
    let settings = c.get("/settings").await;
    assert_eq!(
        by(&settings["secrets"], "name")["ANTHROPIC_AUTH_TOKEN"]["set"],
        true
    );
    assert_eq!(
        w.vault.value("ANTHROPIC_AUTH_TOKEN").unwrap().as_str(),
        TOKEN
    );
}

#[tokio::test]
async fn an_apps_password_and_one_of_the_operators_own() {
    let mut w = World::new();
    let apps = Arc::new(FakeApps::default());
    let mut proxy = AppSpec::at("proxy", "http://x");
    proxy.title = "Proxy".into();
    proxy.headers.insert("A".into(), "${PROXY_KEY}".into());
    apps.push_spec(proxy);
    w.apps = Some(apps.clone());
    let c = w.client();
    let listed = c.get("/passwords").await;
    let proxy = rows(&listed.json)["PROXY_KEY"];
    assert_eq!(proxy["set"], false);
    assert_eq!(proxy["used_by"], json!(["Proxy"]));
    assert_eq!(proxy["ghost"], false);
    let r = c
        .put(
            "/passwords/PROXY_KEY",
            json!({"value": "proxy-key-0123456789"}),
        )
        .await;
    assert_eq!(r["restart_required"], false); // the app connects again now
    assert_eq!(apps.revived(), vec!["proxy".to_string()]);
    let mine = c
        .put(
            "/passwords/MY_SCRIPT_TOKEN",
            json!({"value": "mine-0123456789"}),
        )
        .await;
    assert_eq!(
        rows(&mine["passwords"])["MY_SCRIPT_TOKEN"]["used_by"],
        json!([])
    );
    let gone = c.delete("/passwords/MY_SCRIPT_TOKEN").await;
    assert!(!rows(&gone["passwords"]).contains_key("MY_SCRIPT_TOKEN"));
    assert_eq!(c.delete("/passwords/MY_SCRIPT_TOKEN").await.status, 404);
}

#[tokio::test]
async fn bad_names_empty_values_and_a_locked_keyring() {
    let w = World::new();
    let c = w.client();
    assert_eq!(
        c.put("/passwords/lower", json!({"value": "x".repeat(20)}))
            .await
            .status,
        422
    );
    let r = c.put("/passwords/A_TOKEN", json!({"value": "   "})).await;
    assert_eq!(r.status, 422);
    assert!(r.detail().contains("empty"));
    let long = "y".repeat(20_000);
    let r = c.put("/passwords/A_TOKEN", json!({"value": long})).await;
    assert_eq!(r.status, 422);
    assert!(!r.text.contains(&long)); // never echoed back
    w.vault.set("B_TOKEN", "kept-0123456789").unwrap(); // a vault on disk to lock
    w.keyring.set_locked(true);
    assert!(!w.vault.open());
    let r = c
        .put("/passwords/A_TOKEN", json!({"value": "a-good-value-123"}))
        .await;
    assert_eq!(r.status, 423, "{}", r.text);
    assert!(r.detail().contains("Unlock"));
}

#[tokio::test]
async fn unlock_opens_the_vault_and_the_daemon_follows() {
    let w = World::new();
    let c = w.client();
    w.vault.set("HF_TOKEN", "hf_0123456789abcdef").unwrap();
    w.keyring.set_locked(true);
    assert!(!w.vault.open());
    let v = c.get("/passwords").await;
    assert_eq!(v["vault"]["state"], "locked");
    assert_eq!(v["vault"]["problem"], "Your keyring is locked");
    // Whether each is set can't be known while it's locked: unknown, never "not set".
    assert!(
        v["passwords"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["set"] == Value::Null)
    );
    let r = c.delete("/passwords/HF_TOKEN").await;
    assert_eq!(r.status, 423);
    assert!(r.detail().contains("Unlock"));
    w.keyring.set_locked(false); // the keyring's prompt was answered
    let v = c.post("/passwords/unlock", None).await;
    assert_eq!(v["vault"]["state"], "open", "{}", v.text);
    assert_eq!(rows(&v.json)["HF_TOKEN"]["set"], true);
    assert!(w.control.has_call(json!(["passwords_opened"])));
}

#[tokio::test]
async fn import_moves_the_plaintext_files_into_the_vault() {
    let w = World::new();
    let c = w.client();
    let config = w.env_path.parent().unwrap().to_path_buf();
    std::fs::write(
        &w.env_path,
        format!("ANTHROPIC_AUTH_TOKEN={TOKEN}\nANTHROPIC_BASE_URL=http://proxy:8317\n"),
    )
    .unwrap();
    std::fs::write(
        config.join("apps.env"),
        "GHOST_APP_MOBBIN_KEY=mobbin-0123456789\n",
    )
    .unwrap();
    let before = c.get("/passwords").await;
    let names: std::collections::BTreeSet<String> = before["importable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            s["path"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        ["env".to_string(), "apps.env".to_string()]
            .into_iter()
            .collect()
    );
    let r = c.post("/passwords/import", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains(TOKEN));
    assert_eq!(
        r["imported"],
        json!(["ANTHROPIC_AUTH_TOKEN", "GHOST_APP_MOBBIN_KEY"])
    );
    assert_eq!(r["settings"], json!(["ANTHROPIC_BASE_URL"]));
    assert_eq!(r["restart_required"], true);
    let removed: std::collections::BTreeSet<String> = r["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().rsplit('/').next().unwrap().to_string())
        .collect();
    assert_eq!(
        removed,
        ["env".to_string(), "apps.env".to_string()]
            .into_iter()
            .collect()
    );
    assert_eq!(r["passwords"]["importable"], json!([]));
    assert!(!w.env_path.exists() && !config.join("apps.env").exists());
    let settings = read_env(&w.settings_path).unwrap();
    assert_eq!(settings.len(), 1);
    assert_eq!(settings["ANTHROPIC_BASE_URL"], "http://proxy:8317");
    assert_eq!(
        w.vault.value("ANTHROPIC_AUTH_TOKEN").unwrap().as_str(),
        TOKEN
    );
}

#[tokio::test]
async fn settings_never_shows_or_writes_a_password() {
    // settings.env is plain text: PUT /settings refuses a password under `values` and GET
    // /settings never returns one, whatever it is called (ADR 0036, ADR 0037).
    let mut w = World::new();
    let apps = Arc::new(FakeApps::default());
    let mut proxy = AppSpec::at("proxy", "http://x");
    proxy.title = "Proxy".into();
    proxy.headers.insert("A".into(), "${PROXY_ACCESS}".into());
    apps.push_spec(proxy);
    apps.up.lock().unwrap().insert("proxy".into());
    w.apps = Some(apps);
    let c = w.client();
    std::fs::write(&w.settings_path, "GHOST_LLM_PASSWORD=MY_MODEL\n").unwrap();
    for name in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "LLM_API_KEY",
        "PROXY_ACCESS",  // an app's ${NAME}, not shaped like a password
        "MY_MODEL",      // the one the model connection names
        "MCP_PROXY_KEY", // shaped like one
        "GHOST_APP_MOBBIN_KEY",
    ] {
        let r = c
            .put(
                "/settings",
                json!({"values": {name: "sk-the-value-0123456789"}}),
            )
            .await;
        assert_eq!(r.status, 422, "{name}");
        assert!(!r.text.contains("sk-the-value-0123456789"));
        assert!(r.detail().contains("Passwords"));
    }
    let env = read_env(&w.settings_path).unwrap();
    assert_eq!(env.len(), 1);
    assert_eq!(env["GHOST_LLM_PASSWORD"], "MY_MODEL");

    // One already written there by hand never comes back, and may be taken out.
    w.vault.set("HAND_ADDED", "hand-added-0123456789").unwrap();
    std::fs::write(
        &w.settings_path,
        "GHOST_LLM_PASSWORD=MY_MODEL\nMCP_PROXY_KEY=proxy-key-0123456789\n\
         OPENAI_API_KEY=sk-openai-0123456789\nPROXY_ACCESS=proxy-access-0123456\n\
         MY_MODEL=my-model-key-0123456\nHAND_ADDED=hand-added-0123456789\n\
         GHOST_REPOS=acme/app\n",
    )
    .unwrap();
    let r = c.get("/settings").await;
    assert_eq!(
        r["values"],
        json!({"GHOST_LLM_PASSWORD": "MY_MODEL", "GHOST_REPOS": "acme/app"})
    );
    for leaked in [
        "proxy-key",
        "sk-openai",
        "proxy-access",
        "my-model-key",
        "hand-added",
    ] {
        assert!(!r.text.contains(leaked), "{leaked}");
    }
    let r = c
        .put("/settings", json!({"values": {"MCP_PROXY_KEY": null}}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(
        !read_env(&w.settings_path)
            .unwrap()
            .contains_key("MCP_PROXY_KEY")
    );
    assert!(!r.text.contains("proxy-access"));
}

#[tokio::test]
async fn one_update_cant_name_a_password_and_write_it() {
    // The check is built from the settings the update leaves behind: naming a password
    // and writing its value in the same PUT is refused, and names compare without case.
    let w = World::new();
    let c = w.client();
    std::fs::write(&w.settings_path, "GHOST_REPOS=acme/app\n").unwrap();
    let value = "sk-the-value-0123456789";
    for values in [
        json!({"GHOST_LLM_PASSWORD": "NEW_ONE", "NEW_ONE": value}),
        json!({"GHOST_APP_SECRETS": "PLAIN_NAME", "PLAIN_NAME": value}),
        json!({"openai_api_key": value}),
        json!({"smtp_pass": value}),
        json!({"DB_PASSWD": value}),
        json!({"GOOGLE_CREDENTIALS": value}),
    ] {
        let r = c.put("/settings", json!({"values": values})).await;
        assert_eq!(r.status, 422, "{values}");
        assert!(!r.text.contains(value));
    }
    let env = read_env(&w.settings_path).unwrap();
    assert_eq!(env.len(), 1);
    assert_eq!(env["GHOST_REPOS"], "acme/app");
    // Naming one alone is a setting like any other.
    let r = c
        .put(
            "/settings",
            json!({"values": {"GHOST_LLM_PASSWORD": "NEW_ONE"}}),
        )
        .await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn no_validation_error_echoes_what_was_sent() {
    // FastAPI's 422 carries each error's input; Ghost's says where and why only.
    let w = World::new();
    let c = w.client();
    let typed = format!("sk-typed-{}", "k".repeat(5000));
    let r = c
        .post(
            "/model/models",
            Some(json!({"values": {}, "password_value": typed})),
        )
        .await;
    assert_eq!(r.status, 422);
    assert!(!r.text.contains("sk-typed-"));
    assert_eq!(r["detail"][0]["loc"], json!(["body", "password_value"]));
    let r = c
        .put(
            "/settings",
            json!({"secrets": {"GITHUB_TOKEN": ["ghp_inside_a_list_0123"]}}),
        )
        .await;
    assert_eq!(r.status, 422);
    assert!(!r.text.contains("ghp_inside_a_list"));
    let r = c
        .put(
            "/passwords/A_TOKEN",
            json!({"value": {"nested": "ghp_nested_value_0123"}}),
        )
        .await;
    assert_eq!(r.status, 422);
    assert!(!r.text.contains("ghp_nested_value"));
    let keys: std::collections::BTreeSet<String> = r["detail"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        ["type".to_string(), "loc".to_string(), "msg".to_string()]
            .into_iter()
            .collect()
    );
}
