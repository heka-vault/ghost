//! A pretend GitHub as an MCP server, for running Ghost before a token exists and for the
//! latency harness. Same tool shape as the official `github-mcp-server` issues toolset
//! (`issue_read` and `issue_write` multiplexed by `method`, `add_issue_comment`,
//! `list_issues`, `search_issues`), so the same connection drives both.
//!
//! Run it over stdio: `ghost-mock-github --state PATH`. With a state file every write is
//! saved there and a restart picks it up again, so the audit log and the conversation history
//! outlive the process; delete the file to start over from the seed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

pub const MOCK_REPOS: [&str; 2] = ["ghost-mock/app", "ghost-mock/infra"];

struct Seed {
    repo: &'static str,
    number: i64,
    title: &'static str,
    labels: &'static [&'static str],
    body: &'static str,
}

const SEED: [Seed; 4] = [
    Seed {
        repo: "ghost-mock/app",
        number: 142,
        title: "Choose Redis or Postgres for the session cache",
        labels: &["needs-triage", "backend"],
        body: "We need a session cache before launch. Redis is simpler to operate; Postgres \
               avoids a new service. Blocks #143 and #144.",
    },
    Seed {
        repo: "ghost-mock/app",
        number: 151,
        title: "Add dark mode toggle to settings",
        labels: &["needs-triage", "frontend"],
        body: "Design is attached. Needs a decision on whether to follow the OS setting by default.",
    },
    Seed {
        repo: "ghost-mock/infra",
        number: 17,
        title: "Rotate the Discord webhook",
        labels: &["needs-triage", "security"],
        body: "The URL was printed into a session log. Create a new webhook and update the config.",
    },
    Seed {
        repo: "ghost-mock/app",
        number: 98,
        title: "Flaky test in payments suite",
        labels: &["bug"],
        body: "test_refund_partial fails about one run in ten.",
    },
];

fn seed_comments(key: &str) -> Vec<Comment> {
    if key == "ghost-mock/app#142" {
        vec![Comment {
            id: 1,
            user: User {
                login: "dev".into(),
            },
            created_at: "2026-10-15T14:02:00Z".into(),
            body: "Leaning Redis unless ops objects.".into(),
        }]
    } else {
        Vec::new()
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MockGitHubError {
    #[error("{0} does not exist")]
    NoSuchIssue(String),
    #[error("unknown method {0}")]
    UnknownMethod(String),
    #[error("issue_number is required for update")]
    IssueNumberRequired,
    #[error("state must be open or closed")]
    BadState,
    /// The state file could not be read or written.
    #[error("{0}")]
    State(String),
}

/// A tool's answer as the Python SDK wrote it: JSON text, two spaces of indent, the
/// fields in the order the official server has them.
fn text<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

#[derive(Serialize)]
struct Comments<'a> {
    comments: &'a [Comment],
}

#[derive(Serialize)]
struct Labels<'a> {
    labels: &'a [Label],
}

#[derive(Serialize)]
struct Listed<'a> {
    issues: &'a [&'a Issue],
    #[serde(rename = "totalCount")]
    total_count: usize,
}

#[derive(Serialize)]
struct Found<'a> {
    items: &'a [&'a Issue],
    total_count: usize,
}

fn now() -> String {
    crate::time::utcnow()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Python's `repr` of a string, for the messages that quote what was asked.
fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub login: String,
}

/// `null` in the file is `Some(None)`: an issue reopened; a key absent is `None`.
fn null_is_some<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// An issue as the official server returns one (the fields Ghost reads).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub number: i64,
    pub title: String,
    pub state: String,
    pub labels: Vec<Label>,
    pub body: String,
    pub user: User,
    pub comments: i64,
    pub updated_at: String,
    pub html_url: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "null_is_some"
    )]
    pub state_reason: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub user: User,
    pub created_at: String,
    pub body: String,
}

/// A JSON object kept in insertion order, as a Python dict is: `Vec<(key, value)>`.
mod ordered {
    use std::fmt;
    use std::marker::PhantomData;

    use serde::de::{MapAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer, V: serde::Serialize>(
        rows: &[(String, V)],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(rows.len()))?;
        for (k, v) in rows {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>, V: Deserialize<'de>>(
        d: D,
    ) -> Result<Vec<(String, V)>, D::Error> {
        struct OrderedVisitor<V>(PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for OrderedVisitor<V> {
            type Value = Vec<(String, V)>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::with_capacity(m.size_hint().unwrap_or(0));
                while let Some((k, v)) = m.next_entry::<String, V>()? {
                    out.push((k, v));
                }
                Ok(out)
            }
        }
        d.deserialize_map(OrderedVisitor(PhantomData))
    }
}

/// What the state file holds.
#[derive(Serialize, Deserialize)]
struct State {
    #[serde(with = "ordered")]
    issues: Vec<(String, Issue)>,
    #[serde(with = "ordered")]
    comments: Vec<(String, Vec<Comment>)>,
    next_comment_id: i64,
}

// --- the tools' arguments, as the official server names them --------------------------

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct IssueRead {
    pub method: String,
    pub owner: String,
    pub repo: String,
    pub issue_number: i64,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct IssueWrite {
    pub method: String,
    pub owner: String,
    pub repo: String,
    #[serde(default)]
    pub issue_number: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub state_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct AddIssueComment {
    pub owner: String,
    pub repo: String,
    pub issue_number: i64,
    pub body: String,
}

fn default_state() -> String {
    "open".into()
}

fn default_per_page() -> i64 {
    30
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct ListIssues {
    pub owner: String,
    pub repo: String,
    #[serde(default = "default_state")]
    pub state: String,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// The official server's spelling.
    #[serde(default = "default_per_page", rename = "perPage")]
    pub per_page: i64,
}

impl Default for ListIssues {
    fn default() -> Self {
        ListIssues {
            owner: String::new(),
            repo: String::new(),
            state: default_state(),
            labels: None,
            per_page: default_per_page(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct SearchIssues {
    pub query: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
}

/// The state behind the tools: issues and comments keyed `owner/repo#number`.
#[derive(Debug)]
pub struct MockGitHub {
    path: Option<PathBuf>,
    pub issues: Vec<(String, Issue)>,
    pub comments: Vec<(String, Vec<Comment>)>,
    next_comment_id: i64,
}

impl MockGitHub {
    /// The seed, or the state file's contents when there is one and it exists.
    pub fn new(state_path: Option<&Path>) -> Result<Self, MockGitHubError> {
        let mut gh = MockGitHub {
            path: state_path.map(Path::to_path_buf),
            issues: Vec::new(),
            comments: Vec::new(),
            next_comment_id: 1000,
        };
        if let Some(path) = state_path
            && path.exists()
        {
            let text = std::fs::read_to_string(path)
                .map_err(|e| MockGitHubError::State(format!("{}: {e}", path.display())))?;
            let data: State = serde_json::from_str(&text)
                .map_err(|e| MockGitHubError::State(format!("{}: {e}", path.display())))?;
            gh.issues = data.issues;
            gh.comments = data.comments;
            gh.next_comment_id = data.next_comment_id;
            return Ok(gh);
        }
        for s in &SEED {
            let key = format!("{}#{}", s.repo, s.number);
            let comments = seed_comments(&key);
            gh.issues.push((
                key.clone(),
                Issue {
                    number: s.number,
                    title: s.title.into(),
                    state: "open".into(),
                    labels: s
                        .labels
                        .iter()
                        .map(|l| Label { name: (*l).into() })
                        .collect(),
                    body: s.body.into(),
                    user: User {
                        login: "dev".into(),
                    },
                    comments: comments.len() as i64,
                    updated_at: "2026-10-16T09:00:00Z".into(),
                    html_url: format!("https://github.com/{}/issues/{}", s.repo, s.number),
                    state_reason: None,
                },
            ));
            gh.comments.push((key, comments));
        }
        Ok(gh)
    }

    pub fn save(&self) -> Result<(), MockGitHubError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let data = State {
            issues: self.issues.clone(),
            comments: self.comments.clone(),
            next_comment_id: self.next_comment_id,
        };
        let mut text = Vec::new();
        let mut ser = serde_json::Serializer::with_formatter(
            &mut text,
            serde_json::ser::PrettyFormatter::with_indent(b" "),
        );
        data.serialize(&mut ser)
            .map_err(|e| MockGitHubError::State(format!("{}: {e}", path.display())))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &text)
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(|e| MockGitHubError::State(format!("{}: {e}", path.display())))
    }

    fn key(&self, owner: &str, repo: &str, number: i64) -> Result<String, MockGitHubError> {
        let key = format!("{owner}/{repo}#{number}");
        if !self.issues.iter().any(|(k, _)| *k == key) {
            return Err(MockGitHubError::NoSuchIssue(key));
        }
        Ok(key)
    }

    fn issue(&self, key: &str) -> &Issue {
        &self
            .issues
            .iter()
            .find(|(k, _)| k == key)
            .expect("a checked key")
            .1
    }

    fn issue_mut(&mut self, key: &str) -> &mut Issue {
        &mut self
            .issues
            .iter_mut()
            .find(|(k, _)| k == key)
            .expect("a checked key")
            .1
    }

    fn comments_mut(&mut self, key: &str) -> &mut Vec<Comment> {
        &mut self
            .comments
            .iter_mut()
            .find(|(k, _)| k == key)
            .expect("a checked key")
            .1
    }

    // --- tools --------------------------------------------------------------------

    pub fn issue_read(&self, p: &IssueRead) -> Result<String, MockGitHubError> {
        let key = self.key(&p.owner, &p.repo, p.issue_number)?;
        match p.method.as_str() {
            "get" => Ok(text(self.issue(&key))),
            "get_comments" => {
                let all = self
                    .comments
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, c)| c.as_slice())
                    .unwrap_or(&[]);
                let last = &all[all.len().saturating_sub(10)..];
                Ok(text(&Comments { comments: last }))
            }
            "get_labels" => Ok(text(&Labels {
                labels: &self.issue(&key).labels,
            })),
            other => Err(MockGitHubError::UnknownMethod(repr(other))),
        }
    }

    pub fn issue_write(&mut self, p: &IssueWrite) -> Result<String, MockGitHubError> {
        if p.method == "create" {
            let number = self.issues.iter().map(|(_, i)| i.number).max().unwrap_or(0) + 1;
            let key = format!("{}/{}#{}", p.owner, p.repo, number);
            let issue = Issue {
                number,
                title: p.title.clone().unwrap_or_default(),
                state: "open".into(),
                labels: p
                    .labels
                    .iter()
                    .flatten()
                    .map(|l| Label { name: l.clone() })
                    .collect(),
                body: p.body.clone().unwrap_or_default(),
                user: User {
                    login: "operator".into(),
                },
                comments: 0,
                updated_at: now(),
                html_url: format!(
                    "https://github.com/{}/{}/issues/{}",
                    p.owner, p.repo, number
                ),
                state_reason: None,
            };
            let out = text(&issue);
            self.issues.push((key.clone(), issue));
            self.comments.push((key, Vec::new()));
            self.save()?;
            return Ok(out);
        }
        if p.method != "update" {
            return Err(MockGitHubError::UnknownMethod(repr(&p.method)));
        }
        let Some(number) = p.issue_number else {
            return Err(MockGitHubError::IssueNumberRequired);
        };
        let key = self.key(&p.owner, &p.repo, number)?;
        if let Some(state) = &p.state
            && state != "open"
            && state != "closed"
        {
            return Err(MockGitHubError::BadState);
        }
        let stamp = now();
        let issue = self.issue_mut(&key);
        if let Some(title) = &p.title {
            issue.title = title.clone();
        }
        if let Some(body) = &p.body {
            issue.body = body.clone();
        }
        if let Some(labels) = &p.labels {
            issue.labels = labels.iter().map(|l| Label { name: l.clone() }).collect();
        }
        if let Some(state) = &p.state {
            issue.state = state.clone();
            issue.state_reason = Some(if state == "closed" {
                p.state_reason.clone()
            } else {
                None
            });
        }
        issue.updated_at = stamp;
        let out = text(&issue);
        self.save()?;
        Ok(out)
    }

    pub fn add_issue_comment(&mut self, p: &AddIssueComment) -> Result<String, MockGitHubError> {
        let key = self.key(&p.owner, &p.repo, p.issue_number)?;
        self.next_comment_id += 1;
        let comment = Comment {
            id: self.next_comment_id,
            user: User {
                login: "operator".into(),
            },
            created_at: now(),
            body: p.body.clone(),
        };
        let out = text(&comment);
        self.comments_mut(&key).push(comment);
        let issue = self.issue_mut(&key);
        issue.comments += 1;
        issue.updated_at = now();
        self.save()?;
        Ok(out)
    }

    pub fn list_issues(&self, p: &ListIssues) -> Result<String, MockGitHubError> {
        let want: Vec<&str> = p.labels.iter().flatten().map(String::as_str).collect();
        let prefix = format!("{}/{}#", p.owner, p.repo);
        let mut out: Vec<&Issue> = self
            .issues
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, i)| i)
            .filter(|i| p.state == "all" || i.state == p.state)
            .filter(|i| want.iter().all(|w| i.labels.iter().any(|l| l.name == *w)))
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let total = out.len();
        let per_page = usize::try_from(p.per_page).unwrap_or(0).min(total);
        Ok(text(&Listed {
            issues: &out[..per_page],
            total_count: total,
        }))
    }

    pub fn search_issues(&self, p: &SearchIssues) -> Result<String, MockGitHubError> {
        let lower = p.query.to_lowercase();
        let words: Vec<&str> = lower
            .split_whitespace()
            .filter(|w| !w.contains(':'))
            .collect();
        let mut out: Vec<&Issue> = Vec::new();
        for (key, i) in &self.issues {
            if let Some(owner) = &p.owner
                && !key.starts_with(&format!("{owner}/"))
            {
                continue;
            }
            if let Some(repo) = &p.repo
                && !key.contains(&format!("/{repo}#"))
            {
                continue;
            }
            let text = format!("{} {}", i.title, i.body).to_lowercase();
            if words.iter().all(|w| text.contains(w)) {
                out.push(i);
            }
        }
        let total = out.len();
        Ok(text(&Found {
            items: &out[..total.min(30)],
            total_count: total,
        }))
    }
}

/// A tool's answer as the Python SDK sent it: the value as JSON text, or an error result
/// carrying the reason.
fn reply(tool: &str, result: Result<String, MockGitHubError>) -> CallToolResult {
    match result {
        Ok(v) => CallToolResult::success(vec![ContentBlock::text(v)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(format!(
            "Error executing tool {tool}: {e}"
        ))]),
    }
}

/// The MCP server over `MockGitHub`: tool names, argument names and read-only hints
/// as the official server has them.
pub struct MockGithubServer {
    gh: Mutex<MockGitHub>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl MockGithubServer {
    pub fn new(state_path: Option<&Path>) -> Result<Self, MockGitHubError> {
        Ok(MockGithubServer {
            gh: Mutex::new(MockGitHub::new(state_path)?),
            tool_router: Self::tool_router(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MockGitHub> {
        self.gh.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The tools this server offers, as the client lists them.
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    #[tool(
        description = "Get information about a specific issue in a GitHub repository (method: get, get_comments, get_labels).",
        annotations(read_only_hint = true)
    )]
    fn issue_read(&self, Parameters(p): Parameters<IssueRead>) -> CallToolResult {
        reply("issue_read", self.lock().issue_read(&p))
    }

    #[tool(
        description = "Create a new or update an existing issue in a GitHub repository (method: create, update)."
    )]
    fn issue_write(&self, Parameters(p): Parameters<IssueWrite>) -> CallToolResult {
        reply("issue_write", self.lock().issue_write(&p))
    }

    #[tool(description = "Add a comment to a specific issue in a GitHub repository.")]
    fn add_issue_comment(&self, Parameters(p): Parameters<AddIssueComment>) -> CallToolResult {
        reply("add_issue_comment", self.lock().add_issue_comment(&p))
    }

    #[tool(
        description = "List issues in a GitHub repository, newest first, optionally only those carrying every label given.",
        annotations(read_only_hint = true)
    )]
    fn list_issues(&self, Parameters(p): Parameters<ListIssues>) -> CallToolResult {
        reply("list_issues", self.lock().list_issues(&p))
    }

    #[tool(
        description = "Search for issues in GitHub repositories by words in their title or body.",
        annotations(read_only_hint = true)
    )]
    fn search_issues(&self, Parameters(p): Parameters<SearchIssues>) -> CallToolResult {
        reply("search_issues", self.lock().search_issues(&p))
    }
}

#[tool_handler(router = self.tool_router, name = "mock-github")]
impl ServerHandler for MockGithubServer {}

#[cfg(test)]
mod tests {
    //! The mock GitHub MCP server: the official server's tool shape, state that survives a
    //! restart, and the comment marker that makes a retried comment land once.

    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;

    fn parsed(text: String) -> Value {
        serde_json::from_str(&text).unwrap()
    }

    fn numbers(v: &Value, list: &str) -> BTreeSet<i64> {
        v[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["number"].as_i64().unwrap())
            .collect()
    }

    #[test]
    fn seeded_state_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("mock.json");
        let mut gh = MockGitHub::new(Some(&state)).unwrap();
        let listed = gh
            .list_issues(&ListIssues {
                owner: "ghost-mock".into(),
                repo: "app".into(),
                labels: Some(vec!["needs-triage".into()]),
                ..Default::default()
            })
            .unwrap();
        let listed = parsed(listed);
        assert_eq!(numbers(&listed, "issues"), BTreeSet::from([142, 151]));
        gh.issue_write(&IssueWrite {
            method: "update".into(),
            owner: "ghost-mock".into(),
            repo: "infra".into(),
            issue_number: Some(17),
            state: Some("closed".into()),
            state_reason: Some("completed".into()),
            ..Default::default()
        })
        .unwrap();
        gh.add_issue_comment(&AddIssueComment {
            owner: "ghost-mock".into(),
            repo: "app".into(),
            issue_number: 142,
            body: "Go with Redis.".into(),
        })
        .unwrap();
        let again = MockGitHub::new(Some(&state)).unwrap(); // a restart picks the writes up
        let got = again
            .issue_read(&IssueRead {
                method: "get".into(),
                owner: "ghost-mock".into(),
                repo: "infra".into(),
                issue_number: 17,
            })
            .unwrap();
        let got = parsed(got);
        assert_eq!(got["state"], "closed");
        let got = again
            .issue_read(&IssueRead {
                method: "get_comments".into(),
                owner: "ghost-mock".into(),
                repo: "app".into(),
                issue_number: 142,
            })
            .unwrap();
        let got = parsed(got);
        let comments = got["comments"].as_array().unwrap();
        assert_eq!(comments.last().unwrap()["body"], "Go with Redis.");
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
        assert_eq!(
            file["issues"]["ghost-mock/infra#17"]["state_reason"],
            "completed"
        );
    }

    #[test]
    fn search_and_create() {
        let mut gh = MockGitHub::new(None).unwrap();
        let found = gh
            .search_issues(&SearchIssues {
                query: "webhook".into(),
                ..Default::default()
            })
            .unwrap();
        let found = parsed(found);
        assert_eq!(
            found["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["number"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [17]
        );
        let made = gh
            .issue_write(&IssueWrite {
                method: "create".into(),
                owner: "ghost-mock".into(),
                repo: "app".into(),
                title: Some("New one".into()),
                labels: Some(vec!["needs-triage".into()]),
                ..Default::default()
            })
            .unwrap();
        let made = parsed(made);
        assert_eq!(made["number"], 152);
        assert_eq!(made["state"], "open");
    }

    #[test]
    fn server_has_the_official_tool_shape() {
        let srv = MockGithubServer::new(None).unwrap();
        let tools: std::collections::BTreeMap<String, rmcp::model::Tool> = srv
            .tools()
            .into_iter()
            .map(|t| (t.name.to_string(), t))
            .collect();
        assert_eq!(
            tools.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "issue_read",
                "issue_write",
                "add_issue_comment",
                "list_issues",
                "search_issues",
            ])
        );
        assert_eq!(
            tools["issue_read"]
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint),
            Some(true)
        );
        assert!(tools["issue_write"].annotations.is_none());
        let properties: BTreeSet<&str> = tools["issue_write"].input_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for want in ["method", "owner", "repo", "issue_number", "labels", "state"] {
            assert!(
                properties.contains(want),
                "{want} missing from {properties:?}"
            );
        }
        let per_page: Vec<&str> = tools["list_issues"].input_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(per_page.contains(&"perPage"));
    }
}
