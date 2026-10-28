//! An issue-like write model for the gate's tests, independent of any source.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::gate::WriteGate;
use crate::levels::Risk;
use crate::models::{Change, ChangeSet, FakeIssue, FakeIssueSnap, Snapshot, Target};
use crate::source::{Operation, Prepared, Writer};

#[derive(Debug, Clone, PartialEq)]
pub struct IssueState {
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub updated_at: String,
}

impl Default for IssueState {
    fn default() -> Self {
        Self {
            title: "Add login".into(),
            state: "open".into(),
            labels: vec!["needs-signoff".into()],
            updated_at: "2026-10-16T00:00:00Z".into(),
        }
    }
}

/// What the writer was asked to do, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Fetch(FakeIssue),
    SetLabels(FakeIssue, Vec<String>, Vec<String>),
    AddComment(FakeIssue, String, String),
    SetState(FakeIssue, String),
}

impl Call {
    pub fn name(&self) -> &'static str {
        match self {
            Call::Fetch(_) => "fetch",
            Call::SetLabels(..) => "set_labels",
            Call::AddComment(..) => "add_comment",
            Call::SetState(..) => "set_state",
        }
    }
}

/// In-memory issues. Records calls; can be told to fail a step; can take a moment per
/// read, as a real app's does (`delay`), and can hand back snapshots with no version, as
/// an app's tool does (`versioned` false, ADR 0034).
#[derive(Default)]
pub struct FakeWriter {
    pub issues: Mutex<HashMap<FakeIssue, IssueState>>,
    pub comments: Mutex<HashMap<FakeIssue, Vec<(String, String)>>>,
    pub calls: Mutex<Vec<Call>>,
    pub fail_on: Mutex<Option<String>>,
    pub delay: Option<Duration>,
    pub unversioned: bool,
}

impl FakeWriter {
    pub fn seed(&self, target: &FakeIssue) {
        self.issues
            .lock()
            .unwrap()
            .insert(target.clone(), IssueState::default());
    }

    pub fn issue(&self, target: &FakeIssue) -> IssueState {
        self.issues.lock().unwrap().get(target).cloned().unwrap()
    }

    pub fn set_issue(&self, target: &FakeIssue, f: impl FnOnce(&mut IssueState)) {
        f(self.issues.lock().unwrap().get_mut(target).unwrap());
    }

    pub fn call_names(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().iter().map(Call::name).collect()
    }

    pub fn fail_on(&self, step: &str) {
        *self.fail_on.lock().unwrap() = Some(step.to_string());
    }

    fn touch(&self, target: &FakeIssue) {
        self.set_issue(target, |i| i.updated_at.push('+'));
    }

    fn issue_of(target: &Target) -> anyhow::Result<&FakeIssue> {
        match target {
            Target::FakeIssue(i) => Ok(i),
            other => anyhow::bail!("not a fake issue: {other}"),
        }
    }
}

#[async_trait]
impl Writer for FakeWriter {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot> {
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        let t = Self::issue_of(target)?;
        self.calls.lock().unwrap().push(Call::Fetch(t.clone()));
        let i = self.issue(t);
        Ok(Snapshot::FakeIssue(FakeIssueSnap {
            version: if self.unversioned {
                String::new()
            } else {
                i.updated_at.clone()
            },
            title: i.title,
            state: i.state,
            labels: i.labels,
        }))
    }

    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        idempotency_key: &str,
    ) -> anyhow::Result<()> {
        let t = Self::issue_of(target)?;
        let failing = self.fail_on.lock().unwrap().clone();
        match change {
            Change::SetLabels(c) => {
                self.calls.lock().unwrap().push(Call::SetLabels(
                    t.clone(),
                    c.add.clone(),
                    c.remove.clone(),
                ));
                if failing.as_deref() == Some("set_labels") {
                    anyhow::bail!("422 label does not exist");
                }
                self.set_issue(t, |i| {
                    i.labels.retain(|l| !c.remove.contains(l));
                    for a in &c.add {
                        if !i.labels.contains(a) {
                            i.labels.push(a.clone());
                        }
                    }
                });
            }
            Change::AddComment(c) => {
                self.calls.lock().unwrap().push(Call::AddComment(
                    t.clone(),
                    c.body.clone(),
                    idempotency_key.to_string(),
                ));
                if failing.as_deref() == Some("add_comment") {
                    anyhow::bail!("500 from GitHub");
                }
                let mut comments = self.comments.lock().unwrap();
                let existing = comments.entry(t.clone()).or_default();
                if existing.iter().any(|(_, m)| m == idempotency_key) {
                    return Ok(());
                }
                existing.push((c.body.clone(), idempotency_key.to_string()));
            }
            Change::SetState(c) => {
                self.calls
                    .lock()
                    .unwrap()
                    .push(Call::SetState(t.clone(), c.state.clone()));
                self.set_issue(t, |i| i.state = c.state.clone());
            }
            other => anyhow::bail!("FakeWriter cannot apply {}", other.describe()),
        }
        self.touch(t);
        Ok(())
    }
}

pub const RISKS: [(&str, Risk); 5] = [
    ("approve", Risk::Low),
    ("set_labels", Risk::Low),
    ("comment", Risk::Low),
    ("close", Risk::High),
    ("reopen", Risk::Medium),
];

/// `approve` declares no words: it always asks.
pub fn asked_by(name: &str) -> Vec<String> {
    match name {
        "close" => vec!["close".into(), "shut".into()],
        _ => Vec::new(),
    }
}

pub fn register_fake(gate: &WriteGate, writer: Arc<FakeWriter>, allowed_repo: &str) {
    let repo = allowed_repo.to_string();
    let ops = RISKS
        .iter()
        .map(|(name, risk)| {
            let repo = repo.clone();
            Operation {
                name: (*name).to_string(),
                description: (*name).to_string(),
                properties: Value::Object(Map::new()),
                required: Vec::new(),
                prepare: Arc::new(|_args| {
                    Box::pin(async {
                        anyhow::bail!("the fake prepares nothing") as anyhow::Result<Prepared>
                    })
                }),
                writer: writer.clone(),
                risk: *risk,
                allowed: Arc::new(
                    move |t| matches!(t, Target::FakeIssue(i) if i.full_name() == repo),
                ),
                asked_by: asked_by(name),
                tool_name: None,
            }
        })
        .collect();
    gate.register("github_issues", ops);
}

pub fn changeset(steps: Vec<Change>) -> ChangeSet {
    ChangeSet::new(steps).unwrap()
}

pub fn issue(owner: &str, repo: &str, number: i64) -> FakeIssue {
    FakeIssue {
        owner: owner.into(),
        repo: repo.into(),
        number,
    }
}
