//! The gate bridge: every tool that is not read-only is a change through the gate, under
//! the app's name (`github_issue_write`); the words a request asks with are read off the
//! tool's name and short arguments (ADR 0017, ADR 0034).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::AppError;
use super::connection::App;
use super::tools::{ToolInfo, bound};
use crate::models::{AppChange, AppSnapshot, AppTarget, Change, ChangeSet, Snapshot, Target};
use crate::source::{Operation, Prepared, ReadTool, ToolSource, Writer};

/// The argument that carries the calling agent's slug to Ghost's own app (ADR 0045). The
/// session's dispatcher sets it, over anything the model wrote; `clean` drops a `_`-led
/// argument for every other app, so no app the operator connected ever sees it.
pub const CALLER_ARG: &str = "_agent";

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
        let answer = self.app.call(&c.tool, args, !self.app.is_own()).await?;
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
    let mut b = bound(tool, raw)?;
    if app.is_own()
        && let Some(Value::String(agent)) = raw.get(CALLER_ARG)
    {
        b.rest
            .insert(CALLER_ARG.into(), Value::String(agent.clone()));
    }
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
    /// What the model reads about a tool: an app's description up to its cap, which is
    /// untrusted text the prompt calls data (ADR 0046); the whole of Ghost's own, which
    /// is ours (ADR 0045).
    fn says(&self, tool: &ToolInfo) -> String {
        if self.is_own() {
            tool.description.clone()
        } else {
            tool.offered_description()
        }
    }

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
                description: self.says(&t),
                properties: Value::Object(t.properties()),
                required: t.required(),
                handler: Arc::new(move |args| {
                    let app = Arc::clone(&app);
                    let tool_name = tool_name.clone();
                    Box::pin(async move {
                        let trim = !app.is_own();
                        match app.call(&tool_name, super::tools::clean(&args), trim).await {
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
                self.says(&t),
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
    source_without(app, &HashSet::new())
}

/// The same, with the tools in `hidden` (by the model's names) not offered by name: they
/// are still there for the gate and for a call, and found by search (ADR 0046).
pub fn source_without(app: &Arc<App>, hidden: &HashSet<String>) -> ToolSource {
    let read_tools = app.read_tools();
    let operations = app.operations();
    let deferred: Vec<String> = read_tools
        .iter()
        .map(|t| t.name.clone())
        .chain(operations.iter().filter_map(|o| o.tool_name.clone()))
        .filter(|n| hidden.contains(n))
        .collect();
    ToolSource {
        name: app.name().to_string(),
        prompt: app.prompt_without(hidden),
        read_tools,
        operations,
        title: app.spec.shown(),
        // A call to Ghost's own app never leaves the process: nothing to acknowledge.
        app: !app.is_own(),
        deferred,
        ..Default::default()
    }
}
