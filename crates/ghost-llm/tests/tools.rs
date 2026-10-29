//! The tools offered to the model (the port of `test_prompt_and_tools.py`'s schema tests):
//! only reads, `change` and the session's own are exposed, blocked operations are not
//! listed, there is no undo, and the list never names a tool twice or badly.

use std::sync::Arc;

use async_trait::async_trait;
use ghost_core::policy::AutonomyPolicy;
use ghost_core::source::{Operation, ReadTool, ToolSource, Writer};
use ghost_core::{Change, Snapshot, Target};
use ghost_llm::{OfferAll, Offered, Offering, Offers, SessionTools, tool_name_ok};
use serde_json::{Map, Value, json};

struct NoWriter;

#[async_trait]
impl Writer for NoWriter {
    async fn fetch(&self, _target: &Target) -> anyhow::Result<Snapshot> {
        anyhow::bail!("nothing to fetch")
    }

    async fn apply(&self, _target: &Target, _change: &Change, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

fn op(name: &str, tool_name: Option<&str>) -> Operation {
    let mut o = Operation::new(
        name,
        format!("{name}."),
        Arc::new(|_args| Box::pin(async { anyhow::bail!("not prepared here") })),
        Arc::new(NoWriter),
    );
    o.properties = json!({"name": {"type": "string"}});
    o.required = vec!["name".into()];
    o.tool_name = tool_name.map(String::from);
    o
}

fn read(name: &str, description: &str, properties: Value, required: &[&str]) -> ReadTool {
    ReadTool {
        name: name.into(),
        description: description.into(),
        properties,
        required: required.iter().map(|r| (*r).to_string()).collect(),
        handler: Arc::new(|_args| Box::pin(async { Ok(Map::new()) })),
        operation: String::new(),
    }
}

fn source(name: &str, reads: Vec<ReadTool>, ops: Vec<Operation>) -> ToolSource {
    ToolSource {
        name: name.into(),
        read_tools: reads,
        operations: ops,
        ..Default::default()
    }
}

fn fake_source() -> ToolSource {
    let mut s = source(
        "fake",
        vec![read(
            "echo",
            "Echo.",
            json!({"x": {"type": "string"}}),
            &["x"],
        )],
        vec![op("flag", None), op("nuke", None), op("never", None)],
    );
    s.prompt = "FAKE FRAGMENT".into();
    s
}

fn wired() -> Offering {
    let policy = AutonomyPolicy::parse(
        "[permissions]\nblock = [\"fake.never\"]\nask = [\"fake.nuke\"]\n",
        "policy.toml",
    )
    .unwrap();
    let offers: Arc<dyn Offers> = Arc::new(policy.permissions);
    Offering::new(offers, SessionTools::default())
}

#[test]
fn test_only_reads_change_and_sleep_are_exposed() {
    let schemas = wired().schemas(&[fake_source()]);
    let mut names = schemas.names();
    names.sort_unstable();
    assert_eq!(names, vec!["bring_up_change", "change", "echo", "hang_up"]);
    let echo = schemas.get("echo").unwrap();
    assert_eq!(echo.description, "Echo.");
    assert_eq!(echo.required, vec!["x".to_string()]);
}

#[test]
fn test_denied_ops_are_not_offered() {
    let schemas = wired().schemas(&[fake_source()]);
    let change = schemas.get("change").unwrap();
    assert!(change.description.contains("fake.flag") && change.description.contains("fake.nuke"));
    assert!(!change.description.contains("fake.never"));
    assert!(
        change
            .description
            .contains("- fake.flag: flag. Args: ['name'] (required: ['name'])")
    );
    assert_eq!(change.properties["ability"]["enum"], json!(["fake"]));
    assert_eq!(change.required, vec!["ability", "operation", "args"]);
}

#[test]
fn test_no_undo_tool() {
    let offering = Offering::new(
        Arc::new(OfferAll),
        SessionTools {
            conversations: true,
            hold: true,
        },
    );
    let schemas = offering.schemas(&[fake_source()]);
    for name in schemas.names() {
        let lower = name.to_lowercase();
        assert!(
            !lower.contains("undo") && !lower.contains("revert") && !lower.contains("rollback"),
            "{name}"
        );
    }
}

#[test]
fn test_the_tool_list_never_names_a_tool_twice_or_badly() {
    // The model API refuses the whole list over one name twice or one it can't take, so
    // every turn failed. Ghost's own tools keep their names; a later clash isn't offered.
    let skills = source(
        "skills",
        vec![read("read_skill", "Ghost's.", json!({}), &[])],
        vec![],
    );
    let app = source(
        "read",
        vec![
            read("read_skill", "An app's.", json!({}), &[]),
            read("hang_up", "An app's.", json!({}), &[]),
            read("bad.name", "Bad.", json!({}), &[]),
            read(&"x".repeat(65), "Long.", json!({}), &[]),
        ],
        vec![],
    );
    let other = source(
        "other",
        vec![read("read_skill", "Another.", json!({}), &[])],
        vec![],
    );
    let sources = [app, other, skills];
    let offering = Offering::new(Arc::new(OfferAll), SessionTools::default());
    let schemas = offering.schemas(&sources);
    let names = schemas.names();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(names.len(), unique.len(), "{names:?}");
    assert!(names.iter().all(|n| tool_name_ok(n)), "{names:?}");
    assert_eq!(schemas.get("read_skill").unwrap().description, "Ghost's.");
    match offering.lookup(&sources, "read_skill") {
        Some(Offered::Read { source, tool }) => {
            assert_eq!(source.name, "skills");
            assert_eq!(tool.description, "Ghost's.");
        }
        other => panic!("{other:?}"),
    }
    assert!(offering.lookup(&sources, "bad.name").is_none());
    assert!(offering.lookup(&sources, "hang_up").is_none()); // Ghost's own
    assert!(!format!("{offering:?}").is_empty());
}

#[test]
fn test_session_tools_follow_the_session() {
    let all = Offering::new(
        Arc::new(OfferAll),
        SessionTools {
            conversations: true,
            hold: true,
        },
    );
    assert_eq!(
        all.schemas(&[]).names(),
        vec!["switch_conversation", "bring_up_change", "hang_up", "hold"]
    );
    let bare = Offering::new(Arc::new(OfferAll), SessionTools::default());
    assert_eq!(
        bare.schemas(&[]).names(),
        vec!["bring_up_change", "hang_up"]
    );
    // No operations: no `change`.
    assert!(
        bare.schemas(&[source("x", vec![read("r", "R.", json!({}), &[])], vec![])])
            .get("change")
            .is_none()
    );
}

#[test]
fn test_an_apps_change_is_offered_under_its_own_name() {
    let github = source(
        "github",
        vec![read("github_issue_read", "Read an issue.", json!({}), &[])],
        vec![
            op("issue_write", Some("github_issue_write")),
            op("label", None),
        ],
    );
    let offering = Offering::new(Arc::new(OfferAll), SessionTools::default());
    let schemas = offering.schemas(std::slice::from_ref(&github));
    assert_eq!(
        schemas.names(),
        vec![
            "github_issue_read",
            "github_issue_write",
            "change",
            "bring_up_change",
            "hang_up"
        ]
    );
    let own = schemas.get("github_issue_write").unwrap();
    assert_eq!(own.description, "issue_write.");
    assert_eq!(own.required, vec!["name".to_string()]);
    let change = schemas.get("change").unwrap();
    assert!(change.description.contains("github.label"));
    assert!(!change.description.contains("github.issue_write"));
    match offering.lookup(&[github], "github_issue_write") {
        Some(Offered::Change { operation, source }) => {
            assert_eq!(operation.name, "issue_write");
            assert_eq!(source, "github");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_reserved_sources_keep_a_name_in_a_clash() {
    // An app connected before `waiting` in the list still loses `waiting_list` to it.
    let app = source(
        "app",
        vec![read("waiting_list", "An app's.", json!({}), &[])],
        vec![],
    );
    let waiting = source(
        "waiting",
        vec![read("waiting_list", "Ghost's.", json!({}), &[])],
        vec![],
    );
    let offering = Offering::new(Arc::new(OfferAll), SessionTools::default());
    let schemas = offering.schemas(&[app, waiting]);
    assert_eq!(schemas.get("waiting_list").unwrap().description, "Ghost's.");
    assert_eq!(schemas.len(), 3);
}

#[test]
fn test_the_schemas_take_each_protocols_shape() {
    let schemas = wired().schemas(&[fake_source()]);
    let a = schemas.anthropic();
    assert_eq!(a[0]["name"], "echo");
    assert_eq!(a[0]["input_schema"]["required"], json!(["x"]));
    let o = schemas.openai();
    assert_eq!(o[0]["type"], "function");
    assert_eq!(
        o[0]["function"]["parameters"]["properties"]["x"]["type"],
        "string"
    );
    assert_eq!(a.len(), schemas.len());
}
