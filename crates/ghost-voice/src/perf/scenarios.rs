//! Scripted conversations (`ghostd.perf.scenarios`). Each opens like a real wake (Ghost
//! greets), then plays the operator's lines one at a time, waiting for Ghost to finish
//! each answer. The model is scripted too: each line carries the replies the fake model
//! gives, tool calls included, so the mock GitHub app and the gate run for real.
//!
//! A line with `" ... "` has a pause in the middle. The checks are soft: a run that
//! drifts is reported, not failed. They read the mock GitHub server's state and the
//! latency records, never transcripts.

use serde_json::{Value, json};

use crate::llm::{ScriptedModel, ScriptedReply};

pub type Check = fn(&Value) -> Option<String>;

#[derive(Debug, Clone)]
pub struct Line {
    pub text: String,
    /// Tools the answer should use.
    pub tools: Vec<&'static str>,
    /// A change's expected status, e.g. "done:announce".
    pub status: Option<&'static str>,
    /// The session should end after this line.
    pub ends: bool,
    pub intercepted: bool,
    /// What the scripted model answers, call by call.
    pub replies: Vec<ScriptedReply>,
}

impl Line {
    fn new(text: &str, replies: Vec<ScriptedReply>) -> Self {
        Self {
            text: text.into(),
            tools: Vec::new(),
            status: None,
            ends: false,
            intercepted: false,
            replies,
        }
    }

    fn tools(mut self, tools: &[&'static str]) -> Self {
        self.tools = tools.to_vec();
        self
    }

    fn status(mut self, status: &'static str) -> Self {
        self.status = Some(status);
        self
    }

    fn ends(mut self) -> Self {
        self.ends = true;
        self
    }

    fn intercepted(mut self) -> Self {
        self.intercepted = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: &'static str,
    pub about: &'static str,
    pub lines: Vec<Line>,
    pub check: Option<Check>,
    pub tags: Vec<&'static str>,
}

fn issue<'a>(issues: &'a Value, key: &str) -> &'a Value {
    &issues[key]
}

fn closed_infra_17(issues: &Value) -> Option<String> {
    let state = issue(issues, "ghost-mock/infra#17")["state"]
        .as_str()
        .unwrap_or("missing");
    (state != "closed").then(|| format!("ghost-mock/infra#17 is {state}, expected closed"))
}

/// "ready for agent", "ready_for_agent" and "ready-for-agent" are one label here: the
/// check is that the write went through the gate, not how the model hyphenates.
fn slug(label: &str) -> String {
    label
        .trim()
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn labeled_app_151(issues: &Value) -> Option<String> {
    let labels: Vec<String> = issue(issues, "ghost-mock/app#151")["labels"]
        .as_array()
        .map(|ls| {
            ls.iter()
                .filter_map(|l| l["name"].as_str().map(slug))
                .collect()
        })
        .unwrap_or_default();
    (!labels.contains(&"ready-for-agent".to_string()))
        .then(|| "ghost-mock/app#151 lacks 'ready-for-agent'".to_string())
}

fn args(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn list_needs_triage() -> ScriptedReply {
    ScriptedModel::call(
        "",
        "github_list_issues",
        args(json!({"owner": "ghost-mock", "repo": "app", "labels": ["needs-triage"]})),
    )
}

fn read_issue(number: i64, method: &str) -> ScriptedReply {
    ScriptedModel::call(
        "",
        "github_issue_read",
        args(
            json!({"method": method, "owner": "ghost-mock", "repo": "app", "issue_number": number}),
        ),
    )
}

fn close_infra_17() -> ScriptedReply {
    ScriptedModel::call(
        "",
        "github_issue_write",
        args(
            json!({"method": "update", "owner": "ghost-mock", "repo": "infra", "issue_number": 17, "state": "closed"}),
        ),
    )
}

fn label_151() -> ScriptedReply {
    ScriptedModel::call(
        "",
        "github_issue_write",
        args(
            json!({"method": "update", "owner": "ghost-mock", "repo": "app", "issue_number": 151, "labels": ["ready-for-agent"]}),
        ),
    )
}

pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "greeting",
            about: "the wake greeting, then a hello",
            lines: vec![Line::new(
                "Hi Ghost, how's it going?",
                vec![ScriptedModel::say("Doing well. What can I do for you?")],
            )],
            check: None,
            tags: vec![],
        },
        Scenario {
            name: "small_talk",
            about: "a question with no tools",
            lines: vec![Line::new(
                "What's a good name for a gray cat?",
                vec![ScriptedModel::say("Ash, or Pepper if the cat has attitude.")],
            )],
            check: None,
            tags: vec![],
        },
        Scenario {
            name: "lookup",
            about: "one read tool",
            lines: vec![
                Line::new(
                    "What's waiting for triage?",
                    vec![
                        list_needs_triage(),
                        ScriptedModel::say("Three are waiting: the session cache choice, dark mode, and the webhook rotation."),
                    ],
                )
                .tools(&["github_list_issues"]),
            ],
            check: None,
            tags: vec![],
        },
        Scenario {
            name: "lookup_pause",
            about: "one read tool, a pause mid-sentence (the turn hold)",
            lines: vec![
                Line::new(
                    "What's waiting for triage ... in the app repo?",
                    vec![
                        list_needs_triage(),
                        ScriptedModel::say("Three are waiting: the session cache choice, dark mode, and the webhook rotation."),
                    ],
                )
                .tools(&["github_list_issues"]),
            ],
            check: None,
            tags: vec!["pause"],
        },
        Scenario {
            name: "chain",
            about: "an issue and what people said: issue_read for the issue and for its comments",
            lines: vec![
                Line::new(
                    "What's going on with app 142, and what have people said about it?",
                    vec![
                        ScriptedReply {
                            text: String::new(),
                            tool_calls: vec![
                                read_issue(142, "get").tool_calls.remove(0),
                                read_issue(142, "get_comments").tool_calls.remove(0),
                            ],
                        },
                        ScriptedModel::say("Issue one forty two is Redis versus Postgres for the session cache; two people lean toward Postgres."),
                    ],
                )
                .tools(&["github_issue_read"]),
            ],
            check: None,
            tags: vec![],
        },
        Scenario {
            name: "announce",
            about: "a change at announce: a label",
            lines: vec![
                Line::new(
                    "Mark app 151 ready for agent.",
                    vec![label_151(), ScriptedModel::say("Marked it ready for agent.")],
                )
                .tools(&["github_issue_write"])
                .status("done:announce"),
            ],
            check: Some(labeled_app_151),
            tags: vec![],
        },
        Scenario {
            name: "confirm_plain",
            about: "a change that asks, asked for plainly: the request is the yes (ADR 0017)",
            lines: vec![
                Line::new(
                    "Close infra 17.",
                    vec![close_infra_17(), ScriptedModel::say("Closed infra seventeen.")],
                )
                .tools(&["github_issue_write"])
                .status("done:request"),
            ],
            check: Some(closed_infra_17),
            tags: vec![],
        },
        Scenario {
            name: "confirm_yes",
            about: "a change that asks and needs a spoken yes (Jev or regex)",
            lines: vec![
                Line::new(
                    "I think infra 17 can probably be closed now.",
                    vec![close_infra_17(), ScriptedModel::say("Want me to close infra seventeen?")],
                )
                .tools(&["github_issue_write"])
                .status("needs_confirmation"),
                Line::new("Yes, go ahead.", vec![ScriptedModel::say("Closed it.")]),
            ],
            check: Some(closed_infra_17),
            tags: vec![],
        },
        Scenario {
            name: "bye_shortcut",
            about: "a goodbye the quick intents take without the model",
            lines: vec![Line::new("That's all for now, bye.", vec![]).ends().intercepted()],
            check: None,
            tags: vec![],
        },
        Scenario {
            name: "bye_model",
            about: "a goodbye only the model recognizes (hang_up)",
            lines: vec![
                Line::new(
                    "Okay, I'm heading out to lunch now, we can stop here.",
                    vec![ScriptedModel::call("Sounds good, talk soon!", "hang_up", args(json!({})))],
                )
                .tools(&["hang_up"])
                .ends(),
            ],
            check: None,
            tags: vec![],
        },
        Scenario {
            name: "session",
            about: "a realistic session: lookups, a change, goodbye (cache growth across turns)",
            lines: vec![
                Line::new(
                    "What's waiting for triage?",
                    vec![
                        list_needs_triage(),
                        ScriptedModel::say("Three are waiting: the session cache choice, dark mode, and the webhook rotation."),
                    ],
                )
                .tools(&["github_list_issues"]),
                Line::new(
                    "Tell me more about the dark mode one.",
                    vec![
                        read_issue(151, "get"),
                        ScriptedModel::say("Dark mode is a toggle in settings; three comments, nobody assigned yet."),
                    ],
                )
                .tools(&["github_issue_read"]),
                Line::new(
                    "Mark it ready for agent.",
                    vec![label_151(), ScriptedModel::say("Done.")],
                )
                .tools(&["github_issue_write"])
                .status("done:announce"),
                Line::new("Thanks, that's everything.", vec![]).ends().intercepted(),
            ],
            check: Some(labeled_app_151),
            tags: vec!["long"],
        },
    ]
}

pub const DEFAULT: [&str; 11] = [
    "greeting",
    "small_talk",
    "lookup",
    "lookup_pause",
    "chain",
    "announce",
    "confirm_plain",
    "confirm_yes",
    "bye_shortcut",
    "bye_model",
    "session",
];

pub fn by_name(name: &str) -> Option<Scenario> {
    scenarios().into_iter().find(|s| s.name == name)
}
