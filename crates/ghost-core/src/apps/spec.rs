//! The file: `apps.json`, or another client's `mcpServers` file, read with its problems
//! named, and written for the operator only (0600, atomic).

use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::{AppError, Environ};

pub const APPS_FILE: &str = "apps.json";
/// `$NAME`, `${NAME}` or `${env:NAME}` (the shape other clients' config files use).
static SECRET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{(?:env:)?([A-Z][A-Z0-9_]*)\}|\$([A-Z][A-Z0-9_]*)").unwrap());
pub static APP_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,39}$").unwrap());
/// The names of Ghost's own tool sources (`crate::source::ToolSource`): the gate, the
/// permissions (`skills.*`) and the audit key on them, so an app may not take one.
pub const RESERVED_NAMES: [&str; 5] = ["conversations", "outputs", "rules", "skills", "waiting"];
const TRANSPORTS: [&str; 5] = [
    "stdio",
    "http",
    "streamable-http",
    "streamable_http",
    "streamableHttp",
];
const APP_KEYS: [&str; 9] = [
    "type", "command", "args", "env", "url", "headers", "title", "catalog", "sign_in",
];
/// A literal that is probably a token: a known prefix, or a bearer value with no reference.
static SECRET_SHAPES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(ghp_|github_pat_|gho_|sk-|xox[abp]-|AKIA|glpat-|Bearer\s+\S{16,})").unwrap()
});
/// An argument or a query parameter named for a key: `--api-key`, `--token`, `key=`.
const KEY_WORD: &str =
    r"(?:[a-z0-9]+[-_])*(?:api[-_]?key|apikey|key|token|secret|password|pat|auth)";
static KEY_FLAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i)^--?{KEY_WORD}$")).unwrap());
static KEY_PARAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i)^{KEY_WORD}$")).unwrap());

// --- JSON in the file's own order -----------------------------------------------------

/// JSON with its objects' keys in the order the file has them: the first app in
/// `apps.json` keeps a tool name two apps would offer, and an edit keeps the file as the
/// operator wrote it.
#[derive(Debug, Clone, PartialEq)]
enum Ordered {
    Object(IndexMap<String, Ordered>),
    Other(Value),
}

impl Ordered {
    fn into_value(self) -> Value {
        match self {
            Ordered::Object(m) => {
                Value::Object(m.into_iter().map(|(k, v)| (k, v.into_value())).collect())
            }
            Ordered::Other(v) => v,
        }
    }

    fn from_value(v: Value) -> Ordered {
        match v {
            Value::Object(m) => Ordered::Object(
                m.into_iter()
                    .map(|(k, v)| (k, Ordered::from_value(v)))
                    .collect(),
            ),
            other => Ordered::Other(other),
        }
    }
}

impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Ordered::Object(m) => {
                let mut map = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
            Ordered::Other(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Ordered, A::Error> {
                let mut m = IndexMap::new();
                while let Some((k, v)) = access.next_entry::<String, Ordered>()? {
                    m.insert(k, v);
                }
                Ok(Ordered::Object(m))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Ordered, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = access.next_element::<Ordered>()? {
                    out.push(v.into_value());
                }
                Ok(Ordered::Other(Value::Array(out)))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::from(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::from(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::from(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::String(v.to_string())))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::String(v)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::Null))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Ordered, E> {
                Ok(Ordered::Other(Value::Null))
            }
        }
        d.deserialize_any(V)
    }
}

fn pretty(v: &Ordered) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
}

// --- the file -------------------------------------------------------------------------

/// One app as `apps.json` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSpec {
    pub name: String,
    pub title: String,
    pub url: Option<String>,
    pub headers: IndexMap<String, String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: IndexMap<String, String>,
    /// The catalog entry it was connected from.
    pub catalog: Option<String>,
    /// Ghost signs in to it (OAuth) when it asks.
    pub sign_in: bool,
}

impl AppSpec {
    /// An app Ghost signs in to or sends a key to.
    pub fn at(name: &str, url: &str) -> Self {
        Self {
            name: name.into(),
            url: Some(url.into()),
            ..Self::default()
        }
    }

    /// An app that runs on this computer.
    pub fn runs(name: &str, command: &str) -> Self {
        Self {
            name: name.into(),
            command: Some(command.into()),
            ..Self::default()
        }
    }

    pub fn transport(&self) -> &'static str {
        if self.url.is_some() { "http" } else { "stdio" }
    }

    /// How the operator knows it: the title, or the name as words.
    pub fn shown(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        py_title(&self.name.replace(['-', '_'], " "))
    }

    /// The environment names this app needs.
    pub fn secrets(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let values = std::iter::once(self.url.clone().unwrap_or_default())
            .chain(self.headers.values().cloned())
            .chain(self.env.values().cloned())
            .chain(self.args.iter().cloned());
        for value in values {
            for name in secret_names(&value) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }
}

impl Default for AppSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            title: String::new(),
            url: None,
            headers: IndexMap::new(),
            command: None,
            args: Vec::new(),
            env: IndexMap::new(),
            catalog: None,
            sign_in: false,
        }
    }
}

/// Python's `str.title()`: the first letter of every word up, the rest down.
fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppsFile {
    /// In the file's order: the first to name a tool keeps the name.
    pub apps: Vec<AppSpec>,
    pub problems: Vec<String>,
    /// The keys the file used: `mcpServers` from another client is read, not written.
    pub imported: bool,
    /// Entries that did not read, named in `problems`.
    pub left_out: Vec<String>,
}

impl AppsFile {
    pub fn get(&self, name: &str) -> Option<&AppSpec> {
        self.apps.iter().find(|a| a.name == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.apps.iter().map(|a| a.name.as_str()).collect()
    }
}

/// Why `name` can't name an app, or None.
pub fn app_name_problem(name: &str) -> Option<String> {
    if !APP_NAME_RE.is_match(name) {
        return Some("an app's name is lower-case letters, digits, _ or - (up to 40)".into());
    }
    if RESERVED_NAMES.contains(&name) {
        return Some(format!(
            "{name} is the name of Ghost's own tools; give the app another name"
        ));
    }
    None
}

/// The environment names a value refers to.
pub fn secret_names(value: &str) -> Vec<String> {
    SECRET
        .captures_iter(value)
        .map(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// `$NAME`, `${NAME}` or `${env:NAME}` from the environment. A missing name fails so a
/// misconfigured app fails loudly at connect time, not silently at first use.
pub fn fill_secrets(value: &str, environ: &dyn Environ) -> Result<String, AppError> {
    let mut out = String::with_capacity(value.len());
    let mut last = 0;
    for c in SECRET.captures_iter(value) {
        let m = c.get(0).unwrap();
        let name = c
            .get(1)
            .or_else(|| c.get(2))
            .map(|m| m.as_str())
            .unwrap_or_default();
        let Some(filled) = environ.get(name) else {
            return Err(AppError::MissingSecret(name.to_string()));
        };
        out.push_str(&value[last..m.start()]);
        out.push_str(&filled);
        last = m.end();
    }
    out.push_str(&value[last..]);
    Ok(out)
}

/// A literal where a `${NAME}` reference belongs.
pub fn looks_like_secret(value: &str) -> bool {
    if !secret_names(value).is_empty() {
        return false;
    }
    let trimmed = value.trim();
    SECRET_SHAPES.is_match(trimmed)
        || (value.chars().count() >= 32
            && !trimmed.contains(' ')
            && !value.starts_with("http")
            && !value.starts_with('/'))
}

fn str_list(value: Option<&Ordered>, where_: &str) -> Result<Vec<String>, AppError> {
    let Some(Ordered::Other(Value::Array(items))) = value else {
        if value.is_none() {
            return Ok(Vec::new());
        }
        return Err(AppError::Value(format!(
            "{where_} must be a list of strings"
        )));
    };
    items
        .iter()
        .map(|v| match v {
            Value::String(s) => Ok(s.clone()),
            _ => Err(AppError::Value(format!(
                "{where_} must be a list of strings"
            ))),
        })
        .collect()
}

fn str_table(value: Option<&Ordered>, where_: &str) -> Result<IndexMap<String, String>, AppError> {
    let Some(Ordered::Object(m)) = value else {
        if value.is_none() {
            return Ok(IndexMap::new());
        }
        return Err(AppError::Value(format!(
            "{where_} must be an object of strings"
        )));
    };
    m.iter()
        .map(|(k, v)| match v {
            Ordered::Other(Value::String(s)) => Ok((k.clone(), s.clone())),
            _ => Err(AppError::Value(format!(
                "{where_} must be an object of strings"
            ))),
        })
        .collect()
}

fn read_json(text: &str, where_: &str) -> Result<Ordered, AppError> {
    if text.trim().is_empty() {
        return Ok(Ordered::Object(IndexMap::new()));
    }
    serde_json::from_str::<Ordered>(text)
        .map_err(|e| AppError::Value(format!("{where_}: not JSON: {e}")))
}

/// `apps.json`, or another client's `mcpServers` file. An app with a problem is left out
/// and the problem named; the rest of the file still counts.
pub fn parse_apps_json(text: &str, where_: &str) -> Result<AppsFile, AppError> {
    let raw = read_json(text, where_)?;
    let Ordered::Object(raw) = raw else {
        return Err(AppError::Value(format!(
            "{where_}: the file must be an object"
        )));
    };
    let imported = !raw.contains_key("apps") && raw.contains_key("mcpServers");
    let empty = Ordered::Object(IndexMap::new());
    let entries = raw
        .get("apps")
        .or_else(|| raw.get("mcpServers"))
        .unwrap_or(&empty);
    let Ordered::Object(entries) = entries else {
        return Err(AppError::Value(format!(
            "{where_}: apps must be an object of apps by name"
        )));
    };
    let mut problems = Vec::new();
    let mut apps = Vec::new();
    let mut left_out = Vec::new();
    for (name, body) in entries {
        let at = format!("{where_}: {name}");
        if let Some(why) = app_name_problem(name) {
            problems.push(format!("{at}: {why}"));
            left_out.push(name.clone());
            continue;
        }
        match parse_entry(name, body, &at, &mut problems) {
            Ok(spec) => apps.push(spec),
            Err(e) => {
                problems.push(e.to_string());
                left_out.push(name.clone());
            }
        }
    }
    let mut extra: Vec<&str> = raw
        .keys()
        .map(String::as_str)
        .filter(|k| *k != "apps" && *k != "mcpServers")
        .collect();
    extra.sort_unstable();
    if !extra.is_empty() {
        problems.push(format!(
            "{where_}: unknown key(s) {} are ignored",
            extra.join(", ")
        ));
    }
    Ok(AppsFile {
        apps,
        problems,
        imported,
        left_out,
    })
}

fn parse_entry(
    name: &str,
    body: &Ordered,
    where_: &str,
    problems: &mut Vec<String>,
) -> Result<AppSpec, AppError> {
    let Ordered::Object(t) = body else {
        return Err(AppError::Value(format!("{where_} must be an object")));
    };
    let mut unknown: Vec<&str> = t
        .keys()
        .map(String::as_str)
        .filter(|k| !APP_KEYS.contains(k))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        problems.push(format!(
            "{where_}: key(s) {} are not read by Ghost",
            unknown.join(", ")
        ));
    }
    let text_of = |key: &str| -> Option<String> {
        match t.get(key) {
            Some(Ordered::Other(v)) if truthy(v) => Some(crate::models::py_str(v)),
            _ => None,
        }
    };
    if let Some(Ordered::Other(kind)) = t.get("type")
        && !kind.is_null()
    {
        let k = crate::models::py_str(kind);
        if !TRANSPORTS.contains(&k.as_str()) {
            return Err(AppError::Value(format!(
                "{where_}: {} is not a kind of app Ghost can connect",
                crate::models::py_repr(&k)
            )));
        }
    }
    let url = text_of("url");
    let command = text_of("command");
    if url.is_some() == command.is_some() {
        return Err(AppError::Value(format!(
            "{where_} needs exactly one of url or command"
        )));
    }
    let headers = str_table(t.get("headers"), &format!("{where_}.headers"))?;
    let env = str_table(t.get("env"), &format!("{where_}.env"))?;
    let args = str_list(t.get("args"), &format!("{where_}.args"))?;
    let mut found: Vec<String> = Vec::new();
    for (label, table) in [("headers", &headers), ("env", &env)] {
        for (key, value) in table {
            if looks_like_secret(value) {
                found.push(format!("{label} {key}"));
            }
        }
    }
    found.extend(
        keys_in_args(&args)
            .into_iter()
            .map(|k| format!("argument {}", k.label)),
    );
    found.extend(
        keys_in_url(url.as_deref().unwrap_or(""))
            .into_iter()
            .map(|(param, _)| format!("the address's {param}")),
    );
    for what in found {
        problems.push(format!(
            "{where_}: {what} looks like a key written into the file; use ${{NAME}} and set \
             it in Ghost's environment"
        ));
    }
    let sign_in = matches!(t.get("sign_in"), Some(Ordered::Other(v)) if truthy(v)) && url.is_some();
    Ok(AppSpec {
        name: name.to_string(),
        title: text_of("title").unwrap_or_default(),
        url,
        headers,
        command,
        args,
        env,
        catalog: text_of("catalog"),
        sign_in,
    })
}

/// One entry of the file, from a body the desktop or a test hands in. Problems that
/// don't stop the entry (a key written into it) are named in `problems`.
pub fn parse_app(
    name: &str,
    body: &Value,
    where_: &str,
    problems: &mut Vec<String>,
) -> Result<AppSpec, AppError> {
    parse_entry(name, &Ordered::from_value(body.clone()), where_, problems)
}

/// Python's truthiness of a JSON value.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn render_entry(spec: &AppSpec) -> Ordered {
    let table = |m: &IndexMap<String, String>| {
        Ordered::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), Ordered::Other(Value::String(v.clone()))))
                .collect(),
        )
    };
    let mut entry: IndexMap<String, Ordered> = IndexMap::new();
    if !spec.title.is_empty() {
        entry.insert(
            "title".into(),
            Ordered::Other(Value::String(spec.title.clone())),
        );
    }
    if let Some(url) = &spec.url {
        entry.insert("url".into(), Ordered::Other(Value::String(url.clone())));
        if !spec.headers.is_empty() {
            entry.insert("headers".into(), table(&spec.headers));
        }
        if spec.sign_in {
            entry.insert("sign_in".into(), Ordered::Other(Value::Bool(true)));
        }
    } else {
        entry.insert(
            "command".into(),
            Ordered::Other(
                spec.command
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            ),
        );
        if !spec.args.is_empty() {
            entry.insert(
                "args".into(),
                Ordered::Other(Value::Array(
                    spec.args.iter().map(|a| Value::String(a.clone())).collect(),
                )),
            );
        }
        if !spec.env.is_empty() {
            entry.insert("env".into(), table(&spec.env));
        }
    }
    if let Some(catalog) = &spec.catalog {
        entry.insert(
            "catalog".into(),
            Ordered::Other(Value::String(catalog.clone())),
        );
    }
    Ordered::Object(entry)
}

/// The entry as the file has it.
pub fn render_app(spec: &AppSpec) -> Value {
    render_entry(spec).into_value()
}

pub fn render_apps_json(apps: &[AppSpec]) -> String {
    let entries: IndexMap<String, Ordered> = apps
        .iter()
        .map(|s| (s.name.clone(), render_entry(s)))
        .collect();
    let mut top = IndexMap::new();
    top.insert("apps".to_string(), Ordered::Object(entries));
    pretty(&Ordered::Object(top))
}

/// The file, or an empty one when it isn't there. A file that will not read is one
/// problem and no apps.
pub fn load_apps(path: &Path) -> AppsFile {
    if !path.is_file() {
        return AppsFile::default();
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return AppsFile {
                problems: vec![e.to_string()],
                ..Default::default()
            };
        }
    };
    match parse_apps_json(&text, &path.display().to_string()) {
        Ok(file) => file,
        Err(e) => AppsFile {
            problems: vec![e.to_string()],
            ..Default::default()
        },
    }
}

/// The whole file from `apps` (a new file, a test). Connecting and disconnecting use
/// `edit_apps`, which keeps what Ghost can't read.
pub fn write_apps(path: &Path, apps: &[AppSpec]) -> Result<(), AppError> {
    write_private(path, &render_apps_json(apps))
}

/// The operator's action (connect, disconnect in the desktop); never the model's. Only
/// the entries named change: every other entry, one Ghost can't read included, and any
/// other key in the file stay as they are. A file that is not a JSON object of apps is
/// never overwritten: the error says why, and the operator fixes it by hand.
pub fn edit_apps(path: &Path, put: &[AppSpec], remove: Option<&str>) -> Result<(), AppError> {
    let text = if path.is_file() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let raw = if text.trim().is_empty() {
        Ordered::Object(IndexMap::new())
    } else {
        serde_json::from_str::<Ordered>(&text).map_err(|e| {
            AppError::Value(format!(
                "{} is not JSON ({e}); fix it by hand first",
                path.display()
            ))
        })?
    };
    let Ordered::Object(mut raw) = raw else {
        return Err(AppError::Value(format!(
            "{} is not an object of apps; fix it by hand first",
            path.display()
        )));
    };
    let key = if raw.contains_key("apps") || !raw.contains_key("mcpServers") {
        "apps"
    } else {
        "mcpServers"
    };
    let entries = raw
        .entry(key.to_string())
        .or_insert_with(|| Ordered::Object(IndexMap::new()));
    let Ordered::Object(entries) = entries else {
        return Err(AppError::Value(format!(
            "{}: {key} is not an object of apps; fix it by hand first",
            path.display()
        )));
    };
    if let Some(name) = remove {
        entries.shift_remove(name);
    }
    for spec in put {
        entries.insert(spec.name.clone(), render_entry(spec));
    }
    write_private(path, &pretty(&Ordered::Object(raw)))
}

/// Atomic, and 0600: an entry may hold a key the operator pasted before Ghost moved it.
fn write_private(path: &Path, text: &str) -> Result<(), AppError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// --- keys written into a paste --------------------------------------------------------

/// The environment name a secret of an app is kept under: `GHOST_APP_GITHUB_TOKEN`.
pub fn secret_name(app: &str, key: &str) -> String {
    static NOT_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^A-Z0-9]+").unwrap());
    let upper = format!("{app}_{key}").to_uppercase();
    let cleaned = NOT_ALNUM.replace_all(&upper, "_");
    format!("GHOST_APP_{}", cleaned.trim_matches('_'))
}

/// A pasted config with keys written into it: each literal that looks like a secret, in
/// a header, the environment, an argument (`--api-key X`, `--token=X`) or the address's
/// query (`?api_key=X`), moves into Ghost's environment under its own name, and the entry
/// keeps a reference.
pub fn extract_secrets(spec: &AppSpec) -> (AppSpec, IndexMap<String, String>) {
    let mut secrets: IndexMap<String, String> = IndexMap::new();
    let mut keep = |key: &str, value: &str| -> String {
        let base = secret_name(&spec.name, key);
        let mut name = base.clone();
        let mut n = 2;
        while secrets.contains_key(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        secrets.insert(name.clone(), value.to_string());
        format!("${{{name}}}")
    };
    let headers: IndexMap<String, String> = spec
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                if looks_like_secret(v) {
                    keep(k, v)
                } else {
                    v.clone()
                },
            )
        })
        .collect();
    let env: IndexMap<String, String> = spec
        .env
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                if looks_like_secret(v) {
                    keep(k, v)
                } else {
                    v.clone()
                },
            )
        })
        .collect();
    let mut args = spec.args.clone();
    for key in keys_in_args(&spec.args) {
        args[key.index] = format!("{}{}", key.lead, keep(&key.label, &key.value));
    }
    let mut url = spec.url.clone();
    if let Some(u) = &spec.url
        && !keys_in_url(u).is_empty()
    {
        let (head, rest) = u.split_once('?').unwrap_or((u, ""));
        let (query, fragment) = match rest.split_once('#') {
            Some((q, f)) => (q, Some(f)),
            None => (rest, None),
        };
        let mut parts = Vec::new();
        for part in query.split('&') {
            let found = keys_in_url(&format!("?{part}"));
            match part.split_once('=') {
                Some((param, value)) if found.iter().any(|(p, v)| p == param && v == value) => {
                    // As written, still percent-encoded: filled back in, it is the address.
                    parts.push(format!("{param}={}", keep(param, value)));
                }
                _ => parts.push(part.to_string()),
            }
        }
        let tail = fragment.map(|f| format!("#{f}")).unwrap_or_default();
        url = Some(format!("{head}?{}{tail}", parts.join("&")));
    }
    (
        AppSpec {
            headers,
            env,
            args,
            url,
            ..spec.clone()
        },
        secrets,
    )
}

fn is_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with('~')
        || value.starts_with("./")
        || value.starts_with("../")
}

/// A key written into an app's arguments: where, a name for it, what stays before it,
/// and the key.
struct KeyInArgs {
    index: usize,
    label: String,
    lead: String,
    value: String,
}

/// Keys written into an app's arguments: the value after `--api-key`, the value in
/// `--token=X`, and a value that is a key by its shape. A reference or a path is not a key.
fn keys_in_args(args: &[String]) -> Vec<KeyInArgs> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if !secret_names(a).is_empty() {
            continue;
        }
        let split = a.split_once('=');
        if let Some((flag, value)) = split
            && !value.is_empty()
            && KEY_FLAG.is_match(flag)
            && !is_path(value)
        {
            out.push(KeyInArgs {
                index: i,
                label: flag.trim_start_matches('-').to_string(),
                lead: format!("{flag}="),
                value: value.to_string(),
            });
        } else if i > 0 && KEY_FLAG.is_match(&args[i - 1]) && !a.starts_with('-') && !is_path(a) {
            out.push(KeyInArgs {
                index: i,
                label: args[i - 1].trim_start_matches('-').to_string(),
                lead: String::new(),
                value: a.clone(),
            });
        } else if SECRET_SHAPES.is_match(a)
            || (looks_like_secret(a)
                && !a.chars().any(|c| "/@:=.".contains(c))
                && !a.starts_with('-'))
        {
            // A package's name has / or @; a key doesn't.
            out.push(KeyInArgs {
                index: i,
                label: format!("arg_{i}"),
                lead: String::new(),
                value: a.clone(),
            });
        }
    }
    out
}

/// Query parameters named for a key (`api_key=`, `token=`), as written.
fn keys_in_url(url: &str) -> Vec<(String, String)> {
    let query = url.split_once('?').map(|(_, rest)| rest).unwrap_or("");
    let query = query.split_once('#').map(|(q, _)| q).unwrap_or(query);
    let mut out = Vec::new();
    for part in query.split('&') {
        if let Some((param, value)) = part.split_once('=')
            && !value.is_empty()
            && KEY_PARAM.is_match(param)
            && secret_names(value).is_empty()
        {
            out.push((param.to_string(), value.to_string()));
        }
    }
    out
}
