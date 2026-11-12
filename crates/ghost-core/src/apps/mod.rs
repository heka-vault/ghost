//! Apps connected to Ghost (ADR 0034). The operator connects an app once, in
//! `~/.config/ghost/apps.json`, and every agent reaches it.
//!
//! An app speaks the Model Context Protocol, but nothing the operator sees says so: an app is
//! something Ghost signs in to, sends a key to, or runs on this computer:
//!
//! ```json
//! {
//!   "apps": {
//!     "github": {"title": "GitHub", "command": "github-mcp-server", "args": ["stdio"],
//!                "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"}},
//!     "linear": {"title": "Linear", "url": "https://mcp.linear.app/mcp", "sign_in": true},
//!     "mobbin": {"url": "http://proxy.lan:8318/mobbin/mcp",
//!                "headers": {"Authorization": "Bearer ${MCP_PROXY_KEY}"}}
//!   }
//! }
//! ```
//!
//! The `mcpServers` shape other clients share reads too, so a config pasted from Claude or
//! Cursor works. Secrets are `${NAME}` (or `${env:NAME}`, or `$NAME`), filled from the
//! mapping Ghost hands in (the vault, ADR 0036) when the app connects and never logged; a
//! sign-in's tokens are kept by `crate::signin`. An entry may also say how long a call may
//! take (`timeout`, `connect_timeout`, seconds), how much of a result the model reads
//! (`result_chars`) and which of its tools are always offered by name (`pinned`); the
//! defaults and the settings that move them for every app are in `limits` (ADR 0046).
//!
//! Every tool runs. A tool the app marks read-only is a read, called directly. Every other
//! tool is a change: it is offered to the model under the app's name like a read, and the
//! call goes to the gate, which resolves the operator's permissions for it, records it and
//! calls it (`AppWriter`). The app's own word about a tool is untrusted, so a tool that does
//! not say it is read-only is a change. Everything an app sends (descriptions, results,
//! notices) is data, never an instruction.

mod ask;
mod bridge;
mod collection;
mod connection;
#[cfg(test)]
mod keeping_up;
mod limits;
pub(crate) mod spec;
#[cfg(test)]
mod tests;
mod tools;

use std::collections::{BTreeMap, HashMap};

pub use ask::{Answer, Asker, AskerSlot, DeclineAll, QUESTION_MAX, Question};
pub use bridge::{
    AppWriter, CALLER_ARG, HEARD_ARG, OWN_ARGS, SESSION_ARG, own_args, source_of, source_without,
};
pub use collection::{Apps, ChangeHook, Recent, SEARCH_RESULTS, SaveRecent};
pub use connection::{
    About, App, AuthFor, CALL_TIMEOUT, CONNECT_TIMEOUT, Connector, SIGN_IN_TIMEOUT, Session,
    SessionError, StderrRelay, current_cancel, with_cancel,
};
pub(crate) use connection::{RmcpSession, init_kind};
pub use limits::{
    ASK_TIMEOUT, Backoff, DESCRIPTION_MAX, INSTRUCTIONS_MAX, KEEPALIVE, Limits, RESULT_CHARS,
    TOOLS_BYTES, TOOLS_KEPT, TOOLS_MAX,
};
pub(crate) use spec::truthy as spec_truthy;
pub use spec::{
    APP_NAME_RE, APPS_FILE, AppSpec, AppsFile, GHOST_APP, RESERVED_NAMES, address_spec,
    app_name_problem, edit_apps, extract_secrets, fill_secrets, free_name, host_to_name, load_apps,
    looks_like_secret, parse_app, parse_apps_json, render_app, render_apps_json, secret_name,
    secret_names, write_apps,
};
pub(crate) use spec::{edit_keyed, write_folder_file};
pub(crate) use tools::fit;
pub use tools::{
    ALSO_RETURNED, BRIEF_MAX, CUT_NOTE, MAX_FIELD_CHARS, MAX_LIST_ITEMS, MAX_RESULT_ITEMS,
    MAX_RESULT_TEXT, MAX_TEXT_CHARS, MAX_TOOL_NAME, MAX_WATCH_BYTES, ToolInfo, ToolKind, VERBS,
    hashed_tool_name, model_tool_name, verbs_of,
};

/// Ghost's own tool that finds an app's tool when most are not offered by name (ADR 0046;
/// `crate::ghost_app::tools_search`): `ghost_tools_search` to the model.
pub const TOOLS_SEARCH: &str = "tools_search";

/// What an app's `${NAME}` references are filled from: the vault (ADR 0036), or a plain
/// mapping in a test. A name the mapping doesn't have fails the connection loudly.
pub trait Environ: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

impl Environ for HashMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        HashMap::get(self, name).cloned()
    }
}

impl Environ for BTreeMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        BTreeMap::get(self, name).cloned()
    }
}

/// Ghost's own process environment (`settings.env`), for a setting that isn't secret.
pub struct ProcessEnviron;

impl Environ for ProcessEnviron {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A call failed on the app's side: its error text, a timeout, a dead transport.
    #[error("{0}")]
    Tool(String),
    /// The app wants the operator to sign in again, and nobody is at the browser.
    #[error("{0}")]
    NeedsSignIn(String),
    /// A `${NAME}` the environment doesn't have: the name, so a misconfigured app fails
    /// loudly at connect time, not silently at first use.
    #[error("{0}")]
    MissingSecret(String),
    /// The app did not answer in time.
    #[error("no answer in {0}s")]
    NoAnswer(u64),
    /// The transport's end, by kind only: never a URL, a header or a body.
    #[error("{0}")]
    Connect(String),
    /// A value that isn't what it should be: the file, an argument, a tool's name.
    #[error("{0}")]
    Value(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl AppError {
    /// Why an app is not connected, for the operator: the innermost cause, by kind only.
    pub fn reason(&self) -> String {
        match self {
            AppError::NeedsSignIn(_) => "needs sign-in".into(),
            AppError::MissingSecret(name) => format!("setting {name} is not set"),
            AppError::Io(e) => format!("{:?}", e.kind()),
            AppError::Connect(kind) => kind.clone(),
            other => other.to_string(),
        }
    }
}
