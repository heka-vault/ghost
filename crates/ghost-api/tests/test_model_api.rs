//! The model connection's routes (ADR 0036): the view, saving through PUT /settings with a
//! Password that goes live, the model list, Test connection and OpenRouter's sign-in. No
//! key ever comes back out.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::{FakeModelNet, FakeSignIns, World, by};
use ghost_api::controller::Probe;
use ghost_core::apps::AppError;
use ghost_core::connection::{ConnectionError, ModelOption};
use ghost_core::passwords::{MemoryKeyring, Passwords};
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

#[tokio::test]
async fn test_connection_times_the_first_token() {
    let w = World::new();
    type Seen = Option<(HashMap<String, String>, String, u32)>;
    let seen: Arc<std::sync::Mutex<Seen>> = Arc::new(std::sync::Mutex::new(None));
    let seen2 = seen.clone();
    w.control
        .set_probe(Arc::new(move |env, prompt, max_tokens| {
            *seen2.lock().unwrap() = Some((env, prompt, max_tokens));
            Probe {
                ttft: Some(0.4123),
                total: Some(0.61),
                text: "Hello there, nice to meet.".into(),
                model: "gpt-5-2026".into(),
                error: None,
            }
        }));
    let c = w.client();
    let r = c
        .post(
            "/model/test",
            Some(json!({
                "values": {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-5"},
                "password_value": TYPED_KEY,
            })),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let v = &r.json;
    assert_eq!(v["ok"], true);
    assert_eq!(
        (v["ttft_ms"].clone(), v["total_ms"].clone()),
        (json!(412), json!(610))
    );
    assert_eq!(v["model"], "gpt-5");
    assert_eq!(v["answered_by"], "gpt-5-2026");
    assert!(
        v["where"]
            .as_str()
            .unwrap()
            .starts_with("openai (openai, https://api.openai.com")
    );
    let (env, _, max_tokens) = seen.lock().unwrap().clone().unwrap();
    assert_eq!(env["OPENAI_API_KEY"], TYPED_KEY);
    assert!(max_tokens <= 64);
    assert!(!r.text.contains(TYPED_KEY));
    assert!(!w.env().contains_key("OPENAI_API_KEY")); // tried, not kept
}

#[tokio::test]
async fn a_failed_test_says_why() {
    let w = World::new();
    w.control.set_probe(Arc::new(|_env, _prompt, _max| Probe {
        ttft: None,
        total: None,
        text: String::new(),
        model: String::new(),
        error: Some("AuthenticationError (HTTP 401)".into()),
    }));
    let c = w.client();
    let v = c
        .post(
            "/model/test",
            Some(json!({"values": {"GHOST_LLM_PROVIDER": "anthropic"}})),
        )
        .await;
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], "AuthenticationError (HTTP 401)");
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
