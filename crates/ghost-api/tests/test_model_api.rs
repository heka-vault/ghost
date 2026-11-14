//! The model connection's routes (ADR 0036): the view, saving through PUT /settings with a
//! Password that goes live, the model list, Test connection and OpenRouter's sign-in. No
//! key ever comes back out.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::{FakeModelNet, FakeSignIns, World, by};
use ghost_api::controller::ModelCheckRun;
use ghost_core::apps::AppError;
use ghost_core::connection::{ConnectionError, ModelOption};
use ghost_core::passwords::{MemoryKeyring, Passwords};
use ghost_llm::check::{ModelCheck, State, Timing, Verdict};
use serde_json::{Value, json};

const OR_KEY: &str = "sk-or-v1-fedcba9876543210";
const OPENAI_KEY: &str = "sk-openai-0123456789";
const TYPED_KEY: &str = "sk-typed-but-not-saved";

fn option(id: &str, name: &str) -> ModelOption {
    ModelOption {
        id: id.into(),
        name: name.into(),
    }
}

#[tokio::test]
async fn the_setup_from_before_shows_as_openrouter_without_its_key() {
    let w = World::new();
    w.set_env("GHOST_LLM_ENDPOINT", "openrouter");
    w.set_env("OPENROUTER_API_KEY", OR_KEY);
    let c = w.client();
    let r = c.get("/model").await;
    assert_eq!(r.status, 200);
    let v = &r.json;
    assert_eq!(
        (
            v["provider"].clone(),
            v["protocol"].clone(),
            v["wire_model"].clone()
        ),
        (
            json!("openrouter"),
            json!("anthropic"),
            json!("anthropic/claude-sonnet-5.5")
        )
    );
    assert_eq!(v["password"], "OPENROUTER_API_KEY");
    assert_eq!(v["password_set"], true);
    assert_eq!(v["legacy"], "GHOST_LLM_ENDPOINT=openrouter");
    assert_eq!(v["ready"], true);
    assert_eq!(v["values"], json!({"GHOST_LLM_PROVIDER": "openrouter"}));
    let ids: std::collections::BTreeSet<String> =
        common::strings(&v["providers"], "id").into_iter().collect();
    for id in ["anthropic", "openrouter", "ollama", "custom"] {
        assert!(ids.contains(id), "{id}");
    }
    assert!(!r.text.contains(OR_KEY));
}

#[tokio::test]
async fn a_saved_cliproxy_provider_is_reported_and_saved_as_your_own_address() {
    // A settings.env from before the named provider was removed (ADR 0037, amended
    // 2026-11-14): it keeps working, read as the custom connection it always was.
    let w = World::new();
    w.set_env("GHOST_LLM_PROVIDER", "cliproxy");
    w.set_env("CLIPROXY_BASE_URL", "http://proxy.lan:8317");
    w.set_env("ANTHROPIC_AUTH_TOKEN", OPENAI_KEY);
    let c = w.client();
    let v = c.get("/model").await;
    assert_eq!(v.status, 200);
    assert_eq!(v["provider"], "custom");
    assert_eq!(v["protocol"], "anthropic");
    assert_eq!(v["base_url"], "http://proxy.lan:8317");
    assert_eq!(v["wire_model"], "claude-sonnet-5-5");
    assert_eq!(v["password"], "ANTHROPIC_AUTH_TOKEN");
    assert_eq!(v["password_set"], true);
    assert_eq!(v["legacy"], Value::Null, "silent");
    assert_eq!(v["ready"], true, "{}", v.text);
    let ids = common::strings(&v["providers"], "id");
    assert!(
        !ids.iter().any(|id| id == "cliproxy"),
        "never offered: {ids:?}"
    );
    assert!(!v.text.to_lowercase().contains("cliproxy"), "{}", v.text);
    let values = json!({
        "GHOST_LLM_PROVIDER": "custom",
        "GHOST_LLM_PROTOCOL": "anthropic",
        "GHOST_LLM_BASE_URL": "http://proxy.lan:8317",
        "GHOST_LLM_PASSWORD": "ANTHROPIC_AUTH_TOKEN",
    });
    assert_eq!(v["values"], values);
    // What the page saves is those values: the same route, without the retired name.
    let r = c.put("/settings", json!({ "values": values })).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let saved = std::fs::read_to_string(&w.settings_path).unwrap();
    assert!(!saved.contains("cliproxy"), "{saved}");
    assert!(saved.contains("GHOST_LLM_PROVIDER=custom"), "{saved}");
    let after = c.get("/model").await;
    for k in [
        "provider",
        "protocol",
        "base_url",
        "wire_model",
        "password",
        "ready",
    ] {
        assert_eq!(after[k], v[k], "{k}");
    }
    assert!(!after.text.contains(OPENAI_KEY));
}

#[tokio::test]
async fn a_key_in_the_vault_counts_without_coming_out() {
    let w = World::new();
    w.set_env("GHOST_LLM_PROVIDER", "openrouter");
    w.vault.set("OPENROUTER_API_KEY", OR_KEY).unwrap();
    let c = w.client();
    let r = c.get("/model").await;
    assert_eq!(r["password_set"], true);
    assert_eq!(r["ready"], true);
    assert!(!r.text.contains(OR_KEY));
    // Read for a view, it isn't a use: "last used" stays empty.
    let meta = w.vault.info();
    let m = meta
        .iter()
        .find(|m| m.name == "OPENROUTER_API_KEY")
        .unwrap();
    assert!(m.last_used.is_none());
}

#[tokio::test]
async fn saving_a_connection_and_its_password_is_live() {
    let w = World::new();
    let c = w.client();
    let r = c
        .put(
            "/settings",
            json!({"values": {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-5-mini"}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    // The key is a password like any other, saved on its own into the vault (ADR 0036).
    let r = c
        .put("/passwords/OPENAI_API_KEY", json!({"value": OPENAI_KEY}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["restart_required"], false); // the next session reads it by name
    assert!(!r.text.contains(OPENAI_KEY));
    assert_eq!(
        w.vault.value("OPENAI_API_KEY").unwrap().as_str(),
        OPENAI_KEY
    );
    assert!(!w.env_path.parent().unwrap().join("apps.env").exists());
    assert!(!w.env_path.exists());
    let v = c.get("/model").await;
    assert_eq!(
        (
            v["provider"].clone(),
            v["model"].clone(),
            v["password_set"].clone()
        ),
        (json!("openai"), json!("gpt-5-mini"), json!(true))
    );
    let listed = c.get("/passwords").await;
    let rows = by(&listed["passwords"], "name");
    assert_eq!(rows["OPENAI_API_KEY"]["set"], true);
    assert_eq!(rows["OPENAI_API_KEY"]["ghost"], true);
    assert_eq!(
        rows["OPENAI_API_KEY"]["used_by"],
        json!(["The model connection"])
    );
}

#[tokio::test]
async fn the_passwords_page_lists_only_the_model_keys_in_play() {
    let w = World::new();
    let c = w.client();
    let names = common::strings(&c.get("/passwords").await["passwords"], "name");
    // Nothing set up: Anthropic is the provider, so its key is listed; the others wait.
    assert!(names.contains(&"ANTHROPIC_API_KEY".to_string()));
    assert!(!names.contains(&"OPENAI_API_KEY".to_string()));
    assert!(!names.contains(&"LLM_API_KEY".to_string()));
    assert!(names.contains(&"OPENROUTER_API_KEY".to_string())); // Jev's too
}

#[tokio::test]
async fn jev_still_wants_a_restart_for_openrouters_key() {
    let w = World::new();
    let c = w.client();
    let r = c
        .put("/passwords/OPENROUTER_API_KEY", json!({"value": OR_KEY}))
        .await;
    assert_eq!(r["restart_required"], true);
}

#[tokio::test]
async fn a_password_the_connection_doesnt_name_is_refused() {
    let w = World::new();
    let c = w.client();
    let r = c
        .put("/settings", json!({"secrets": {"SOMEBODY_ELSES_KEY": "x"}}))
        .await;
    assert_eq!(r.status, 422);
}

#[tokio::test]
async fn a_custom_password_name_is_the_models_once_named() {
    let w = World::new();
    let c = w.client();
    let values = json!({
        "GHOST_LLM_PROVIDER": "custom",
        "GHOST_LLM_BASE_URL": "http://gpu-box:8000/v1",
        "GHOST_LLM_PROTOCOL": "openai",
        "GHOST_LLM_PASSWORD": "GPU_BOX_KEY",
    });
    assert_eq!(
        c.put("/settings", json!({"values": values})).await.status,
        200
    );
    let r = c
        .put("/passwords/GPU_BOX_KEY", json!({"value": "k-0123456789"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["restart_required"], false);
    let rows = by(&r["passwords"]["passwords"], "name");
    assert_eq!(
        rows["GPU_BOX_KEY"]["used_by"],
        json!(["The model connection"])
    );
    assert_eq!(c.get("/model").await["password_set"], true);
}

#[tokio::test]
async fn the_model_list_reads_the_saved_key_from_the_vault() {
    let mut w = World::new();
    w.vault.set("OPENAI_API_KEY", OPENAI_KEY).unwrap();
    w.model_net = Some(Arc::new(FakeModelNet::listing(Arc::new(|conn, env| {
        assert_eq!(
            env.get(&conn.password).map(String::as_str),
            Some(OPENAI_KEY)
        ); // from the vault, by name
        Ok(vec![option("gpt-5", "")])
    }))));
    let c = w.client();
    let r = c
        .post(
            "/model/models",
            Some(json!({"values": {"GHOST_LLM_PROVIDER": "openai"}})),
        )
        .await;
    assert_eq!(
        r["models"],
        json!([{"id": "gpt-5", "name": null}]),
        "{}",
        r.text
    );
    assert!(!r.text.contains(OPENAI_KEY));
    let meta = w.vault.info();
    let m = meta.iter().find(|m| m.name == "OPENAI_API_KEY").unwrap();
    assert!(m.last_used.is_some()); // a list is a use
}

#[tokio::test]
async fn the_model_list_for_a_draft() {
    let mut w = World::new();
    w.model_net = Some(Arc::new(FakeModelNet::listing(Arc::new(|conn, env| {
        assert_eq!(env.get(&conn.password).map(String::as_str), Some(TYPED_KEY)); // the typed key, for this request only
        Ok(vec![
            option("gpt-5", ""),
            option("gpt-5-mini", "GPT-5 mini"),
        ])
    }))));
    let c = w.client();
    let r = c
        .post(
            "/model/models",
            Some(json!({"values": {"GHOST_LLM_PROVIDER": "openai"}, "password_value": TYPED_KEY})),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json,
        json!({
            "models": [{"id": "gpt-5", "name": null}, {"id": "gpt-5-mini", "name": "GPT-5 mini"}],
            "default": "gpt-5",
            "error": null,
        })
    );
    assert!(!r.text.contains(TYPED_KEY));
}

#[tokio::test]
async fn a_model_list_that_cant_be_read_says_why() {
    let mut w = World::new();
    w.model_net = Some(Arc::new(FakeModelNet::listing(Arc::new(|_conn, _env| {
        Err(ConnectionError::Unreachable {
            host: "http://localhost:11434".into(),
            kind: "ConnectError".into(),
        })
    }))));
    let c = w.client();
    let r = c
        .post(
            "/model/models",
            Some(json!({"values": {"GHOST_LLM_PROVIDER": "ollama"}})),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r["models"], json!([]));
    assert!(r["error"].as_str().unwrap().contains("couldn’t reach"));
}

#[tokio::test]
async fn a_draft_takes_only_connection_settings() {
    let w = World::new();
    let c = w.client();
    let r = c
        .post(
            "/model/models",
            Some(json!({"values": {"GHOST_WAKE_PHRASES": "x"}})),
        )
        .await;
    assert_eq!(r.status, 422);
}

fn found(role: &str, model: &str, first_word: &[f64]) -> ModelCheck {
    let yes = Verdict {
        state: State::Yes,
        detail: None,
    };
    ModelCheck {
        role: role.into(),
        model: model.into(),
        answered_by: Some(format!("{model}-2026")),
        reachable: true,
        error: None,
        first_byte: Timing::of(&[0.3]),
        first_word: Timing::of(first_word),
        samples: first_word.len(),
        streaming: yes.clone(),
        tools: yes.clone(),
        caching: Verdict {
            state: State::NotReported,
            detail: Some("the provider doesn't report it".into()),
        },
        thinking: Verdict {
            state: State::No,
            detail: Some("refuses between_tools".into()),
        },
        thinking_mode: Some("between_tools".into()),
    }
}

#[tokio::test]
async fn the_check_reports_each_model_with_the_key_used_and_never_shown() {
    let w = World::new();
    type Seen = Option<HashMap<String, String>>;
    let seen: Arc<std::sync::Mutex<Seen>> = Arc::new(std::sync::Mutex::new(None));
    let seen2 = seen.clone();
    w.control.set_check(Arc::new(move |env| {
        *seen2.lock().unwrap() = Some(env);
        ModelCheckRun {
            at: chrono::Utc::now(),
            models: vec![
                found("main", "gpt-5", &[0.4123, 0.9, 0.5]),
                found("fast", "gpt-5-mini", &[0.2]),
            ],
            advice: None,
        }
    }));
    let c = w.client();
    let r = c
        .post(
            "/model/check",
            Some(json!({
                "values": {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-5"},
                "password_value": TYPED_KEY,
            })),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let v = &r.json;
    let main = &v["models"][0];
    assert_eq!(main["role"], "main");
    assert_eq!(main["reachable"], true);
    assert_eq!(
        main["first_word"],
        json!({"median_ms": 500, "worst_ms": 900})
    );
    assert_eq!(main["samples"], 3);
    assert_eq!(main["answered_by"], "gpt-5-2026");
    assert_eq!(main["thinking"]["state"], "no");
    assert_eq!(main["caching"]["state"], "not_reported");
    assert_eq!(v["models"][1]["role"], "fast");
    assert!(
        v["where"]
            .as_str()
            .unwrap()
            .starts_with("openai (openai, https://api.openai.com")
    );
    let env = seen.lock().unwrap().clone().unwrap();
    assert_eq!(env["OPENAI_API_KEY"], TYPED_KEY);
    assert!(!r.text.contains(TYPED_KEY));
    assert!(!w.env().contains_key("OPENAI_API_KEY")); // tried, not kept
}

#[tokio::test]
async fn a_failed_check_says_why_in_plain_words() {
    let w = World::new();
    w.control.set_check(Arc::new(|_env| {
        let mut m = found("main", "claude-sonnet-5-5", &[]);
        m.reachable = false;
        m.error = Some("The model provider refused Ghost's key. (HTTP 401)".into());
        ModelCheckRun {
            at: chrono::Utc::now(),
            models: vec![m],
            advice: None,
        }
    }));
    let c = w.client();
    let v = c
        .post(
            "/model/check",
            Some(json!({"values": {"GHOST_LLM_PROVIDER": "anthropic"}})),
        )
        .await;
    assert_eq!(v["models"][0]["reachable"], false);
    assert!(
        v["models"][0]["error"]
            .as_str()
            .unwrap()
            .contains("refused Ghost's key")
    );
}

#[tokio::test]
async fn no_check_yet_is_none() {
    let w = World::new();
    let v = w.client().get("/model/check").await;
    assert_eq!(v.status, 200);
    assert_eq!(v.json["check"], Value::Null);
}

/// `GET /model` until the sign-in is no longer pending.
async fn settled(c: &common::TestClient) -> Value {
    for _ in 0..100 {
        let v = c.get("/model").await.json;
        if v["sign_in_pending"] == false {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the sign-in never settled");
}

#[tokio::test]
async fn signing_in_to_openrouter_keeps_the_key_as_a_password() {
    let mut w = World::new();
    w.set_env("GHOST_LLM_PROVIDER", "openrouter");
    let signins = Arc::new(FakeSignIns::new());
    w.signins = Some(signins.clone());
    let net = Arc::new(FakeModelNet::new());
    w.model_net = Some(net.clone());
    let c = w.client();
    let r = c.post("/model/sign-in", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let url = r["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("https://openrouter.ai/auth?"));
    assert!(url.contains("callback_url=http%3A%2F%2Flocalhost%3A47823%2Fcallback"));
    assert!(url.contains("code_challenge_method=S256"));
    let v = settled(&c).await;
    assert_eq!(net.traded(), vec!["the-code".to_string()]);
    assert_eq!(
        w.vault.value("OPENROUTER_API_KEY").unwrap().as_str(),
        OR_KEY
    ); // in the vault (ADR 0036)
    assert!(!w.env_path.exists());
    assert_eq!(v["sign_in_error"], Value::Null);
    assert_eq!(v["password_set"], true);
    assert!(!r.text.contains(OR_KEY));
}

#[tokio::test]
async fn a_sign_in_that_comes_back_for_another_request_is_refused() {
    let mut w = World::new();
    w.signins = Some(Arc::new(FakeSignIns::answering(Arc::new(|| {
        let mut q = HashMap::new();
        q.insert("code".to_string(), "c".to_string());
        q.insert("state".to_string(), "not-ours".to_string());
        Ok(q)
    }))));
    w.model_net = Some(Arc::new(FakeModelNet::new()));
    let c = w.client();
    c.post("/model/sign-in", None).await;
    let v = settled(&c).await;
    assert!(
        v["sign_in_error"]
            .as_str()
            .unwrap_or("")
            .contains("another request")
    );
    assert!(!w.vault.contains("OPENROUTER_API_KEY"));
}

#[tokio::test]
async fn no_sign_in_while_the_vault_is_locked() {
    let mut w = World::new();
    let signins = Arc::new(FakeSignIns::new());
    w.signins = Some(signins.clone());
    w.vault.set("GITHUB_TOKEN", "ghp_0123456789").unwrap(); // a vault on disk to open
    let locked = Passwords::new(w.vault.path(), Arc::new(MemoryKeyring::locked()));
    locked.open();
    w.passwords = Some(Arc::new(locked));
    let c = w.client();
    let r = c.post("/model/sign-in", None).await;
    assert_eq!(r.status, 423);
    assert!(r.detail().contains("Unlock"));
    assert!(signins.urls().is_empty()); // nobody was sent to sign in
}

#[tokio::test]
async fn no_list_is_asked_for_without_the_key() {
    let mut w = World::new();
    w.model_net = Some(Arc::new(FakeModelNet::listing(Arc::new(|_c, _e| {
        panic!("asked without a key")
    }))));
    let c = w.client();
    let r = c
        .post(
            "/model/models",
            Some(json!({"values": {"GHOST_LLM_PROVIDER": "anthropic"}})),
        )
        .await;
    assert_eq!(r["error"], "Save its password to see the models it offers.");
}

#[tokio::test]
async fn a_locked_keyring_is_said_not_a_missing_key() {
    // While the vault can't be read the environment has no key; that is not "not set".
    let mut w = World::new();
    w.set_env("GHOST_LLM_PROVIDER", "openrouter");
    w.vault.set("OPENROUTER_API_KEY", OR_KEY).unwrap();
    // The vault put the key in the daemon's environment; a daemon whose keyring was
    // locked at start never had it.
    w.environ.lock().unwrap().remove("OPENROUTER_API_KEY");
    let locked = Passwords::new(w.vault.path(), Arc::new(MemoryKeyring::locked()));
    locked.open();
    w.passwords = Some(Arc::new(locked));
    w.model_net = Some(Arc::new(FakeModelNet::listing(Arc::new(|_c, _e| {
        panic!("asked without a key")
    }))));
    let c = w.client();
    let v = c.get("/model").await;
    assert_eq!(v["ready"], false);
    assert_eq!(v["password_set"], false);
    assert_eq!(v["problems"].as_array().unwrap().len(), 1, "{}", v.text);
    let problem = v["problems"][0].as_str().unwrap();
    assert!(problem.contains("keyring is locked") && !problem.contains("not set"));
    let r = c
        .post(
            "/model/models",
            Some(json!({"values": {"GHOST_LLM_PROVIDER": "anthropic"}})),
        )
        .await;
    assert!(
        r["error"]
            .as_str()
            .unwrap()
            .starts_with("Unlock your keyring")
    );
}

/// The sign-in's refusal, as a `FakeSignIns` may answer it.
#[allow(dead_code)]
fn refused() -> Result<HashMap<String, String>, AppError> {
    Err(AppError::NeedsSignIn("the sign-in was replaced".into()))
}

// --- what Ghost really uses (ADR 0037): the environment it was started in counts ---------

const SHELL_TOKEN: &str = "sk-shell-0123456789abcdef";

#[tokio::test]
async fn a_connection_from_the_environment_ghost_was_started_in_is_the_one_reported() {
    // Started from a shell with a provider's address and key exported: Ghost answers with
    // them, so the desktop must not say it needs a model.
    let w = World::new();
    w.set_started("ANTHROPIC_BASE_URL", "http://proxy.lan:8317");
    w.set_started("ANTHROPIC_AUTH_TOKEN", SHELL_TOKEN);
    let v = w.client().get("/model").await;
    assert_eq!(v["ready"], true, "{}", v.text);
    assert_eq!(v["password_set"], true);
    assert_eq!(v["base_url"], "http://proxy.lan:8317");
    // Named, never shown.
    assert_eq!(
        v["from_environment"],
        json!(["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"])
    );
    assert!(!v.text.contains(SHELL_TOKEN));
}

#[tokio::test]
async fn what_settings_env_or_the_vault_holds_is_not_from_the_environment() {
    let w = World::new();
    // The launcher loads settings.env into the environment Ghost starts in.
    std::fs::write(
        &w.settings_path,
        "ANTHROPIC_BASE_URL=http://proxy.lan:8317\n",
    )
    .unwrap();
    w.set_started("ANTHROPIC_BASE_URL", "http://proxy.lan:8317");
    w.set_started("ANTHROPIC_AUTH_TOKEN", SHELL_TOKEN);
    w.vault
        .set("ANTHROPIC_AUTH_TOKEN", "sk-vault-0123456789")
        .unwrap();
    let v = w.client().get("/model").await;
    assert_eq!(v["ready"], true, "{}", v.text);
    assert_eq!(v["from_environment"], json!([]));
}

#[tokio::test]
async fn a_setting_in_settings_env_that_ghost_is_not_using_is_not_reported() {
    // settings.env edited by hand after the start: the daemon reads it at its next start,
    // so the page says what Ghost uses now.
    let w = World::new();
    std::fs::write(&w.settings_path, "GHOST_LLM_PROVIDER=openai\n").unwrap();
    let v = w.client().get("/model").await;
    assert_eq!(v["provider"], "anthropic", "{}", v.text);
}

#[tokio::test]
async fn a_setting_cleared_on_the_page_is_cleared_even_when_ghost_started_with_it() {
    let w = World::new();
    w.set_started("GHOST_LLM_PROVIDER", "openai");
    w.set_started("GHOST_LLM_MODEL", "gpt-5-mini");
    let c = w.client();
    assert_eq!(c.get("/model").await["model"], "gpt-5-mini");
    let r = c
        .put("/settings", json!({"values": {"GHOST_LLM_MODEL": null}}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let v = c.get("/model").await;
    assert_eq!(v["model"], "", "{}", v.text);
    assert_eq!(v["from_environment"], json!(["GHOST_LLM_PROVIDER"]));
}

#[tokio::test]
async fn an_older_setting_the_connection_does_not_read_is_not_named() {
    // ANTHROPIC_BASE_URL is read only when no provider is named.
    let w = World::new();
    w.set_started("ANTHROPIC_BASE_URL", "http://proxy.lan:8317");
    w.set_env("GHOST_LLM_PROVIDER", "openai");
    let v = w.client().get("/model").await;
    assert_eq!(v["provider"], "openai");
    assert_eq!(v["from_environment"], json!([]));
}
