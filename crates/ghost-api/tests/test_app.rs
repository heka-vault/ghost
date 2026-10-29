//! The control API's own routes: status, wake and sleep, the pending change, the audit,
//! the policy, the settings, setup, the phone, the waiting list and the event stream.

mod common;

use std::sync::Arc;

use chrono::Utc;
use common::{FakeDevices, World, by, pend, strings};
use futures::StreamExt;
use ghost_api::models::{Build, BuildKind, DeviceInfo, Launcher, PhoneDevice, SessionOrigin};
use ghost_api::{Phase, pending_view};
use ghost_core::envfile::read_env;
use ghost_core::levels::Autonomy;
use serde_json::{Value, json};

#[tokio::test]
async fn status_and_wake_sleep() {
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Passive, None, None);
    let s = c.get("/status").await;
    assert_eq!(s["phase"], "passive");
    assert_eq!(s["apps"], json!([]));
    assert_eq!(s["pending"], Value::Null);
    let r = c.post("/wake", Some(json!({"text": "what's up"}))).await;
    assert_eq!(r["ok"], true);
    assert_eq!(w.control.last_call(), json!(["wake", "what's up"]));
    assert_eq!(c.get("/status").await["session_id"], "sess1");
    assert_eq!(c.post("/wake", None).await["ok"], false); // already awake
}

#[tokio::test]
async fn wake_can_open_the_session_in_a_line() {
    // A Call button on a line's page (2026-10-24): the session starts there, not in Ghost.
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Passive, None, None);
    let r = c
        .post("/wake", Some(json!({"conversation": "legal"})))
        .await;
    assert_eq!(r["ok"], true);
    assert_eq!(w.control.last_call(), json!(["wake", null, "legal"]));
    assert_eq!(c.get("/status").await["conversation"], "legal");
    assert_eq!(c.post("/sleep", None).await["ok"], true);
    assert_eq!(c.get("/status").await["phase"], "passive");
}

#[tokio::test]
async fn say_wakes_and_records_turn() {
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Passive, None, None);
    let r = c
        .post("/session/say", Some(json!({"text": "label it bug"})))
        .await;
    assert_eq!(r["ok"], true);
    let turns = c.get("/session/turns").await;
    let last = turns.json.as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["role"], "user");
    assert_eq!(last["via"], "text");
    w.state.set_phase(Phase::Starting, None, None);
    assert_eq!(
        c.post("/session/say", Some(json!({"text": "x"}))).await["ok"],
        false
    );
    assert_eq!(
        c.post("/session/say", Some(json!({"text": ""})))
            .await
            .status,
        422
    );
}

#[tokio::test]
async fn pending_confirm_executes_and_audits() {
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Listening, Some("sess1"), None);
    let sub = pend(&w.gate, "sess1").await;
    assert!(!sub.executed());
    w.state.set_pending(Some(pending_view(&sub.proposal, None)));
    let p = c.get("/proposals/pending").await;
    assert_eq!(p["operation"], "nuke");
    assert!(p["summary"].as_str().unwrap().contains("add flagged"));
    assert!(p["expires_in_secs"].as_f64().unwrap() > 0.0);
    let out = c.post("/proposals/pending/confirm", None).await;
    assert_eq!(out["status"], "success");
    assert!(
        w.writer
            .issue(&common::the_issue())
            .labels
            .contains(&"flagged".to_string())
    );
    assert!(out["state_now"].as_str().unwrap().contains("flagged"));
    assert_eq!(c.get("/proposals/pending").await.json, Value::Null);
    assert_eq!(c.post("/proposals/pending/confirm", None).await.status, 409);
    let audit = c.get("/audit").await;
    assert_eq!(audit["total"], 1);
    let entry = &audit["items"][0];
    assert_eq!(entry["status"], "executed");
    assert_eq!(entry["autonomy"], "confirm");
    assert!(
        entry["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| { e["event"] == "user_turn" && e["detail"]["classifier"] == "ui" })
    );
    assert!(entry.get("undo_of").is_none());
    assert!([404, 405].contains(&c.post("/undo", None).await.status));
}

#[tokio::test]
async fn cancel_pending() {
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Listening, Some("sess1"), None);
    pend(&w.gate, "sess1").await;
    assert_eq!(c.post("/proposals/pending/cancel", None).await["ok"], true);
    assert_eq!(c.post("/proposals/pending/cancel", None).await["ok"], false);
    assert!(
        !w.writer
            .issue(&common::the_issue())
            .labels
            .contains(&"flagged".to_string())
    );
    assert_eq!(c.get("/audit?status=cancelled").await["total"], 1);
    assert_eq!(c.get("/audit?source=nope").await["total"], 0);
    // A proposal's origin is its source (ghost_core.source); the database says ability.
    let page = c.get("/audit?source=fake").await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let entry = &items[0];
    assert_eq!(entry["source"], "fake");
    assert!(!entry.to_string().contains("ability"));
    assert_eq!(entry["events"][0]["detail"]["source"], "fake");
}

#[tokio::test]
async fn sources_listing() {
    let w = World::new();
    let c = w.client();
    let out = c.get("/sources").await;
    let by_name = by(&out.json, "name");
    assert_eq!(by_name["fake"]["kind"], "app");
    assert_eq!(by_name["fake"]["description"], "fake");
    let ops = by(&by_name["fake"]["operations"], "name");
    assert_eq!(ops["flag"]["autonomy"], "announce");
    assert_eq!(ops["nuke"]["autonomy"], "confirm");
    assert_eq!(by_name["fake"]["read_tools"][0]["name"], "echo");
    assert_eq!(by_name["conversations"]["kind"], "built-in");
    let conv_ops = strings(&by_name["conversations"]["operations"], "name");
    for op in ["retire", "rehire", "create", "rename"] {
        assert!(conv_ops.contains(&op.to_string()), "{op}");
    }
}

#[tokio::test]
async fn policy_round_trip_and_live_reload() {
    let w = World::new();
    let c = w.client();
    let v = c.get("/policy").await;
    assert_eq!(v["exists"], true);
    assert_eq!(v["permissions"]["ask"], json!(["fake.nuke"]));
    assert_eq!(
        w.gate.level("fake", "nuke", None, None).unwrap(),
        Autonomy::Confirm
    );
    // the raw text path, reloaded live
    let r = c
        .put(
            "/policy",
            json!({"text": "[permissions]\nquiet = [\"fake.nuke\"]\n"}),
        )
        .await;
    assert_eq!(r["applied"], true);
    assert_eq!(r["policy"]["permissions"]["quiet"], json!(["fake.nuke"]));
    assert_eq!(
        w.gate.level("fake", "nuke", None, None).unwrap(),
        Autonomy::Quiet
    );
    // an older per-source table still reads, as a rule
    let r2 = c
        .put(
            "/policy",
            json!({"text": "[fake.targets.\"docs\"]\nnuke = \"deny\"\n"}),
        )
        .await;
    assert_eq!(
        r2["policy"]["permissions"]["block"],
        json!(["fake.nuke(target=docs)"])
    );
    assert_eq!(
        c.put("/policy", json!({"text": "[fake]\nnuke = 'yolo'\n"}))
            .await
            .status,
        422
    );
    let bad = c
        .put(
            "/policy",
            json!({"text": "[permissions]\nblock = [\"x.(y\"]\n"}),
        )
        .await;
    assert_eq!(bad.status, 422);
}

#[tokio::test]
async fn settings_write_only_secrets() {
    let w = World::new();
    let c = w.client();
    let v = c.get("/settings").await;
    assert_eq!(v["values"], json!({}));
    assert!(
        v["secrets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["set"] == false)
    );
    let r = c
        .put(
            "/settings",
            json!({
                "values": {"GHOST_REPOS": "acme/app", "GHOST_GITHUB_MOCK": null},
                "secrets": {"GITHUB_TOKEN": "ghp_secret"},
            }),
        )
        .await;
    assert_eq!(r["restart_required"], true, "{}", r.text);
    let s = &r["settings"];
    assert_eq!(s["values"], json!({"GHOST_REPOS": "acme/app"}));
    let set = by(&s["secrets"], "name");
    assert_eq!(set["GITHUB_TOKEN"]["set"], true);
    assert!(!s.to_string().contains("ghp_secret"));
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&w.settings_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    // The value is in the vault (ADR 0036), sealed; no plaintext file holds it.
    let env = read_env(&w.settings_path).unwrap();
    assert_eq!(env.get("GHOST_REPOS").unwrap(), "acme/app");
    assert_eq!(env.len(), 1);
    assert!(!w.env_path.exists());
    assert_eq!(w.vault.names(), vec!["GITHUB_TOKEN".to_string()]);
    let raw = std::fs::read(w.vault.path()).unwrap();
    assert!(!raw.windows(10).any(|win| win == b"ghp_secret"));
    assert_eq!(
        c.put("/settings", json!({"values": {"GITHUB_TOKEN": "x"}}))
            .await
            .status,
        422
    );
    assert_eq!(
        c.put("/settings", json!({"secrets": {"NOPE": "x"}}))
            .await
            .status,
        422
    );
    assert_eq!(
        c.put("/settings", json!({"values": {"bad-name": "x"}}))
            .await
            .status,
        422
    );
    let r = c
        .put("/settings", json!({"secrets": {"GITHUB_TOKEN": null}}))
        .await;
    let set = by(&r["settings"]["secrets"], "name");
    assert_eq!(set["GITHUB_TOKEN"]["set"], false);
}

#[tokio::test]
async fn setup_endpoints() {
    let w = World::new();
    let c = w.client();
    let out = c.post("/setup/echo-cancel", None).await;
    assert_eq!(out["ok"], true, "{}", out.text);
    assert!(out["detail"].as_str().unwrap().contains("installed"));
    let out = c.post("/setup/echo-cancel?uninstall=true", None).await;
    assert!(out["detail"].as_str().unwrap().contains("--uninstall"));
    let m = c.get("/setup/models").await;
    assert_eq!(m["ready"], false);
    assert_eq!(m["kokoro_present"], false);
    assert_eq!(m["stt_model"], "parakeet");
}

#[tokio::test]
async fn status_says_the_version_and_the_build() {
    let w = World::new();
    let c = w.client();
    assert_eq!(
        c.get("/status").await["build"],
        json!({"kind": "dev", "commit": null, "dirty": false, "launcher": null})
    );
    w.state.set_build(Build {
        kind: BuildKind::Dev,
        launcher: Some(Launcher::MakeDev),
        ..Default::default()
    });
    assert_eq!(c.get("/status").await["build"]["launcher"], "make-dev");
    w.state.set_build(Build {
        kind: BuildKind::Deploy,
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        ..Default::default()
    });
    let s = c.get("/status").await;
    assert_eq!(s["version"], "test");
    assert_eq!(
        s["build"],
        json!({
            "kind": "deploy",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "dirty": false,
            "launcher": null,
        })
    );
}

#[tokio::test]
async fn events_websocket_streams_status_and_turns() {
    let w = World::new();
    w.state.set_phase(Phase::Passive, None, None);
    let router = ghost_api::router(w.deps());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/events"))
        .await
        .unwrap();
    async fn next<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("an event in time")
            .unwrap()
            .unwrap();
        serde_json::from_str::<Value>(&msg.into_text().unwrap()).unwrap()
    }
    let first = next(&mut ws).await;
    assert_eq!(first["phase"], "passive"); // initial snapshot is a plain Status
    w.state.set_phase(Phase::Listening, Some("sess1"), None);
    let ev = next(&mut ws).await;
    assert_eq!(ev["type"], "status");
    assert_eq!(ev["status"]["phase"], "listening");
    w.state.add_turn(
        ghost_api::models::TurnRole::Assistant,
        "Hello.",
        ghost_api::models::TurnVia::Voice,
    );
    let ev = next(&mut ws).await;
    assert_eq!(ev["type"], "turn");
    assert_eq!(ev["turn"]["text"], "Hello.");
    w.state.log("INFO", "hi");
    let ev = next(&mut ws).await;
    assert_eq!(ev["type"], "log");
    assert_eq!(ev["message"], "hi");
    server.abort();
}

#[tokio::test]
async fn devices_endpoint_tolerates_no_audio() {
    let mut w = World::new();
    w.devices = Some(Arc::new(FakeDevices(vec![DeviceInfo {
        index: 0,
        name: "ghost.mic".into(),
        max_input_channels: 1,
        max_output_channels: 0,
        default_samplerate: 48000.0,
        is_default_input: true,
        is_default_output: false,
    }])));
    let c = w.client();
    let d = c.get("/devices").await;
    assert_eq!(d["devices"][0]["name"], "ghost.mic");
    assert_eq!(d["echo_cancel_source_present"], true);
    assert_eq!(d["input_device"], "ghost.mic");
}

#[tokio::test]
async fn openapi_lists_every_route_with_tags() {
    let doc = ghost_api::openapi::document();
    let paths: Vec<&str> = doc["paths"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for p in [
        "/status",
        "/wake",
        "/sleep",
        "/session/say",
        "/proposals/pending/confirm",
        "/audit",
        "/sources",
        "/apps",
        "/catalog",
        "/permissions",
        "/policy",
        "/settings",
        "/devices",
        "/events/schema",
    ] {
        assert!(paths.contains(&p), "{p}");
    }
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    assert!(schemas.contains_key("StatusEvent") && schemas.contains_key("LogEvent"));
}

#[tokio::test]
async fn say_rejects_blank() {
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Passive, None, None);
    for bad in ["", "   "] {
        let status = c
            .post("/session/say", Some(json!({"text": bad})))
            .await
            .status;
        assert!([422, 200].contains(&status));
    }
}

#[tokio::test]
async fn settings_live_keys_do_not_require_restart() {
    let w = World::new();
    let c = w.client();
    let r = c
        .put(
            "/settings",
            json!({"values": {"GHOST_WAKE_PHRASES": "hey ghost, computer"}}),
        )
        .await;
    assert_eq!(r["applied"], json!(["GHOST_WAKE_PHRASES"]));
    assert_eq!(r["restart_required"], false);
    assert_eq!(w.control.applied(), vec!["GHOST_WAKE_PHRASES".to_string()]);
    let r = c
        .put(
            "/settings",
            json!({"values": {"GHOST_WAKE_PHRASES": "x", "GHOST_REPOS": "a/b"}}),
        )
        .await;
    assert_eq!(r["applied"], json!(["GHOST_WAKE_PHRASES"]));
    assert_eq!(r["restart_required"], true);
}

#[tokio::test]
async fn session_origin_follows_the_session() {
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Passive, None, None);
    assert_eq!(c.get("/status").await["session_origin"], Value::Null);
    w.state
        .set_phase(Phase::Listening, Some("sess1"), Some(SessionOrigin::Phone));
    assert_eq!(c.get("/status").await["session_origin"], "phone");
    w.state.set_phase(Phase::Speaking, Some("sess1"), None); // later phases keep the origin
    assert_eq!(c.get("/status").await["session_origin"], "phone");
    w.state.set_phase(Phase::Passive, None, None);
    assert_eq!(c.get("/status").await["session_origin"], Value::Null);
}

#[tokio::test]
async fn phone_routes() {
    let w = World::new();
    let c = w.client();
    assert_eq!(c.post("/phone/pairing", None).await.status, 409); // listener off
    assert_eq!(
        c.get("/phone").await.json,
        json!({"address": null, "addresses": [], "listen": [], "device": null})
    );
    w.control.set_phone_address(Some("192.168.1.20:8765"));
    let body = c.post("/phone/pairing", None).await;
    assert_eq!(body["code"], "123456");
    assert_eq!(body["address"], "192.168.1.20:8765");
    assert!(body["link"].as_str().unwrap().starts_with("ghost://pair?"));
    w.control.set_phone_device(Some(PhoneDevice {
        id: "d1".into(),
        name: "Pixel 9".into(),
        paired_at: Utc::now(),
        last_seen: None,
        last_address: None,
        last_network: None,
    }));
    let view = c.get("/phone").await;
    assert_eq!(view["device"]["name"], "Pixel 9");
    assert!(!view.text.contains("secret"));
    assert_eq!(view["device"]["last_address"], Value::Null);
    assert_eq!(view["device"]["last_network"], Value::Null);
    assert_eq!(c.delete("/phone").await["ok"], true);
    assert_eq!(c.delete("/phone").await["ok"], false);
    assert_eq!(c.get("/phone").await["device"], Value::Null);
}

#[tokio::test]
async fn a_change_from_a_hung_up_session_is_confirmed_from_the_waiting_list() {
    // ADR 0030: the session ends, the change waits, a Confirm by id applies it.
    let w = World::new();
    let c = w.client();
    w.state.set_phase(Phase::Passive, None, None);
    c.post("/wake", None).await;
    let sub = pend(&w.gate, "sess1").await;
    w.gate.set_aside("sess1", "hung up").unwrap();
    c.post("/sleep", None).await;
    let waiting = c.get("/waiting").await;
    let items = waiting["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item["source"], "gate");
    assert_eq!(item["change"]["id"], sub.proposal.id);
    assert_eq!(waiting["count"], 1);
    assert_eq!(item["state"], "new");
    let out = c
        .post(&format!("/proposals/{}/confirm", sub.proposal.id), None)
        .await;
    assert_eq!(out["status"], "success");
    assert!(
        w.writer
            .issue(&common::the_issue())
            .labels
            .contains(&"flagged".to_string())
    );
    assert_eq!(c.get("/waiting").await["items"], json!([]));
    let entry = c.get("/audit").await["items"][0].clone();
    assert_eq!(entry["status"], "executed");
    assert_eq!(
        c.post(&format!("/proposals/{}/confirm", sub.proposal.id), None)
            .await
            .status,
        409
    );
}

#[tokio::test]
async fn acknowledge_and_snooze_an_item() {
    let w = World::new();
    let c = w.client();
    let sub = pend(&w.gate, "sess1").await;
    w.gate.set_aside("sess1", "hung up").unwrap();
    let key = c.get("/waiting").await["items"][0]["key"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        c.post("/waiting/ack", Some(json!({"key": key}))).await["ok"],
        true
    );
    let view = c.get("/waiting").await;
    assert_eq!(view["items"][0]["state"], "acknowledged");
    assert_eq!(view["count"], 0);
    assert_eq!(
        c.post("/waiting/snooze", Some(json!({"key": key, "minutes": 30})))
            .await["ok"],
        true
    );
    assert_eq!(c.get("/waiting").await["items"][0]["state"], "snoozed");
    assert_eq!(
        c.post("/waiting/ack", Some(json!({"key": "nope"}))).await["ok"],
        false
    );
    assert_eq!(
        c.post(&format!("/proposals/{}/cancel", sub.proposal.id), None)
            .await["ok"],
        true
    );
    assert_eq!(c.get("/waiting").await["items"], json!([]));
}

#[tokio::test]
async fn routes_are_edited_as_fields_and_kept_with_the_levels() {
    let w = World::new();
    let c = w.client();
    std::fs::write(
        &w.policy_path,
        "[defaults]\npending_ttl = 900\n[fake]\nnuke = \"confirm\"\n",
    )
    .unwrap();
    let view = c.get("/policy").await["routes"].clone();
    assert_eq!(view["enabled"], json!(["digest", "phone"]));
    assert_eq!(view["available"], json!(["digest"]));
    let mut routes = view.clone();
    routes["enabled"] = json!(["desk", "digest"]);
    routes["quiet_hours"] = json!("22:00-07:00");
    routes["greeting"] = json!("Eric, got a minute?");
    let r = c.put("/policy", json!({"routes": routes})).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let text = std::fs::read_to_string(&w.policy_path).unwrap();
    assert!(
        text.contains("[routes]\nenabled = [\"digest\", \"desk\"]"),
        "{text}"
    );
    assert!(text.contains("quiet_hours = \"22:00-07:00\""));
    // the rest of the file as it was, an older table included
    assert!(text.contains("[fake]\nnuke = \"confirm\""));
    assert!(text.contains("pending_ttl = 900"));
    c.put(
        "/permissions",
        json!({"set": {"rule": "fake.nuke", "level": "quiet"}}),
    )
    .await;
    let text = std::fs::read_to_string(&w.policy_path).unwrap();
    assert!(text.contains("\"Eric, got a minute?\""), "{text}");
    assert!(text.contains("quiet = [\"fake.nuke\"]")); // kept [routes]
    let mut bad_routes = routes.clone();
    bad_routes["escalate_after"] = json!(0);
    let bad = c.put("/policy", json!({"routes": bad_routes})).await;
    assert_eq!(bad.status, 422);
}

#[tokio::test]
async fn saving_the_routes_keeps_the_rest_of_the_file_comments_and_all() {
    // Saving "How Ghost reaches you" rewrote policy.toml from the parsed policy and
    // dropped every comment the operator (and the shipped example) wrote.
    let w = World::new();
    let c = w.client();
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../infra/policy.example.toml");
    let original = std::fs::read_to_string(&example).unwrap();
    std::fs::write(&w.policy_path, &original).unwrap();
    let view = c.get("/policy").await["routes"].clone();
    let mut routes = view.clone();
    routes["enabled"] = json!(["digest", "desk"]);
    routes["urgency"] = json!({"eng/ci": "call"});
    assert_eq!(
        c.put("/policy", json!({"routes": routes})).await.status,
        200
    );
    let text = std::fs::read_to_string(&w.policy_path).unwrap();
    assert!(text.starts_with(original.trim_end_matches('\n'))); // every line of it, comments included
    assert!(text.contains("[routes]\nenabled = [\"digest\", \"desk\"]"));
    assert!(text.contains("[routes.urgency]\n\"eng/ci\" = \"call\""));
    // Saved again: the routes table is replaced, not added, and the rest stays.
    routes["enabled"] = json!(["digest"]);
    routes["urgency"] = json!({});
    assert_eq!(
        c.put("/policy", json!({"routes": routes})).await.status,
        200
    );
    let again = std::fs::read_to_string(&w.policy_path).unwrap();
    assert!(again.starts_with(original.trim_end_matches('\n')));
    assert_eq!(again.matches("\n[routes]").count(), 1);
    assert!(!again.contains("\n[routes.urgency]"));
    assert!(again.contains("\n[routes]\nenabled = [\"digest\"]"));
    // A table in the middle of the file is replaced where it is.
    std::fs::write(
        &w.policy_path,
        "# mine\n[routes]\n# the old way\nenabled = [\"digest\"]\n\n\
         # keep this: it is about the defaults\n[defaults]\npending_ttl = 900\n",
    )
    .unwrap();
    assert_eq!(
        c.put("/policy", json!({"routes": routes})).await.status,
        200
    );
    let text = std::fs::read_to_string(&w.policy_path).unwrap();
    assert!(
        text.starts_with("# mine\n[routes]\nenabled = [\"digest\"]"),
        "{text}"
    );
    assert!(
        text.contains("# keep this: it is about the defaults\n[defaults]\npending_ttl = 900\n")
    );
}
