//! `/skills` (ADR 0023): list, read, add and remove through the gate.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{World, strings};
use ghost_core::skills::{SOURCE, Skills};
use serde_json::json;

fn make_skill(folder: &Path, name: &str, description: &str) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nSteps.\n"),
    )
    .unwrap();
    folder.to_path_buf()
}

#[tokio::test]
async fn skills_routes() {
    let mut w = World::new();
    let skills = Arc::new(Skills::new(&w.path().join("ghost-skills"), None));
    w.gate.register(SOURCE, skills.operations());
    w.skills = Some(skills);
    let c = w.client();
    assert_eq!(c.get("/skills").await.json, json!([]));
    let candidate = make_skill(
        &w.path().join("dl").join("invoicing"),
        "invoicing",
        "Send invoices.",
    );
    let r = c
        .post(
            "/skills",
            Some(json!({"path": candidate.display().to_string()})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["name"], "invoicing");
    assert_eq!(r["source"], "ghost");
    assert_eq!(
        strings(&c.get("/skills").await.json, "name"),
        vec!["invoicing".to_string()]
    );
    let body = c.get("/skills/invoicing").await;
    assert_eq!(body["body"], "Steps.");
    assert_eq!(body["files"], json!([]));
    assert_eq!(c.get("/skills/nope").await.status, 404);
    // the add is audited like any write from the desktop
    let audit = c.get("/audit").await;
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"] == "skills" && e["operation"] == "add")
    );
    let again = c
        .post(
            "/skills",
            Some(json!({"path": candidate.display().to_string()})),
        )
        .await;
    assert_eq!(again.status, 409);
    assert_eq!(c.delete("/skills/invoicing").await["ok"], true);
    assert_eq!(c.get("/skills").await.json, json!([]));
    assert_eq!(
        c.post(
            "/skills",
            Some(json!({"path": w.path().join("nowhere").display().to_string()}))
        )
        .await
        .status,
        409
    );
}
