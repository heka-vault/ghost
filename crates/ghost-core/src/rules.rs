//! Ghost's rules as a tool source (ADR 0035): one operation, `rules.add`.
//!
//! `AGENTS.md` in Ghost's folder is in every agent's prompt. `rules.add` appends the
//! operator's words to it, verbatim, like a persona change (ADR 0014): Ghost never
//! paraphrases, reorders or rewrites that file. The operation has no `asked_by`, so where
//! the permissions ask first a plain request is never the yes: the operator hears the exact
//! text read back and says yes (the words the model wrote are what gets written, so the
//! readback is the check). On the spoken path the text must also be in the operator's
//! request, word for word. Nothing else in Ghost's folder (`apps.json`, `watches.json`,
//! `hooks.json`, `policy.toml`) has a voice path at all.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::folder::{RULES_FILE, arg_str, expanduser, is_symlink};
use crate::levels::Risk;
use crate::models::{AddRule, Change, ChangeSet, RulesSnapshot, RulesTarget, Snapshot, Target};
use crate::source::{Operation, PrepareFn, Prepared, ToolSource, Writer};

/// The gate, policy and audit key.
pub const SOURCE: &str = "rules";
pub const MAX_RULE: usize = 1000;
/// The first line of a new AGENTS.md.
pub const HEADING: &str = "# Ghost";

static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9']+").unwrap());

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("{} is a link; Ghost writes only regular files in its own folder", .0.display())]
    Link(PathBuf),
    #[error("the rule is required, in the operator's words")]
    Required,
    #[error("that rule is too long; keep it to a few sentences")]
    TooLong,
    #[error(
        "a rule is written in the operator's words: pass exactly what they said after 'add a rule', nothing rephrased"
    )]
    NotSaid,
    #[error("RulesWriter cannot apply {0}")]
    CannotApply(String),
    #[error("not Ghost's rules: {0}")]
    NotRules(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The file's text; empty when it is missing, a link or unreadable.
fn read(path: &Path) -> String {
    if is_symlink(path) || !path.is_file() {
        return String::new();
    }
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// Append `text` as its own paragraph, under a heading when the file is new.
fn append(path: &Path, text: &str) -> Result<(), RulesError> {
    if is_symlink(path) {
        return Err(RulesError::Link(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut current = read(path);
    if current.trim().is_empty() {
        current = format!("{HEADING}\n");
    }
    if !current.ends_with('\n') {
        current.push('\n');
    }
    if !current.ends_with("\n\n") {
        current.push('\n');
    }
    std::fs::write(path, format!("{current}{}\n", text.trim()))?;
    Ok(())
}

/// Whether `text` is in the operator's request, word for word (case, punctuation and
/// spacing aside). The spoken path passes the request; the desktop, where the operator
/// typed the rule, does not.
pub fn said(text: &str, request: &str) -> bool {
    fn norm(s: &str) -> String {
        let lower = s.to_lowercase();
        WORDS
            .find_iter(&lower)
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
    let wanted = norm(text);
    !wanted.is_empty() && norm(request).contains(&wanted)
}

pub struct RulesWriter {
    pub path: PathBuf,
}

impl RulesWriter {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn allowed(&self, target: &Target) -> bool {
        matches!(target, Target::Rules(_))
    }
}

#[async_trait]
impl Writer for RulesWriter {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot> {
        if !self.allowed(target) {
            return Err(RulesError::NotRules(target.to_string()).into());
        }
        let rules = read(&self.path);
        let digest = format!("{:x}", Sha256::digest(rules.as_bytes()));
        Ok(Snapshot::Rules(RulesSnapshot {
            version: digest[..16].to_string(),
            rules,
        }))
    }

    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        _idempotency_key: &str,
    ) -> anyhow::Result<()> {
        if !self.allowed(target) {
            return Err(RulesError::NotRules(target.to_string()).into());
        }
        match change {
            Change::AddRule(AddRule { text }) => append(&self.path, text)?,
            other => return Err(RulesError::CannotApply(other.describe()).into()),
        }
        Ok(())
    }
}

/// Ghost's `AGENTS.md`: read for the prompt and the desktop, added to through the gate.
#[derive(Clone)]
pub struct Rules {
    pub path: PathBuf,
    pub writer: Arc<RulesWriter>,
}

impl Rules {
    pub fn new(config_dir: impl AsRef<Path>) -> Self {
        let path = expanduser(config_dir.as_ref()).join(RULES_FILE);
        Self {
            writer: Arc::new(RulesWriter::new(path.clone())),
            path,
        }
    }

    pub fn text(&self) -> String {
        read(&self.path)
    }

    /// The change that appends `text`. `request` is the operator's spoken request, when
    /// there is one: the rule must be in it, word for word.
    pub fn prepare_add(&self, text: &str, request: Option<&str>) -> Result<Prepared, RulesError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RulesError::Required);
        }
        if text.chars().count() > MAX_RULE {
            return Err(RulesError::TooLong);
        }
        if let Some(request) = request
            && !said(text, request)
        {
            return Err(RulesError::NotSaid);
        }
        Ok(Prepared {
            target: Target::Rules(RulesTarget {}),
            changes: ChangeSet::one(Change::AddRule(AddRule {
                text: text.to_string(),
            })),
            rationale: "a rule for every agent, in the operator's words".into(),
            asked_by: Vec::new(),
        })
    }

    pub fn operations(&self) -> Vec<Operation> {
        let writer = self.writer.clone();
        let rules = self.clone();
        let prepare: PrepareFn = Arc::new(move |args: Map<String, Value>| {
            let rules = rules.clone();
            Box::pin(async move {
                let request = args
                    .contains_key("_request")
                    .then(|| arg_str(&args, "_request"));
                Ok(rules.prepare_add(&arg_str(&args, "text"), request.as_deref())?)
            })
        });
        let allowed = writer.clone();
        vec![Operation {
            name: "add".into(),
            description: "Add a standing rule for every agent to Ghost's AGENTS.md, in the \
                          operator's own words, verbatim: 'add a rule: always use UK dates'. Never \
                          paraphrase, never rewrite what is there. The operator hears the exact \
                          text back and says yes before it is written."
                .into(),
            properties: json!({"text": {"type": "string"}}),
            required: vec!["text".into()],
            prepare,
            writer,
            risk: Risk::Medium,
            allowed: Arc::new(move |t| allowed.allowed(t)),
            asked_by: Vec::new(),
            tool_name: None,
        }]
    }

    pub fn source(&self) -> ToolSource {
        ToolSource {
            name: SOURCE.into(),
            prompt: String::new(),
            read_tools: Vec::new(),
            operations: self.operations(),
            ..Default::default()
        }
    }
}

pub fn rules_source(rules: &Rules) -> ToolSource {
    rules.source()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{Request, Submission, WriteGate};
    use crate::levels::Autonomy;
    use crate::models::OutcomeStatus;
    use crate::policy::AutonomyPolicy;
    use crate::store::Store;
    use tempfile::TempDir;

    struct World {
        _tmp: TempDir,
        cfg: PathBuf,
        rules: Rules,
        gate: WriteGate,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("ghost");
        std::fs::create_dir(&cfg).unwrap();
        std::fs::write(cfg.join("AGENTS.md"), "# Ghost\n\nBe warm.\n").unwrap();
        let rules = Rules::new(&cfg);
        // ADR 0034: everything runs unless a rule asks first; this world asks for a rule.
        let gate = WriteGate::new(Arc::new(Store::in_memory().unwrap())).with_autonomy(
            AutonomyPolicy::parse(
                &format!("[permissions]\nask = ['{SOURCE}.add']\n"),
                "<test>",
            )
            .unwrap(),
        );
        gate.register(SOURCE, rules.operations());
        World {
            _tmp: tmp,
            cfg,
            rules,
            gate,
        }
    }

    async fn submit(gate: &WriteGate, rules: &Rules, args: Value) -> anyhow::Result<Submission> {
        let op = rules.operations().remove(0);
        let prepared = (op.prepare)(args.as_object().cloned().unwrap()).await?;
        let mut req = Request::new("s1", SOURCE, "add", prepared.target, prepared.changes, "");
        req.asked_by = prepared.asked_by;
        Ok(gate.submit(req).await?)
    }

    fn agents_md(cfg: &Path) -> String {
        std::fs::read_to_string(cfg.join("AGENTS.md")).unwrap()
    }

    #[test]
    fn one_operation_and_no_words_that_ask_for_it() {
        let w = world();
        let ops = w.rules.operations();
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert!(op.name == "add" && op.asked_by.is_empty() && op.required == ["text"]);
        let keys: Vec<&String> = op.properties.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["text"]); // no department, no file: Ghost's AGENTS.md only
    }

    #[tokio::test]
    async fn by_default_a_rule_is_added_and_announced() {
        let w = world();
        let gate = WriteGate::new(Arc::new(Store::in_memory().unwrap()));
        gate.register(SOURCE, w.rules.operations());
        let sub = submit(&gate, &w.rules, json!({"text": "Be brief."}))
            .await
            .unwrap();
        assert_eq!(sub.autonomy, Autonomy::Announce);
        let outcome = sub.outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Success);
        assert_eq!(agents_md(&w.cfg), "# Ghost\n\nBe warm.\n\nBe brief.\n");
        assert!(w.rules.text().ends_with("Be brief.\n"));
        assert!(!outcome.summary().ends_with("..")); // the rule's own stop is enough
    }

    #[tokio::test]
    async fn a_rule_is_appended_verbatim_at_confirm_after_a_spoken_yes() {
        let w = world();
        let sub = submit(&w.gate, &w.rules, json!({"text": "Always use UK dates."}))
            .await
            .unwrap();
        assert!(sub.autonomy == Autonomy::Confirm && sub.outcome.is_none()); // the ask rule
        assert_eq!(
            sub.proposal.preview,
            "On Ghost's rules: add the rule: Always use UK dates.."
        );
        // The request is never the yes here: the text the model wrote is read back, and the
        // operator's yes to that exact text is what writes it.
        assert!(
            w.gate
                .confirm_by_request("s1", "ghost, add a rule: always use UK dates", "", None)
                .await
                .unwrap()
                .is_none()
        );
        let (_, out) = w.gate.on_user_turn("s1", "yes", None, "").await.unwrap();
        assert_eq!(out.unwrap().status, OutcomeStatus::Success);
        assert_eq!(
            agents_md(&w.cfg),
            "# Ghost\n\nBe warm.\n\nAlways use UK dates.\n"
        );
    }

    #[tokio::test]
    async fn by_voice_the_rule_is_the_operators_words() {
        let w = world();
        let err = submit(
            &w.gate,
            &w.rules,
            json!({"text": "Use British dates.", "_request": "ghost, add a rule: always use UK dates"}),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("operator's words"), "{err}");
        let sub = submit(
            &w.gate,
            &w.rules,
            json!({"text": "Always use UK dates", "_request": "Ghost, add a rule: Always use UK dates!"}),
        )
        .await
        .unwrap();
        assert!(sub.proposal.preview.ends_with("Always use UK dates."));
        let err = submit(&w.gate, &w.rules, json!({"text": "  "}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("required"), "{err}");
        let err = submit(&w.gate, &w.rules, json!({"text": "x".repeat(1001)}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too long"), "{err}");
    }

    #[tokio::test]
    async fn a_fresh_agents_md_gets_a_heading() {
        let w = world();
        std::fs::remove_file(w.cfg.join("AGENTS.md")).unwrap();
        submit(&w.gate, &w.rules, json!({"text": "Start again."}))
            .await
            .unwrap();
        w.gate.confirm_pending("s1", "ui", None).await.unwrap();
        assert_eq!(agents_md(&w.cfg), "# Ghost\n\nStart again.\n");
    }

    #[tokio::test]
    async fn never_written_through_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("CLAUDE.md");
        std::fs::write(&shared, "# Project rules\n").unwrap();
        let cfg = tmp.path().join("ghost");
        std::fs::create_dir(&cfg).unwrap();
        std::os::unix::fs::symlink(&shared, cfg.join("AGENTS.md")).unwrap();
        let rules = Rules::new(&cfg);
        let err = rules
            .writer
            .apply(
                &Target::Rules(RulesTarget {}),
                &Change::AddRule(AddRule {
                    text: "Always use UK dates.".into(),
                }),
                "k",
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("a link"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&shared).unwrap(),
            "# Project rules\n"
        );
        assert_eq!(rules.text(), ""); // and not read through one either
    }
}
