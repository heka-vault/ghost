//! One tool's entries in one shape, and each as Ghost would connect it (ADR 0068).
//!
//! The formats, from each tool's documentation:
//!
//! - Claude Code (code.claude.com/docs/en/mcp): `mcpServers` at the top of
//!   `~/.claude.json` (user scope), under `projects.<path>.mcpServers` there (local scope),
//!   and in a project's `.mcp.json` (project scope). An entry is `command`, `args`, `env`,
//!   or `type` (`http`, `sse`, `ws`, `streamable-http`), `url`, `headers`,
//!   `headersHelper`, `oauth`; `${VAR}` and `${VAR:-default}` in its values.
//! - Cursor (cursor.com/docs/context/mcp): `mcpServers` in `~/.cursor/mcp.json`;
//!   `type`, `command`, `args`, `env`, `envFile`, or `url`, `headers`, `auth`; `${env:NAME}`,
//!   `${userHome}`, `${workspaceFolder}` and the like in its values.
//! - Codex (learn.chatgpt.com/docs/extend/mcp): `[mcp_servers.<name>]` in
//!   `~/.codex/config.toml`; `command`, `args`, `env`, `env_vars`, `cwd`, or `url`,
//!   `bearer_token_env_var`, `http_headers`, `env_http_headers`; OAuth by default for an
//!   address.

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use zeroize::Zeroizing;

use super::{
    ENTRIES_MAX, FileSeen, FileState, FoundApp, Ghost, Linked, Planned, Tool, Where, bounded,
};
use crate::apps::{
    AppSpec, extract_secrets, free_name, json_entries_at, secret_name, secret_names,
};

/// Why a found app is not offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Named `ghost`: Ghost's own name.
    Reserved,
    /// The same as an app already connected, by that name.
    Connected(String),
    /// The same as one found in another tool (or the same tool), offered there.
    Same { tool: String, name: String },
    /// Something Ghost cannot connect, in a sentence.
    Unsupported(String),
}

impl Skip {
    pub fn sentence(&self) -> String {
        match self {
            Skip::Reserved => "Named ghost, which is Ghost's own name.".into(),
            Skip::Connected(name) => format!("Already connected, as {name}."),
            Skip::Same { tool, name } => format!("The same as {name}, found in {tool}."),
            Skip::Unsupported(why) => why.clone(),
        }
    }
}

/// What a found app is: an address shown without its query or user-info, or the exact
/// command Ghost would run (each argument as given, never through a shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Address(String),
    Command(Vec<String>),
}

/// An entry as read, before Ghost names it.
pub struct Raw {
    pub tool: Tool,
    pub name: String,
    pub key: String,
    pub scope: String,
    pub file: String,
    pub url: Option<String>,
    pub headers: IndexMap<String, String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: IndexMap<String, String>,
    pub sign_in: bool,
    pub notes: Vec<String>,
    pub skip: Option<Skip>,
}

impl Raw {
    fn new(tool: Tool, name: &str, scope: &str, file: &FileSeen) -> Self {
        Raw {
            tool,
            name: name.chars().take(100).collect(),
            key: format!("{}#{scope}#{name}", file.shown),
            scope: scope.to_string(),
            file: file.shown.clone(),
            url: None,
            headers: IndexMap::new(),
            command: None,
            args: Vec::new(),
            env: IndexMap::new(),
            sign_in: false,
            notes: Vec::new(),
            skip: None,
        }
    }

    fn unsupported(&mut self, why: String) {
        if self.skip.is_none() {
            self.skip = Some(Skip::Unsupported(why));
        }
    }
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn table_of(v: Option<&Value>) -> IndexMap<String, String> {
    let Some(Value::Object(m)) = v else {
        return IndexMap::new();
    };
    m.iter()
        .filter_map(|(k, v)| text_of(v).map(|s| (k.clone(), s)))
        .collect()
}

/// The entries at `path` of a tool's JSON file, each in one shape. A file that is not
/// JSON, even with its comments taken out, is a line saying so and nothing else.
pub fn json_entries(
    tool: Tool,
    text: &str,
    path: &[&str],
    scope: &str,
    file: &mut FileSeen,
    raws: &mut Vec<Raw>,
) {
    let entries = match json_entries_at(text, path) {
        Ok(e) => e,
        Err(_) => match super::read::strip_jsonc(text).map(|t| json_entries_at(&t, path)) {
            Some(Ok(e)) => {
                file.commented = true;
                e
            }
            _ => {
                file.state = FileState::Unreadable(
                    "could not be read as JSON, so Ghost left it as it is".into(),
                );
                return;
            }
        },
    };
    let Some(mut entries) = entries else {
        return;
    };
    if entries.len() > ENTRIES_MAX {
        file.problems.push(format!(
            "{} lists {} apps; Ghost shows the first {ENTRIES_MAX}.",
            file.shown,
            entries.len()
        ));
        entries.truncate(ENTRIES_MAX);
    }
    for (name, v) in entries {
        let mut raw = Raw::new(tool, &name, scope, file);
        let Value::Object(m) = &v else {
            raw.unsupported("Not an entry Ghost understands.".into());
            raws.push(raw);
            continue;
        };
        let kind = m
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        match kind.as_str() {
            "sse" => raw.unsupported(
                "Connects over SSE, an older kind of connection Ghost can't open.".into(),
            ),
            "ws" => raw.unsupported("Connects over WebSocket, which Ghost can't open.".into()),
            "" | "stdio" | "http" | "streamable-http" | "streamablehttp" | "streamable_http" => {}
            other => raw.unsupported(format!(
                "Is a kind of app ({}) Ghost can't connect.",
                bounded(other)
            )),
        }
        raw.url = m.get("url").and_then(Value::as_str).map(String::from);
        raw.command = m.get("command").and_then(Value::as_str).map(String::from);
        raw.args = match m.get("args") {
            Some(Value::Array(a)) => a.iter().filter_map(text_of).collect(),
            _ => Vec::new(),
        };
        raw.env = table_of(m.get("env"));
        raw.headers = table_of(m.get("headers"));
        if m.contains_key("headersHelper") {
            raw.unsupported(
                "Gets its headers by running a script, which Ghost does not do.".into(),
            );
        }
        if m.get("disabled").and_then(Value::as_bool) == Some(true) {
            raw.unsupported(format!("Turned off in {}.", tool.title()));
        }
        if m.contains_key("oauth") || m.contains_key("auth") {
            raw.sign_in = true;
        }
        if let Some(f) = m.get("envFile").and_then(Value::as_str) {
            raw.notes.push(format!(
                "{} also reads settings from {}, which Ghost does not.",
                tool.title(),
                bounded(f)
            ));
        }
        raws.push(raw);
    }
}

/// Codex's `[mcp_servers.<name>]` tables, each in one shape.
pub fn codex_entries(text: &str, file: &mut FileSeen, raws: &mut Vec<Raw>) {
    let doc: toml::Value = match toml::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            file.state = FileState::Unreadable(
                "could not be read as TOML, so Ghost left it as it is".into(),
            );
            return;
        }
    };
    let Some(toml::Value::Table(servers)) = doc.get("mcp_servers") else {
        return;
    };
    if servers.len() > ENTRIES_MAX {
        file.problems.push(format!(
            "{} lists {} apps; Ghost shows the first {ENTRIES_MAX}.",
            file.shown,
            servers.len()
        ));
    }
    let str_of = |v: &toml::Value| -> Option<String> {
        match v {
            toml::Value::String(s) => Some(s.clone()),
            toml::Value::Integer(i) => Some(i.to_string()),
            toml::Value::Float(f) => Some(f.to_string()),
            toml::Value::Boolean(b) => Some(b.to_string()),
            _ => None,
        }
    };
    let table = |v: Option<&toml::Value>| -> IndexMap<String, String> {
        match v {
            Some(toml::Value::Table(t)) => t
                .iter()
                .filter_map(|(k, v)| str_of(v).map(|s| (k.clone(), s)))
                .collect(),
            _ => IndexMap::new(),
        }
    };
    for (name, v) in servers.iter().take(ENTRIES_MAX) {
        let mut raw = Raw::new(Tool::Codex, name, "every project", file);
        let toml::Value::Table(t) = v else {
            raw.unsupported("Not an entry Ghost understands.".into());
            raws.push(raw);
            continue;
        };
        if t.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
            raw.unsupported("Turned off in Codex.".into());
        }
        raw.command = t.get("command").and_then(|v| v.as_str()).map(String::from);
        raw.url = t.get("url").and_then(|v| v.as_str()).map(String::from);
        raw.args = match t.get("args") {
            Some(toml::Value::Array(a)) => a.iter().filter_map(str_of).collect(),
            _ => Vec::new(),
        };
        raw.env = table(t.get("env"));
        if let Some(toml::Value::Array(vars)) = t.get("env_vars") {
            for v in vars {
                let name = match v {
                    toml::Value::String(s) => Some(s.clone()),
                    toml::Value::Table(t) => {
                        t.get("name").and_then(|n| n.as_str()).map(String::from)
                    }
                    _ => None,
                };
                if let Some(n) = name.filter(|n| ENV_NAME.is_match(n)) {
                    raw.env.insert(n.clone(), format!("${{{n}}}"));
                }
            }
        }
        if let Some(cwd) = t.get("cwd").and_then(|v| v.as_str()) {
            raw.notes.push(format!(
                "Codex starts it in {}; Ghost starts it in its own folder.",
                bounded(cwd)
            ));
        }
        raw.headers = table(t.get("http_headers"));
        for (h, var) in table(t.get("env_http_headers")) {
            if ENV_NAME.is_match(&var) {
                raw.headers.insert(h, format!("${{{var}}}"));
            }
        }
        if let Some(var) = t
            .get("bearer_token_env_var")
            .and_then(|v| v.as_str())
            .filter(|v| ENV_NAME.is_match(v))
        {
            raw.headers
                .insert("Authorization".into(), format!("Bearer ${{{var}}}"));
        }
        // An address with no token of its own signs in, as Codex does by default.
        if raw.url.is_some() && !raw.headers.keys().any(|k| is_key_header(k)) {
            raw.sign_in = true;
        }
        raws.push(raw);
    }
}

static ENV_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$").unwrap());
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{([^}]{0,200})\}").unwrap());
static KEY_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(authorization|key|token|secret|auth|password|cookie|session)").unwrap()
});
static KEY_ENV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(key|token|secret|password|passwd|credential|auth|(^|_)pat$)").unwrap()
});

fn is_key_header(name: &str) -> bool {
    KEY_HEADER.is_match(name)
}

/// The other tool's variables in a value, as Ghost reads them: `${VAR:-default}` is
/// `${VAR}`, `${userHome}` the home folder, `${/}` a slash. A variable only that tool
/// knows (the project's folder, a prompt, a plugin's root) is a reason to skip.
fn translate(
    raw_tool: Tool,
    s: &str,
    w: &Where,
    notes: &mut Vec<String>,
) -> Result<String, String> {
    let mut problem = None;
    let out = VARIABLE.replace_all(s, |c: &regex::Captures| {
        let inner = &c[1];
        if let Some(name) = inner.strip_prefix("env:")
            && ENV_NAME.is_match(name)
        {
            return format!("${{{name}}}");
        }
        if let Some((name, _default)) = inner.split_once(":-")
            && ENV_NAME.is_match(name)
        {
            let note = format!(
                "Ghost does not use the default {} gives {name}.",
                raw_tool.title()
            );
            if !notes.contains(&note) {
                notes.push(note);
            }
            return format!("${{{name}}}");
        }
        match inner {
            "userHome" => return w.home.display().to_string(),
            "/" | "pathSeparator" => return "/".into(),
            _ => {}
        }
        if ENV_NAME.is_match(inner)
            && inner
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !inner.starts_with("CLAUDE_PLUGIN")
        {
            return format!("${{{inner}}}");
        }
        problem.get_or_insert_with(|| {
            format!(
                "Refers to {}, which only {} knows.",
                bounded(&format!("${{{inner}}}")),
                raw_tool.title()
            )
        });
        c[0].to_string()
    });
    match problem {
        Some(p) => Err(p),
        None => Ok(out.into_owned()),
    }
}

/// A name `apps.json` takes, from the other tool's: `GitHub` is `github`, `my.server`
/// is `my-server`.
pub fn app_name_from(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out: String = out
        .trim_matches(|c| c == '-' || c == '_')
        .chars()
        .take(40)
        .collect();
    let out = out.trim_end_matches(['-', '_']).to_string();
    if out.is_empty() { "app".into() } else { out }
}

/// What makes two entries the same app: the address without its query, user-info and
/// trailing slash, or the command and its arguments with every `${NAME}` alike.
pub fn identity(spec: &AppSpec) -> String {
    static REF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\{[^}]*\}|\$[A-Z][A-Z0-9_]*").unwrap());
    if let Some(u) = &spec.url {
        let filled = REF.replace_all(u, "x");
        if let Ok(p) = url::Url::parse(&filled) {
            return format!(
                "url:{}://{}:{}{}",
                p.scheme(),
                p.host_str().unwrap_or_default().to_lowercase(),
                p.port_or_known_default().unwrap_or(0),
                p.path().trim_end_matches('/')
            );
        }
        return format!("url:{}", filled.to_lowercase());
    }
    let mut parts = vec![spec.command.clone().unwrap_or_default()];
    parts.extend(
        spec.args
            .iter()
            .map(|a| REF.replace_all(a, "*").into_owned()),
    );
    format!("cmd:{}", parts.join("\u{0}"))
}

/// The address as shown: no user-info, no query, no fragment.
pub fn shown_url(u: &str) -> String {
    if let Ok(mut p) = url::Url::parse(u) {
        let _ = p.set_username("");
        let _ = p.set_password(None);
        p.set_query(None);
        p.set_fragment(None);
        return bounded(p.as_str());
    }
    let head = u.split(['?', '#']).next().unwrap_or("");
    let head = match head.split_once("://") {
        Some((scheme, rest)) => {
            let (auth, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
            let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
            if path.is_empty() {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}/{path}")
            }
        }
        None => head.to_string(),
    };
    bounded(&head)
}

/// Ghost itself in another tool's file: its listener's address, a token of Ghost's, or
/// `ghostd mcp serve`. Returns the client it names, when it names one.
pub fn points_at_ghost(raw: &Raw, ports: &[u16]) -> Option<Option<String>> {
    let from_token = raw.headers.values().find_map(|v| {
        let t = v.trim().strip_prefix("Bearer ")?.trim();
        let rest = t.strip_prefix("ghost.")?;
        Some(rest.split('.').next().unwrap_or_default().to_string())
    });
    if let Some(client) = from_token {
        return Some(Some(client).filter(|c| !c.is_empty()));
    }
    if let Some(u) = &raw.url
        && let Ok(p) = url::Url::parse(u)
        && matches!(
            p.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
        && p.port().is_some_and(|port| ports.contains(&port))
        && p.path().starts_with("/mcp")
    {
        return Some(None);
    }
    if let Some(cmd) = &raw.command {
        let base = std::path::Path::new(cmd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let serves = raw
            .args
            .windows(2)
            .any(|w| w[0] == "mcp" && w[1] == "serve")
            && raw.args.iter().any(|a| a == "--client");
        if base == "ghostd" || serves {
            let client = raw
                .args
                .windows(2)
                .find(|w| w[0] == "--client")
                .map(|w| w[1].clone());
            return Some(client);
        }
    }
    None
}

pub enum Planning {
    Ghost(Linked),
    App(Box<FoundApp>),
}

/// One entry as Ghost would connect it, or Ghost itself.
pub fn plan(
    w: &Where,
    ghost: &Ghost,
    mut raw: Raw,
    taken: &mut Vec<String>,
    seen: &mut IndexMap<String, String>,
) -> Planning {
    if let Some(client) = points_at_ghost(&raw, &ghost.ports) {
        return Planning::Ghost(Linked {
            entry: raw.name.clone(),
            client,
            file: raw.file.clone(),
        });
    }
    if raw.name.trim().eq_ignore_ascii_case(crate::apps::GHOST_APP) && raw.skip.is_none() {
        raw.skip = Some(Skip::Reserved);
    }
    if raw.url.is_some() == raw.command.is_some() && raw.skip.is_none() {
        raw.skip = Some(Skip::Unsupported(
            "Names neither one address nor one command.".into(),
        ));
    }
    // The other tool's variables, in every value.
    let tool = raw.tool;
    let mut notes = std::mem::take(&mut raw.notes);
    let mut problem: Option<String> = None;
    let mut tr = |s: &str, notes: &mut Vec<String>| -> String {
        match translate(tool, s, w, notes) {
            Ok(t) => t,
            Err(p) => {
                problem.get_or_insert(p);
                s.to_string()
            }
        }
    };
    let url = raw.url.as_deref().map(|u| tr(u, &mut notes));
    let command = raw.command.as_deref().map(|c| tr(c, &mut notes));
    let args: Vec<String> = raw.args.iter().map(|a| tr(a, &mut notes)).collect();
    let headers: IndexMap<String, String> = raw
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), tr(v, &mut notes)))
        .collect();
    let env: IndexMap<String, String> = raw
        .env
        .iter()
        .map(|(k, v)| (k.clone(), tr(v, &mut notes)))
        .collect();
    if let Some(p) = problem
        && raw.skip.is_none()
    {
        raw.skip = Some(Skip::Unsupported(p));
    }
    let base = app_name_from(&raw.name);
    let suggested = free_name(taken, &base);
    let sign_in = url.is_some() && (raw.sign_in || !headers.keys().any(|k| is_key_header(k)));
    let spec = AppSpec {
        name: suggested.clone(),
        url,
        headers,
        command,
        args,
        env,
        sign_in,
        ..AppSpec::default()
    };
    let (spec, secrets, moved) = move_secrets(&spec);
    let shown = match (&spec.url, &spec.command) {
        (Some(u), _) => Shown::Address(shown_url(u)),
        (None, c) => Shown::Command(
            std::iter::once(c.clone().unwrap_or_default())
                .chain(spec.args.iter().cloned())
                .take(65)
                .map(|a| bounded(&a))
                .collect(),
        ),
    };
    let needs: Vec<String> = spec
        .secrets()
        .into_iter()
        .filter(|n| !secrets.contains_key(n) && !ghost.passwords.contains(n))
        .collect();
    if raw.skip.is_none() {
        let id = identity(&spec);
        if let Some(other) = seen.get(&id) {
            raw.skip = Some(match other.split_once('\u{0}') {
                Some((tool, name)) => Skip::Same {
                    tool: tool.to_string(),
                    name: name.to_string(),
                },
                None => Skip::Connected(other.clone()),
            });
        } else {
            seen.insert(id, format!("{}\u{0}{}", tool.title(), bounded(&raw.name)));
            taken.push(suggested.clone());
        }
    }
    Planning::App(Box::new(FoundApp {
        tool,
        key: raw.key,
        name: bounded(&raw.name),
        scope: raw.scope,
        file: raw.file,
        suggested,
        shown,
        sign_in: spec.sign_in,
        moved,
        needs,
        notes: notes.iter().map(|n| bounded(n)).collect(),
        skip: raw.skip,
        plan: Some(Planned { spec, secrets }),
    }))
}

type Moved = (AppSpec, IndexMap<String, Zeroizing<String>>, Vec<String>);

/// Every secret written into the entry, moved out: the ones a pasted config's are (by
/// shape, in an argument, in the address's query), then every literal header named for a
/// key, every literal setting named for one, and the address's user-info. Returns the
/// entry with `${NAME}` in their place, the values by name, and where each was.
pub fn move_secrets(spec: &AppSpec) -> Moved {
    let (mut out, found) = extract_secrets(spec);
    let mut secrets: IndexMap<String, Zeroizing<String>> = found
        .into_iter()
        .map(|(k, v)| (k, Zeroizing::new(v)))
        .collect();
    let mut moved: Vec<String> = Vec::new();
    let keep = |label: &str, value: &str, secrets: &mut IndexMap<String, Zeroizing<String>>| {
        let base = secret_name(&spec.name, label);
        let mut name = base.clone();
        let mut n = 2;
        while secrets.contains_key(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        secrets.insert(name.clone(), Zeroizing::new(value.to_string()));
        format!("${{{name}}}")
    };
    for (k, v) in out.headers.iter_mut() {
        if spec.headers.get(k) != Some(v) {
            moved.push(format!("header {k}"));
        } else if is_key_header(k) && secret_names(v).is_empty() && !v.trim().is_empty() {
            *v = keep(k, v, &mut secrets);
            moved.push(format!("header {k}"));
        }
    }
    for (k, v) in out.env.iter_mut() {
        if spec.env.get(k) != Some(v) {
            moved.push(format!("setting {k}"));
        } else if KEY_ENV.is_match(k)
            && secret_names(v).is_empty()
            && !v.trim().is_empty()
            && !v.starts_with('/')
        {
            *v = keep(k, v, &mut secrets);
            moved.push(format!("setting {k}"));
        }
    }
    for (i, a) in out.args.iter().enumerate() {
        if spec.args.get(i) != Some(a) {
            moved.push(format!("argument {}", i + 1));
        }
    }
    if let Some(u) = out.url.clone() {
        if spec.url.as_ref() != Some(&u) {
            moved.push("the address's query".into());
        }
        // user:password@ in the address.
        if let Some((scheme, rest)) = u.split_once("://") {
            let auth_end = rest.find('/').unwrap_or(rest.len());
            let auth = &rest[..auth_end];
            if let Some((info, host)) = auth.rsplit_once('@')
                && !info.is_empty()
                && secret_names(info).is_empty()
            {
                let r = keep("userinfo", info, &mut secrets);
                out.url = Some(format!("{scheme}://{r}@{host}{}", &rest[auth_end..]));
                moved.push("the address's user name and password".into());
            }
        }
    }
    (out, secrets, moved)
}
