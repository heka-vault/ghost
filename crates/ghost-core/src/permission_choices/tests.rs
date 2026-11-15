//! The writer of the operator's rules from a decision (ADR 0070). Temp folders only: never
//! the operator's own policy.toml.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::fakes::{FakeWriter, changeset, issue, register_fake};
use crate::gate::{Request, WriteGate};
use crate::models::{Change, SetLabels, Target};

fn file(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.toml");
    if !text.is_empty() {
        fs::write(&path, text).unwrap();
    }
    (dir, path)
}

fn rule(text: &str) -> Rule {
    Rule::parse(text).unwrap()
}

fn text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

const HAND_WRITTEN: &str = r#"# My permissions. Ghost: leave my notes alone.

[permissions]
mode = "open"   # everything runs
# the dangerous ones
block = ["github.delete_*"]
allow = ["gmail.send_email"]  # I trust this one

[defaults]
pending_ttl = 900

# how Ghost reaches me
[routes]
enabled = ["digest", "phone"]
"#;

#[test]
fn adding_a_rule_changes_only_its_list_and_keeps_every_other_byte() {
    let (_d, path) = file(HAND_WRITTEN);
    let w = add_rule(&path, Choice::Allow, &rule("github.issue_write")).unwrap();
    assert_eq!(w, Written::default());
    let after = text(&path);
    assert_eq!(
        after,
        HAND_WRITTEN.replace(
            r#"allow = ["gmail.send_email"]"#,
            r#"allow = ["gmail.send_email", "github.issue_write"]"#
        )
    );
    let p = AutonomyPolicy::load(&path).unwrap();
    assert_eq!(p.resolve("github", "issue_write", None), Autonomy::Announce);
    assert_eq!(p.pending_ttl, Some(900));
}

#[test]
fn a_list_written_one_per_line_stays_one_per_line() {
    let before = "[permissions]\nblock = [\n  \"gmail\",\n  \"github.delete_*\",\n]\n";
    let (_d, path) = file(before);
    let w = add_rule(&path, Choice::Block, &rule("github.merge_pull_request")).unwrap();
    assert_eq!(
        text(&path),
        "[permissions]\nblock = [\n  \"gmail\",\n  \"github.delete_*\",\n  \"github.merge_pull_request\",\n]\n"
    );
    assert!(remove_rule(&path, Choice::Block, &rule("github.merge_pull_request"), w).unwrap());
    assert_eq!(text(&path), before);
}

#[test]
fn a_missing_key_a_missing_table_and_a_missing_file_are_added_and_taken_back_out() {
    for before in [
        "# just notes\n\n[permissions]\nmode = \"ask\"\n\n[defaults]\npending_ttl = 600\n",
        "# no permissions table yet\n[defaults]\npending_ttl = 600\n",
        "",
    ] {
        let (_d, path) = file(before);
        let r = rule("github.issue_write");
        let w = add_rule(&path, Choice::Allow, &r).unwrap();
        let p = AutonomyPolicy::load(&path).unwrap();
        assert_eq!(p.permissions.allow, vec![r.clone()], "{before:?}");
        let written = text(&path);
        assert!(written.starts_with(before.lines().next().unwrap_or("")));
        if before.contains("[permissions]") {
            assert!(!w.created_table && w.created_key);
            assert!(
                written.contains("mode = \"ask\"\nallow = [\"github.issue_write\"]"),
                "{written}"
            );
        } else {
            assert!(w.created_table && w.created_key, "{before:?}");
        }
        assert!(remove_rule(&path, Choice::Allow, &r, w).unwrap());
        assert_eq!(text(&path), before, "{before:?}");
    }
}

#[test]
fn a_rule_already_in_the_list_is_not_written_twice() {
    let (_d, path) = file(HAND_WRITTEN);
    let r = rule("github.issue_write");
    add_rule(&path, Choice::Allow, &r).unwrap();
    let once = text(&path);
    let again = add_rule(&path, Choice::Allow, &r).unwrap();
    assert!(again.already);
    assert_eq!(text(&path), once);
    assert_eq!(once.matches("github.issue_write").count(), 1);
}

#[test]
fn removal_restores_the_file_byte_for_byte_when_the_rule_was_the_last_thing_added() {
    let (_d, path) = file(HAND_WRITTEN);
    let r = rule("github.issue_write");
    let w = add_rule(&path, Choice::Block, &r).unwrap();
    assert_ne!(text(&path), HAND_WRITTEN);
    assert!(remove_rule(&path, Choice::Block, &r, w).unwrap());
    assert_eq!(text(&path), HAND_WRITTEN);
    // Not there any more (taken out by hand, or twice): nothing to do, nothing written.
    assert!(!remove_rule(&path, Choice::Block, &r, w).unwrap());
    assert_eq!(text(&path), HAND_WRITTEN);
}

#[test]
fn a_file_that_does_not_read_is_never_written() {
    for broken in [
        "[permissions\nmode = \"open\"\n",
        "[permissions]\nmode = \"loud\"\n",
    ] {
        let (_d, path) = file(broken);
        let err = add_rule(&path, Choice::Allow, &rule("github.issue_write")).unwrap_err();
        assert!(
            matches!(&err, ChoiceError::Refused(m) if m.contains("doesn’t read")),
            "{err:?}"
        );
        assert_eq!(text(&path), broken);
        assert!(!backup_path(&path).exists());
    }
}

#[test]
fn an_older_layout_the_rule_would_change_is_refused() {
    // Without a [permissions] table, `[defaults] autonomy` sets the mode; adding one would
    // quietly turn "ask" into "open". The writer sees that and refuses.
    let legacy = "[defaults]\nautonomy = \"confirm\"\n\n[conversations]\ndelete = \"deny\"\n";
    let (_d, path) = file(legacy);
    let err = add_rule(&path, Choice::Allow, &rule("github.issue_write")).unwrap_err();
    assert!(matches!(err, ChoiceError::Refused(m) if m.contains("layout")));
    assert_eq!(text(&path), legacy);
}

#[test]
fn the_first_write_backs_the_file_up_once_and_keeps_its_mode() {
    let (dir, path) = file(HAND_WRITTEN);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    add_rule(&path, Choice::Allow, &rule("a.b")).unwrap();
    add_rule(&path, Choice::Allow, &rule("a.c")).unwrap();
    assert_eq!(
        fs::read_to_string(backup_path(&path)).unwrap(),
        HAND_WRITTEN
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // Written whole and renamed into place: nothing half-written is left beside it.
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".lock"))
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn only_one_tool_of_one_app_is_ever_offered_and_named_by_its_own_name() {
    assert_eq!(
        rule_for("github", "issue_write").unwrap().to_string(),
        "github.issue_write"
    );
    // A name a rule would read as a pattern, or can't hold, is never offered.
    for (app, tool) in [
        ("github", "delete_*"),
        ("github", "issue?"),
        ("git hub", "x"),
        ("*", "x"),
        ("github", "a(b=c)"),
        ("github", ""),
    ] {
        assert!(rule_for(app, tool).is_none(), "{app}.{tool}");
    }
    assert_eq!(
        scope_words("github", "issue_write"),
        "“issue write” in github"
    );
    assert_eq!(
        scope_words("linear", "createIssue"),
        "“create issue” in linear"
    );
    assert_eq!(scope_words("ghost", "notify"), "“notify” in Ghost");
    let long = scope_words("x", "a_very_long_tool_name_with_far_too_many_words_in_it");
    assert!(long.chars().count() < 60, "{long}");
}

#[test]
fn a_more_careful_rule_holds_the_allow_back_and_the_card_says_so() {
    let perms = |t: &str| Permissions::parse(&toml::from_str(t).unwrap(), "t").unwrap();
    let none = Map::new();
    let open = offer(&Permissions::default(), "github", "issue_write", &none).unwrap();
    assert!(open.allow && open.held.is_none());
    assert_eq!(open.rule, "github.issue_write");
    // mode = ask: an allow is the exception the mode is for, so it takes effect.
    let ask_mode = offer(&perms("mode = \"ask\""), "github", "issue_write", &none).unwrap();
    assert!(ask_mode.allow);
    // An ask rule of the operator's beats any allow: offering it would do nothing.
    for list in [
        "ask = [\"github\"]",
        "ask = [\"github.issue_*\"]",
        "ask = [\"github.issue_write\"]",
        "ask = [\"github.issue_write(state=closed)\"]",
    ] {
        let o = offer(
            &perms(list),
            "github",
            "issue_write",
            &json!({"state": "closed"}).as_object().unwrap().clone(),
        )
        .unwrap();
        assert!(!o.allow, "{list}");
        assert_eq!(o.held.as_deref(), Some(HELD));
    }
    // The pattern does not catch this call: the allow takes effect.
    let other = offer(
        &perms("ask = [\"github.issue_write(state=closed)\"]"),
        "github",
        "issue_write",
        &json!({"state": "open"}).as_object().unwrap().clone(),
    )
    .unwrap();
    assert!(other.allow);
}

// --- the gate and the decision ---------------------------------------------------------------

fn gate_on(path: &Path) -> (WriteGate, Arc<FakeWriter>) {
    let writer = Arc::new(FakeWriter::default());
    writer.seed(&issue("acme", "app", 1));
    let gate = WriteGate::new(Arc::new(Store::in_memory().unwrap()))
        .with_autonomy(AutonomyPolicy::load(path).unwrap());
    register_fake(&gate, writer.clone(), "acme/app");
    (gate, writer)
}

fn label(session: &str) -> Request {
    Request::new(
        session,
        "github_issues",
        "set_labels",
        Target::FakeIssue(issue("acme", "app", 1)),
        changeset(vec![Change::SetLabels(SetLabels {
            add: vec!["triaged".into()],
            remove: Vec::new(),
        })]),
        "asked",
    )
}

#[tokio::test]
async fn the_gate_follows_the_new_rule_on_the_very_next_call_without_a_restart() {
    let (_d, path) = file("[permissions]\nmode = \"ask\"\n");
    let (gate, _w) = gate_on(&path);
    let first = gate.submit(label("s1")).await.unwrap();
    assert_eq!(first.autonomy, Autonomy::Confirm);
    let row = set_from_decision(
        gate.store(),
        &path,
        &first.proposal,
        &WriteGate::args_of(&first.proposal),
        Choice::Allow,
        Surface::Desktop,
    )
    .unwrap();
    assert_eq!(row.rule, "github_issues.set_labels");
    // What the control API does after the write (`Controller::reload_policy`): the file
    // read again into the running gate. No restart.
    gate.set_autonomy(AutonomyPolicy::load(&path).unwrap());
    let next = gate.submit(label("s1")).await.unwrap();
    assert_eq!(next.autonomy, Autonomy::Announce);
    assert!(next.executed());
    // The audit has who set it and from which decision.
    let events = gate.store().events(&first.proposal.id).unwrap();
    let set = events.iter().find(|e| e.event == "permission_set").unwrap();
    let d = set.detail.as_ref().unwrap();
    assert_eq!(d["rule"], "github_issues.set_labels");
    assert_eq!(d["list"], "allow");
    assert_eq!(d["by"], "ui");
}

#[tokio::test]
async fn never_blocks_the_next_call_and_removing_the_choice_puts_the_file_back() {
    let before = "# mine\n[permissions]\nmode = \"ask\"\n";
    let (_d, path) = file(before);
    let (gate, _w) = gate_on(&path);
    let sub = gate.submit(label("s1")).await.unwrap();
    let row = set_from_decision(
        gate.store(),
        &path,
        &sub.proposal,
        &WriteGate::args_of(&sub.proposal),
        Choice::Block,
        Surface::Desktop,
    )
    .unwrap();
    gate.set_autonomy(AutonomyPolicy::load(&path).unwrap());
    assert!(gate.submit(label("s1")).await.is_err());
    assert_eq!(gate.store().permission_choices().unwrap().len(), 1);
    assert_eq!(row.sentence(), "“set labels” in github_issues never runs");

    assert!(remove_choice(gate.store(), &path, row.id).unwrap());
    assert_eq!(text(&path), before);
    assert!(gate.store().permission_choices().unwrap().is_empty());
    assert!(!remove_choice(gate.store(), &path, row.id).unwrap());
    let events = gate.store().events(&sub.proposal.id).unwrap();
    assert!(events.iter().any(|e| e.event == "permission_removed"));
}

#[tokio::test]
async fn an_allow_a_careful_rule_would_overrule_is_refused_and_nothing_is_written() {
    let before = "[permissions]\nask = [\"github_issues\"]\n";
    let (_d, path) = file(before);
    let (gate, _w) = gate_on(&path);
    let sub = gate.submit(label("s1")).await.unwrap();
    let err = set_from_decision(
        gate.store(),
        &path,
        &sub.proposal,
        &WriteGate::args_of(&sub.proposal),
        Choice::Allow,
        Surface::Desktop,
    )
    .unwrap_err();
    assert_eq!(err, ChoiceError::Refused(HELD.into()));
    assert_eq!(text(&path), before);
    assert!(gate.store().permission_choices().unwrap().is_empty());
}

#[tokio::test]
async fn a_runs_change_set_aside_is_made_to_stick_only_when_the_operator_decides_it() {
    // ADR 0047: a change at `ask` in a run with nobody there is set aside on the waiting
    // list, never made, and nothing in the run is the operator's yes. Nothing in it writes
    // a rule either: the file is as it was until the operator decides it on the desktop.
    let before = "[permissions]\nmode = \"ask\"\n";
    let (_d, path) = file(before);
    let (gate, writer) = gate_on(&path);
    let sub = gate.submit(label("run-7")).await.unwrap();
    assert_eq!(sub.autonomy, Autonomy::Confirm);
    gate.set_aside("run-7", "a run asks nobody").unwrap();
    assert_eq!(text(&path), before);
    assert!(gate.store().permission_choices().unwrap().is_empty());
    assert!(
        !writer
            .issue(&issue("acme", "app", 1))
            .labels
            .contains(&"triaged".to_string())
    );

    // Later, on the desktop: "Yes, and don't ask again".
    let waiting = gate.waiting_one(&sub.proposal.id, None).unwrap().unwrap();
    set_from_decision(
        gate.store(),
        &path,
        &waiting,
        &WriteGate::args_of(&waiting),
        Choice::Allow,
        Surface::Desktop,
    )
    .unwrap();
    gate.set_autonomy(AutonomyPolicy::load(&path).unwrap());
    let done = gate
        .confirm_id(&waiting.id, Surface::Desktop.as_str(), None)
        .await
        .unwrap();
    assert!(done.is_some());
    // The next run's change of the same tool runs without asking.
    let next = gate.submit(label("run-8")).await.unwrap();
    assert_eq!(next.autonomy, Autonomy::Announce);
}

#[test]
fn the_writer_is_reached_only_from_the_control_apis_operator_routes() {
    // ADR 0070: a rule is set by the operator's click on the desktop and nothing else. No
    // tool of Ghost's own app, outside client, run, phone route, voice path or model
    // reaches the writer: only the control API's handlers call it, and this module.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut callers = Vec::new();
    let mut stack = vec![crates.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                if !p.ends_with("target") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().is_some_and(|x| x == "rs") {
                let t = fs::read_to_string(&p).unwrap();
                // Callers outside this module name it by path (no `use` of the writer).
                let local = p.starts_with(crates.join("ghost-core/src/permission_choices"))
                    || p.ends_with("ghost-core/src/permission_choices.rs");
                if [
                    "set_from_decision",
                    "remove_choice(",
                    "permission_choices::add_rule",
                    "permission_choices::remove_rule",
                ]
                .iter()
                .any(|w| t.contains(w))
                    || (local && (t.contains("add_rule(") || t.contains("remove_rule(")))
                {
                    callers.push(
                        p.strip_prefix(crates)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
    }
    callers.sort();
    let allowed = [
        "ghost-api/src/permission_choices.rs",
        "ghost-api/tests/permission_choices.rs",
        "ghost-core/src/permission_choices.rs",
        "ghost-core/src/permission_choices/tests.rs",
        // The lock's own tests interleave this writer with the older ones.
        "ghost-core/src/policy_file.rs",
    ];
    for c in &callers {
        assert!(allowed.contains(&c.as_str()), "{c} reaches the rule writer");
    }
}
