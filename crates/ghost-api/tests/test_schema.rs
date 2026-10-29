//! The committed `packages/api/openapi.json` is the document this crate generates, byte for
//! byte once both are pretty-printed: it is the contract `packages/client-ts` is generated
//! from (ADR 0013, ADR 0039). Regenerate with `make api-schema`.

use serde_json::Value;

fn committed() -> Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/api/openapi.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("packages/api/openapi.json"))
        .unwrap()
}

#[test]
fn the_committed_document_is_current() {
    let ours = ghost_api::openapi::document();
    let theirs = committed();
    assert_eq!(
        ours, theirs,
        "packages/api/openapi.json is stale: run `make api-schema`"
    );
}

#[test]
fn every_field_the_daemon_always_writes_is_required() {
    // A field with a serde default is still always serialized, and the TypeScript apps
    // read it without a check; the schema must say `required` for it (`#[schema(required)]`
    // beside `#[serde(default)]`), or the generated client turns it optional. The event
    // envelopes carry a `default`, which the generator reads the same way.
    let doc = ghost_api::openapi::document();
    for (name, field) in [
        ("Status", "apps"),
        ("Status", "build"),
        ("WatchingView", "watches"),
        ("HookInfo", "accepted"),
        ("ModelConnectionView", "values"),
        ("PermissionsView", "block"),
        ("AppAction", "conditional"),
    ] {
        let required = doc["components"]["schemas"][name]["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} has no required list"));
        assert!(
            required.iter().any(|v| v == field),
            "{name}.{field} must be required"
        );
    }
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
    let client = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/client-ts/src/index.ts");
    let client = std::fs::read_to_string(client).expect("packages/client-ts/src/index.ts");
    assert!(!client.contains("mcp.json"));
    assert!(!client.to_lowercase().contains("department"));
}
