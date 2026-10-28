//! The tools offered to the model, bounded as `ghost_voice.tools` bounded them.
//!
//! Three kinds of tool reach the model: a source's typed read tools; each connected app's
//! changes, under the app's name like its reads (ADR 0034); and one shared `change` for
//! Ghost's own operations (the agents, the rules, the outputs), plus the session's tools
//! (switch, bring up, hang up, hold). A change of either kind goes to the gate; what
//! happens then is the operator's permissions (ADR 0011, ADR 0034). There is no undo tool:
//! to take a change back the model asks for the reversing change (ADR 0015).
//!
//! Only what the permissions don't block outright is offered (`Offers`), each name once,
//! and only a name the model API takes: it refuses the whole list over one bad name, which
//! would fail every turn. Ghost's own tools keep theirs in a clash, then the sources in
//! order with Ghost's reserved sources first; a tool left out is logged, once.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ghost_core::gate::WriteGate;
use ghost_core::permissions::Permissions;
use ghost_core::source::{Operation, ReadTool, ToolSource};
use serde_json::{Value, json};
use tracing::warn;

pub const CHANGE_TOOL: &str = "change";
pub const SWITCH_TOOL: &str = "switch_conversation";
pub const BRING_UP_TOOL: &str = "bring_up_change";
pub const HANG_UP_TOOL: &str = "hang_up";
pub const HOLD_TOOL: &str = "hold";

/// The names of Ghost's own sources: an app can't take one.
pub use ghost_core::apps::RESERVED_NAMES;

/// What the model API takes as a tool name: `[a-zA-Z0-9_-]{1,64}`.
pub fn tool_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// One tool as the model sees it (Pipecat's `FunctionSchema`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// The JSON Schema properties of its arguments.
    pub properties: Value,
    pub required: Vec<String>,
}

impl ToolSchema {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        properties: Value,
        required: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            properties,
            required,
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": self.properties,
            "required": self.required,
        })
    }

    /// The Messages API's shape.
    pub fn anthropic(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.parameters(),
        })
    }

    /// Chat Completions' shape.
    pub fn openai(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters(),
            },
        })
    }
}

/// The tools offered for one request, in the order the model is shown them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tools(Vec<ToolSchema>);

impl Tools {
    pub fn new(schemas: Vec<ToolSchema>) -> Self {
        Self(schemas)
    }

    pub fn names(&self) -> Vec<&str> {
        self.0.iter().map(|s| s.name.as_str()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&ToolSchema> {
        self.0.iter().find(|s| s.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolSchema> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn anthropic(&self) -> Vec<Value> {
        self.0.iter().map(ToolSchema::anthropic).collect()
    }

    pub fn openai(&self) -> Vec<Value> {
        self.0.iter().map(ToolSchema::openai).collect()
    }
}

/// What the operator's permissions leave offered: a read (`read`) or a change of
/// `source`'s `operation`. The gate answers this.
pub trait Offers: Send + Sync {
    fn offers(&self, source: &str, operation: &str, read: bool) -> bool;
}

impl Offers for WriteGate {
    fn offers(&self, source: &str, operation: &str, read: bool) -> bool {
        WriteGate::offers(self, source, operation, read)
    }
}

impl Offers for Permissions {
    fn offers(&self, source: &str, operation: &str, read: bool) -> bool {
        if read {
            self.offers_read(source, operation)
        } else {
            self.offers_change(source, operation)
        }
    }
}

/// Everything offered: a session built with no permissions to consult.
#[derive(Clone, Copy, Debug, Default)]
pub struct OfferAll;

impl Offers for OfferAll {
    fn offers(&self, _source: &str, _operation: &str, _read: bool) -> bool {
        true
    }
}

/// Which of the session's own tools this session has: `conversations` when the operator
/// can be transferred to an agent, `hold` when the session can be put on hold (ADR 0021).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionTools {
    pub conversations: bool,
    pub hold: bool,
}

/// What a tool name the model calls stands for: a source's read tool, or an app's change
/// under its own name (an operation with a `tool_name`).
#[derive(Clone)]
pub enum Offered<'a> {
    Read {
        tool: &'a ReadTool,
        source: &'a ToolSource,
    },
    Change {
        operation: &'a Operation,
        source: &'a str,
    },
}

impl std::fmt::Debug for Offered<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Offered::Read { tool, source } => f
                .debug_struct("Read")
                .field("tool", &tool.name)
                .field("source", &source.name)
                .finish(),
            Offered::Change { operation, source } => f
                .debug_struct("Change")
                .field("operation", &operation.name)
                .field("source", source)
                .finish(),
        }
    }
}

/// The tools a session offers, from the sources the active agent reaches: built once per
/// session and asked on every request and every call, since a source may have changed
/// (an app connected since the session was built) and what it offers may have (the
/// permissions). The names it couldn't offer are warned about once.
pub struct Offering {
    offers: Arc<dyn Offers>,
    pub session: SessionTools,
    warned: Mutex<BTreeSet<String>>,
}

impl std::fmt::Debug for Offering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Offering")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Offering {
    pub fn new(offers: Arc<dyn Offers>, session: SessionTools) -> Self {
        Self {
            offers,
            session,
            warned: Mutex::new(BTreeSet::new()),
        }
    }

    /// The session's own tools: switch (when there are agents), bring up a waiting change,
    /// hang up, hold (when the session can be held).
    pub fn session_schemas(&self) -> Vec<ToolSchema> {
        let mut out = Vec::new();
        if self.session.conversations {
            out.push(ToolSchema::new(
                SWITCH_TOOL,
                "Transfer the operator to one of their agents by name (\"Maya\", \"the brand \
                 agent\", \"ghost\" to come back). The transfer is announced and the agent \
                 greets them; say nothing else yourself. If nothing matches, the result says \
                 so: then ask in one short sentence whether to hire an agent for it, and on \
                 yes call change with conversations.create.",
                json!({
                    "name": {"type": "string", "description": "The agent to go to."},
                    "handoff": {
                        "type": "string",
                        "description": "One line for the other side about what the operator \
                                        wants, if the request said.",
                    },
                }),
                vec!["name".into()],
            ));
        }
        out.push(ToolSchema::new(
            BRING_UP_TOOL,
            "Take up a change waiting on the operator's waiting list (its 'change' id from \
             waiting_list) when they ask about it: returns 'done' if their words already \
             asked for exactly it, else 'needs_confirmation' with a summary to read out in \
             one short sentence before you stop.",
            json!({"change": {"type": "string"}}),
            vec!["change".into()],
        ));
        out.push(ToolSchema::new(
            HANG_UP_TOOL,
            "End the conversation: the operator said goodbye. Say a short, warm goodbye in \
             the same reply.",
            json!({}),
            Vec::new(),
        ));
        if self.session.hold {
            out.push(ToolSchema::new(
                HOLD_TOOL,
                "Wait: the operator asked you to hang on or turned to talk to someone else. \
                 You hear nothing until they say your name again; the conversation and any \
                 open question keep.",
                json!({}),
                Vec::new(),
            ));
        }
        out
    }

    /// Operations the operator's permissions do not block outright, per source, in the
    /// sources' order.
    pub fn operations<'a>(&self, sources: &'a [ToolSource]) -> Vec<(&'a str, Vec<&'a Operation>)> {
        let mut out = Vec::new();
        for a in sources {
            let offered: Vec<&Operation> = a
                .operations
                .iter()
                .filter(|op| self.offers.offers(&a.name, &op.name, false))
                .collect();
            if !offered.is_empty() {
                out.push((a.name.as_str(), offered));
            }
        }
        out
    }

    /// Read tools the permissions don't block, with their sources, in the sources' order.
    pub fn reads<'a>(&self, sources: &'a [ToolSource]) -> Vec<(&'a ReadTool, &'a ToolSource)> {
        let mut out = Vec::new();
        for a in sources {
            for tool in &a.read_tools {
                let operation = if tool.operation.is_empty() {
                    &tool.name
                } else {
                    &tool.operation
                };
                if self.offers.offers(&a.name, operation, true) {
                    out.push((tool, a));
                }
            }
        }
        out
    }

    /// Every read tool and app change offered, by the name the model calls it. Each name
    /// once, and only a name the model API takes. Ghost's own tools keep theirs in a
    /// clash, then the sources in order (the reserved ones first); a tool left out is
    /// logged, once.
    pub fn named<'a>(&self, sources: &'a [ToolSource]) -> Vec<(String, Offered<'a>)> {
        let own: BTreeSet<String> = std::iter::once(CHANGE_TOOL.to_string())
            .chain(self.session_schemas().into_iter().map(|s| s.name))
            .collect();
        let mut ordered: Vec<&ToolSource> = sources.iter().collect();
        ordered.sort_by_key(|a| !RESERVED_NAMES.contains(&a.name.as_str())); // stable
        let ordered: Vec<ToolSource> = ordered.into_iter().cloned().collect();
        let mut out: Vec<(String, Offered<'a>)> = Vec::new();
        let mut taken: BTreeSet<String> = BTreeSet::new();
        let mut add = |name: &str, entry: Offered<'a>, source: &str| {
            if own.contains(name) || taken.contains(name) || !tool_name_ok(name) {
                let mut warned = self.warned.lock().unwrap_or_else(|e| e.into_inner());
                if warned.insert(name.to_string()) {
                    let shown: String = name.chars().take(80).collect();
                    warn!(
                        "tools: {source}'s '{shown}' is not offered: another tool has the \
                         name, or the model can't take it"
                    );
                }
                return;
            }
            taken.insert(name.to_string());
            out.push((name.to_string(), entry));
        };
        // The sorted copies only give the order; the entries borrow from `sources`.
        for sorted in &ordered {
            let a = sources
                .iter()
                .find(|s| s.name == sorted.name)
                .expect("a source it was sorted from");
            for (tool, src) in self.reads(std::slice::from_ref(a)) {
                add(&tool.name, Offered::Read { tool, source: src }, &a.name);
            }
        }
        for sorted in &ordered {
            let a = sources
                .iter()
                .find(|s| s.name == sorted.name)
                .expect("a source it was sorted from");
            for (sname, ops) in self.operations(std::slice::from_ref(a)) {
                for op in ops {
                    if let Some(name) = &op.tool_name {
                        add(
                            name,
                            Offered::Change {
                                operation: op,
                                source: sname,
                            },
                            sname,
                        );
                    }
                }
            }
        }
        out
    }

    /// What `name` stands for among `sources`, if it is offered.
    pub fn lookup<'a>(&self, sources: &'a [ToolSource], name: &str) -> Option<Offered<'a>> {
        self.named(sources)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| o)
    }

    /// The tools advertised for a list of sources: their reads, each app's changes under
    /// their own names, `change` for Ghost's own operations, and the session's own tools.
    /// A transfer advertises the new agent's.
    pub fn schemas(&self, sources: &[ToolSource]) -> Tools {
        let mut out: Vec<ToolSchema> = self
            .named(sources)
            .into_iter()
            .map(|(name, offered)| match offered {
                Offered::Read { tool, .. } => ToolSchema::new(
                    name,
                    tool.description.clone(),
                    tool.properties.clone(),
                    tool.required.clone(),
                ),
                Offered::Change { operation, .. } => ToolSchema::new(
                    name,
                    operation.description.clone(),
                    operation.properties.clone(),
                    operation.required.clone(),
                ),
            })
            .collect();
        let ops = self.operations(sources);
        if ops
            .iter()
            .any(|(_, ops)| ops.iter().any(|op| op.tool_name.is_none()))
        {
            out.push(change_schema(&ops));
        }
        out.extend(self.session_schemas());
        Tools(out)
    }
}

/// A Python list of strings, as the prompt showed it (`['a', 'b']`).
fn py_list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let inner: Vec<String> = items.into_iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", inner.join(", "))
}

/// The one `change` tool for Ghost's own operations (those without a name of their own).
fn change_schema(ops: &[(&str, Vec<&Operation>)]) -> ToolSchema {
    let own: Vec<(&str, Vec<&Operation>)> = ops
        .iter()
        .filter_map(|(name, ops)| {
            let o: Vec<&Operation> = ops
                .iter()
                .copied()
                .filter(|op| op.tool_name.is_none())
                .collect();
            (!o.is_empty()).then_some((*name, o))
        })
        .collect();
    let op_lines: Vec<String> = own
        .iter()
        .flat_map(|(aname, ops)| {
            ops.iter().map(move |op| {
                let mut props: Vec<&str> = op
                    .properties
                    .as_object()
                    .map(|m| m.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                props.sort_unstable();
                format!(
                    "- {aname}.{}: {} Args: {} (required: {})",
                    op.name,
                    op.description,
                    py_list(props),
                    py_list(op.required.iter().map(String::as_str)),
                )
            })
        })
        .collect();
    let abilities: Vec<&str> = own.iter().map(|(n, _)| *n).collect();
    ToolSchema::new(
        CHANGE_TOOL,
        format!(
            "Make a change. Returns status 'done' with the verified result when the change \
             was applied at once, or 'needs_confirmation' with a summary when the operator \
             must okay it first: then ask in one short sentence and stop.\nOperations:\n{}",
            op_lines.join("\n")
        ),
        json!({
            "ability": {
                "type": "string",
                "enum": abilities,
                "description": "What it changes: 'conversations' (the agents), 'rules', \
                                'outputs' or 'skills'.",
            },
            "operation": {"type": "string"},
            "args": {"type": "object", "description": "Arguments for the operation."},
            "rationale": {"type": "string", "description": "One sentence: why."},
        }),
        vec!["ability".into(), "operation".into(), "args".into()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_name_is_what_the_model_api_takes() {
        assert!(tool_name_ok("github_list_issues"));
        assert!(tool_name_ok("a-b_C9"));
        assert!(!tool_name_ok(""));
        assert!(!tool_name_ok("bad.name"));
        assert!(!tool_name_ok("has space"));
        assert!(tool_name_ok(&"x".repeat(64)));
        assert!(!tool_name_ok(&"x".repeat(65)));
    }

    #[test]
    fn a_python_list_renders_as_the_prompt_showed_it() {
        assert_eq!(py_list(["name", "x"]), "['name', 'x']");
        assert_eq!(py_list([]), "[]");
    }

    #[test]
    fn schemas_take_each_protocols_shape() {
        let t = ToolSchema::new(
            "get_issue",
            "d",
            json!({"n": {"type": "integer"}}),
            vec!["n".into()],
        );
        assert_eq!(
            t.anthropic(),
            json!({"name": "get_issue", "description": "d", "input_schema": {
                "type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]}})
        );
        assert_eq!(
            t.openai(),
            json!({"type": "function", "function": {"name": "get_issue", "description": "d",
                "parameters": {"type": "object", "properties": {"n": {"type": "integer"}},
                "required": ["n"]}}})
        );
    }
}
