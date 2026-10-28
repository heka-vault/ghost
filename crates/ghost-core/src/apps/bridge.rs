//! The gate bridge: every tool that is not read-only is a change through the gate, under
//! the app's name (`github_issue_write`); the words a request asks with are read off the
//! tool's name and short arguments (ADR 0017, ADR 0034).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::AppError;
use super::connection::App;
use super::tools::{ToolInfo, bound};
use crate::models::{AppChange, AppSnapshot, AppTarget, Change, ChangeSet, Snapshot, Target};
use crate::source::{Operation, Prepared, ReadTool, ToolSource, Writer};

/// The gate's writer for an app's change: nothing to read first, then the call. What the
/// app answered comes back as the read-back, once, for the model to say.
pub struct AppWriter {
    app: Arc<App>,
    answers: Mutex<HashMap<String, Map<String, Value>>>,
}

impl AppWriter {
    pub fn new(app: Arc<App>) -> Self {
        Self {
            app,
            answers: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Writer for AppWriter {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot> {
        let result = self
            .answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&target.key());
        Ok(Snapshot::App(AppSnapshot {
            version: String::new(),
            checked: false,
            result,
        }))
    }

    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        _idempotency_key: &str,
    ) -> anyhow::Result<()> {
        let (Target::App(t), Change::App(c)) = (target, change) else {
            anyhow::bail!("not a change to an app");
        };
        let mut args = t.fields.clone();
        args.extend(c.arguments.iter().map(|(k, v)| (k.clone(), v.clone())));
        let answer = self.app.call(&c.tool, args, true).await?;
        self.answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(target.key(), answer);
        Ok(())
    }
}

/// The model's arguments, checked against the tool's own schema and bounded, as a
/// target, a change set and the words that ask for it.
fn prepare(app: &App, tool: &ToolInfo, raw: &Map<String, Value>) -> Result<Prepared, AppError> {
    let b = bound(tool, raw)?;
    let rationale = match raw.get("rationale") {
        Some(Value::String(s)) => s.clone(),
        Some(v) if super::spec::truthy(v) => crate::models::py_str(v),
        _ => String::new(),
    };
    Ok(Prepared {
        target: Target::App(AppTarget {
            app: app.spec.name.clone(),
            tool: tool.name.clone(),
            fields: b.fields,
            title: app.spec.shown(),
        }),
        changes: ChangeSet::one(Change::App(AppChange {
            tool: tool.name.clone(),
            arguments: b.rest,
        })),
        rationale,
        asked_by: b.verbs,
    })
}

impl App {
    pub fn read_tools(self: &Arc<Self>) -> Vec<ReadTool> {
        let offered = self.offered();
        let mut tools = Vec::new();
        for t in self.reads() {
            let Some((_, name)) = offered.iter().find(|(n, _)| *n == t.name) else {
                continue;
            };
            let app = Arc::clone(self);
            let tool_name = t.name.clone();
            tools.push(ReadTool {
                name: name.clone(),
                description: t.brief(),
                properties: Value::Object(t.properties()),
                required: t.required(),
                handler: Arc::new(move |args| {
                    let app = Arc::clone(&app);
                    let tool_name = tool_name.clone();
                    Box::pin(async move {
                        match app.call(&tool_name, super::tools::clean(&args), true).await {
                            Ok(out) => Ok(out),
                            Err(e) => Ok(Map::from_iter([(
                                "error".to_string(),
                                Value::String(e.to_string()),
                            )])),
                        }
                    })
                }),
                operation: t.name.clone(),
            });
        }
        tools
    }

    /// Every tool that is not read-only, as a change through the gate.
    pub fn operations(self: &Arc<Self>) -> Vec<Operation> {
        let writer: Arc<dyn Writer> = Arc::new(AppWriter::new(Arc::clone(self)));
        let offered = self.offered();
        let mut ops = Vec::new();
        for t in self.changes() {
            let Some((_, name)) = offered.iter().find(|(n, _)| *n == t.name) else {
                continue;
            };
            let app = Arc::clone(self);
            let tool = t.clone();
            let mut op = Operation::new(
                t.name.clone(),
                t.brief(),
                Arc::new(move |args| {
                    let app = Arc::clone(&app);
                    let tool = tool.clone();
                    Box::pin(
                        async move { prepare(&app, &tool, &args).map_err(anyhow::Error::from) },
                    )
                }),
                writer.clone(),
            );
            op.properties = Value::Object(t.properties());
            op.required = t.required();
            op.tool_name = Some(name.clone());
            ops.push(op);
        }
        ops
    }
}

/// What the pipeline and the gate take from a connected app.
pub fn source_of(app: &Arc<App>) -> ToolSource {
    ToolSource {
        name: app.name().to_string(),
        prompt: app.prompt(),
        read_tools: app.read_tools(),
        operations: app.operations(),
        title: app.spec.shown(),
        ..Default::default()
    }
}
