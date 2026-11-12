//! The phone protocol's shared test vectors (docs/phone-protocol.md): a fixed secret, nonce
//! and time, and what this daemon's own signing code makes of them. The phone app's tests
//! (TypeScript in `apps/mobile`, Kotlin in `apps/mobile/modules/ghost-call`) read the same
//! file, so the three sides cannot drift apart without a test going red.
//!
//! `GHOST_WRITE_VECTORS=1 cargo test -p ghostd phone::vectors` rewrites the file; without it
//! the test checks the file still says what the daemon does.

use serde_json::{Value, json};

use super::listener::StreamKey;
use super::signing::{
    Secret, b64url, body_hash, request_string, response_string, sign_request, sign_response,
};
use super::stream::frame;

const FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/api/phone-vectors.json"
);
const NONCE: &str = "00112233445566778899aabbccddeeff";
const TIME: &str = "1762851600";
const DEVICE: &str = "dev-4f2a";

fn secret() -> Secret {
    let raw: Vec<u8> = (0u8..32).collect();
    Secret::parse(&b64url(&raw)).expect("base64url")
}

fn vectors() -> Value {
    let secret = secret();
    let requests: Vec<Value> = [
        ("GET", "/hello", ""),
        ("GET", "/stream?cursor=3fa9c2d1.41", ""),
        (
            "POST",
            "/questions/answer",
            r#"{"id":"7d1f0c2e-9a41-4f3b-8c55-0e6d2b9a1f30","key":"question:0b00bdd56307","text":"Send it today, café “Lumen” too"}"#,
        ),
        ("POST", "/seen", r#"{"alert_id":"9c1e04a7b2d3"}"#),
    ]
    .iter()
    .map(|(method, path, body)| {
        json!({
            "method": method, "path": path, "body": body,
            "string": request_string(method, path, TIME, NONCE, body.as_bytes()),
            "signature": sign_request(&secret, method, path, TIME, NONCE, body.as_bytes()),
        })
    })
    .collect();
    let responses: Vec<Value> = [
        (200u16, r#"{"ok":true}"#),
        (401, r#"{"error":"unauthorized"}"#),
        // The live stream's head: signed over an empty body.
        (200, ""),
    ]
    .iter()
    .map(|(status, body)| {
        json!({
            "status": status, "body": body, "body_hash": body_hash(body.as_bytes()),
            "string": response_string(*status, NONCE, TIME, body.as_bytes()),
            "signature": sign_response(&secret, *status, NONCE, TIME, body.as_bytes()),
        })
    })
    .collect();
    let key = StreamKey {
        secret: secret.clone(),
        nonce: NONCE.to_string(),
    };
    let messages = [
        json!({"type": "hello", "epoch": "3fa9c2d1", "resumed": false, "heartbeat_secs": 15}),
        json!({"type": "question", "question": {
            "key": "question:0b00bdd56307", "kind": "question",
            "asked_by": "Ledger", "source": "agent:ledger",
            "question": "Should the “October” invoice go out today?\nIt is €1,200.", "detail": null,
            "choices": ["Yes", "No"], "choices_only": false, "urgency": "notify",
            "asked_at": "2026-11-11T09:00:00Z", "expires_at": "2026-11-12T09:00:00Z",
            "state": "open", "answer": null, "choice": null,
            "answered_via": null, "answered_at": null, "change_id": null},
            "seq": 42, "cursor": "3fa9c2d1.42"}),
        json!({"type": "heartbeat", "cursor": "3fa9c2d1.42", "at": "2026-11-11T09:01:15Z"}),
    ];
    let events: Vec<Value> = messages
        .iter()
        .map(|m| {
            let frame = String::from_utf8(frame(&key, m).to_vec()).expect("utf-8");
            let data: Value = frame
                .lines()
                .find_map(|l| l.strip_prefix("data: "))
                .and_then(|d| serde_json::from_str(d).ok())
                .expect("a data line");
            json!({
                "frame": frame,
                "msg": data["msg"],
                "sig": data["sig"],
                "string": super::signing::event_string(NONCE, data["msg"].as_str().unwrap_or("").as_bytes()),
            })
        })
        .collect();
    json!({
        "about": "Shared test vectors for docs/phone-protocol.md, written by the daemon's own signing code (crates/ghostd/src/phone/vectors.rs). Regenerate with GHOST_WRITE_VECTORS=1 cargo test -p ghostd phone::vectors.",
        "secret": secret.as_str(),
        "device": DEVICE,
        "nonce": NONCE,
        "time": TIME,
        "requests": requests,
        "responses": responses,
        "events": events,
    })
}

#[test]
fn the_shared_vectors_are_what_the_daemon_signs() {
    let built = vectors();
    if std::env::var_os("GHOST_WRITE_VECTORS").is_some() {
        let mut text = serde_json::to_string_pretty(&built).expect("json");
        text.push('\n');
        std::fs::write(FILE, text).expect("write the vectors");
    }
    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(FILE).expect("packages/api/phone-vectors.json"),
    )
    .expect("json");
    assert_eq!(
        on_disk, built,
        "packages/api/phone-vectors.json is stale: GHOST_WRITE_VECTORS=1 cargo test -p ghostd phone::vectors"
    );
}

#[test]
fn a_message_verifies_only_over_its_own_bytes_and_nonce() {
    let secret = secret();
    let msg = br#"{"type":"heartbeat"}"#;
    let sig = super::signing::sign_event(&secret, NONCE, msg);
    assert!(super::signing::verify_event(&secret, NONCE, msg, &sig));
    assert!(!super::signing::verify_event(
        &secret,
        "ffeeddccbbaa99887766554433221100",
        msg,
        &sig
    ));
    assert!(!super::signing::verify_event(
        &secret,
        NONCE,
        br#"{"type":"heartbeat" }"#,
        &sig
    ));
}
