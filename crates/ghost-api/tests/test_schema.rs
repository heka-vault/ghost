//! The OpenAPI document the Rust daemon generates says what the committed
//! `packages/api/openapi.json` says: the same operations, and the same required fields on
//! the types the apps read, so `packages/client-ts` regenerates to the same client surface.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

fn committed() -> Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/api/openapi.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("packages/api/openapi.json"))
        .unwrap()
}

fn operations(doc: &Value) -> BTreeSet<(String, String)> {
    doc["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(path, ops)| {
            ops.as_object()
                .unwrap()
                .keys()
                .map(move |m| (m.to_uppercase(), path.clone()))
        })
        .collect()
}

/// A schema's required fields, through `allOf` and the `$ref`s in it.
fn required(doc: &Value, schema: &Value) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = schema["required"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if let Some(parts) = schema["allOf"].as_array() {
        for part in parts {
            let part = match part["$ref"].as_str() {
                Some(r) => &doc["components"]["schemas"][r.rsplit('/').next().unwrap()],
                None => part,
            };
            out.extend(required(doc, part));
        }
    }
    out
}

#[test]
fn the_generated_document_has_every_operation_the_committed_one_has() {
    let ours = ghost_api::openapi::document();
    let theirs = committed();
    assert_eq!(operations(&ours), operations(&theirs));
    // and says them the same way: the id, the tag, the status codes
    for (path, ops) in theirs["paths"].as_object().unwrap() {
        for (method, op) in ops.as_object().unwrap() {
            let mine = &ours["paths"][path][method];
            assert_eq!(mine["operationId"], op["operationId"], "{method} {path}");
            assert_eq!(mine["tags"], op["tags"], "{method} {path}");
            let codes = |o: &Value| -> BTreeSet<String> {
                o["responses"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect()
            };
            assert_eq!(codes(mine), codes(op), "{method} {path}");
            let params = |o: &Value| -> BTreeSet<(String, String)> {
                o["parameters"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|p| {
                                (
                                    p["in"].as_str().unwrap().into(),
                                    p["name"].as_str().unwrap().into(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            assert_eq!(params(mine), params(op), "{method} {path}");
            assert_eq!(
                mine.get("requestBody").is_some(),
                op.get("requestBody").is_some(),
                "{method} {path}"
            );
        }
    }
}

#[test]
fn the_top_level_schemas_require_the_same_fields() {
    let ours = ghost_api::openapi::document();
    let theirs = committed();
    let named: BTreeSet<&str> = [
        "Status",
        "AuditPage",
        "WaitingView",
        "AppView",
        "PolicyView",
        "PasswordsView",
        "ModelConnectionView",
        "ConversationDetail",
    ]
    .into_iter()
    .collect();
    let mine = ours["components"]["schemas"].as_object().unwrap();
    let committed = theirs["components"]["schemas"].as_object().unwrap();
    for name in &named {
        assert!(mine.contains_key(*name), "{name} is missing");
    }
    // Every schema both documents name, not only the eight the apps read first.
    let mut differ: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    for (name, schema) in committed {
        let Some(own) = mine.get(name) else {
            differ.insert(name.clone(), (required(&theirs, schema), BTreeSet::new()));
            continue;
        };
        let (a, b) = (required(&theirs, schema), required(&ours, own));
        if a != b {
            differ.insert(name.clone(), (a, b));
        }
    }
    assert!(differ.is_empty(), "required fields differ: {differ:#?}");
    let missing: Vec<&String> = committed
        .keys()
        .filter(|k| !mine.contains_key(*k))
        .collect();
    assert!(missing.is_empty(), "schemas missing: {missing:?}");
}

#[test]
fn the_contract_says_app_not_the_retired_words() {
    // There is no mcp.json (ADR 0034), a proposal's origin is its `source`, not an ability
    // (ADR 0022), and there are no departments (ADR 0035).
    let doc = ghost_api::openapi::document();
    let text = serde_json::to_string(&doc).unwrap();
    assert!(!text.contains("mcp.json"));
    assert!(!text.to_lowercase().contains("ability"));
    assert!(!text.to_lowercase().contains("department"));
    assert!(!text.contains("\"is_default\""));
    for model in ["PendingProposal", "AuditEntry"] {
        assert!(
            doc["components"]["schemas"][model]["properties"]
                .as_object()
                .unwrap()
                .contains_key("source")
        );
    }
}
