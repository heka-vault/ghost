//! The model connection (ADR 0037): one place that says where every model call goes.
//!
//! Spoken sessions with their tool calls, compaction, a watch's model step and the latency
//! harness all reach the model through the connection this module resolves. A provider is
//! picked like an app is: Anthropic with a password, OpenRouter by signing in, the operator's
//! Claude subscription through CLIProxy, OpenAI or a local Ollama or LM Studio, or a URL of
//! the operator's own that speaks either protocol.
//!
//! Two protocols carry it: Anthropic's Messages API and the OpenAI-compatible Chat
//! Completions API (the voice crate has a client for each). The key is a Password: the
//! connection names it (`GHOST_LLM_PASSWORD`, a variable in Ghost's environment) and it is
//! read by that name when a target is built, never stored here, shown, or logged.
//!
//! The non-secret settings, all in settings.env:
//!
//! ```text
//! GHOST_LLM_PROVIDER     anthropic, openrouter, cliproxy, openai, ollama, lmstudio, custom
//! GHOST_LLM_MODEL        the model id as the provider names it (a default per provider)
//! GHOST_LLM_PROTOCOL     anthropic or openai; a custom URL only (the others are fixed)
//! GHOST_LLM_BASE_URL     overrides the provider's address (folded away in the desktop)
//! GHOST_LLM_PASSWORD     the Password's name, when not the provider's own
//! GHOST_LLM_HEADERS      extra headers, ``Name: value; Name: value``, sent as written
//! GHOST_LLM_TIMEOUT_SECS how long a call may take before it fails
//! ```
//!
//! With no `GHOST_LLM_PROVIDER` the settings from before keep working unchanged:
//! `GHOST_LLM_ENDPOINT=openrouter` is OpenRouter with `OPENROUTER_API_KEY`, and the
//! default (`proxy`) is `ANTHROPIC_BASE_URL` with `ANTHROPIC_AUTH_TOKEN`.
//!
//! The environment is a parameter (`Env`): the daemon's own, in memory, which the vault
//! fills (ADR 0036). Nothing here reads the process environment on its own.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::hash::{BuildHasher, Hash};
use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const PROTOCOLS: [&str; 2] = ["anthropic", "openai"];
pub const LEGACY_ENDPOINTS: [&str; 2] = ["proxy", "openrouter"];
pub const DEFAULT_CLAUDE: &str = "claude-sonnet-5-5";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// OpenRouter's Anthropic Messages API.
pub const OPENROUTER_ANTHROPIC_URL: &str = "https://openrouter.ai/api";
pub const OPENROUTER_AUTH_URL: &str = "https://openrouter.ai/auth";
pub const OPENROUTER_KEYS_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
pub const CLIPROXY_DEFAULT_URL: &str = "http://localhost:8317";
pub const LIST_TIMEOUT_SECS: f64 = 8.0;
const EXCHANGE_TIMEOUT_SECS: f64 = 15.0;

/// The environment keys this module reads (besides the Password it names).
pub const ENV_KEYS: [&str; 7] = [
    "GHOST_LLM_PROVIDER",
    "GHOST_LLM_MODEL",
    "GHOST_LLM_PROTOCOL",
    "GHOST_LLM_BASE_URL",
    "GHOST_LLM_PASSWORD",
    "GHOST_LLM_HEADERS",
    "GHOST_LLM_TIMEOUT_SECS",
];

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]{1,63}$").unwrap());
static CLAUDE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(claude-[a-z]+)-(\d+)(?:-(\d{1,2}))?$").unwrap());
static FULL_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://\S+$").unwrap());

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ConnectionError {
    /// `GHOST_LLM_HEADERS` does not read: a sentence the operator can act on.
    #[error("{0}")]
    Headers(String),
    /// The provider's address did not answer at all.
    #[error("couldn’t reach {host} ({kind})")]
    Unreachable { host: String, kind: String },
    /// The provider refused the key. The key is never in the message.
    #[error("{title} refused the key (HTTP {status})")]
    Refused { title: String, status: u16 },
    /// The provider answered an error.
    #[error("{host} answered HTTP {status}")]
    Status { host: String, status: u16 },
    #[error("the answer wasn’t JSON")]
    NotJson,
    #[error("the answer had no model list in it")]
    NoModelList,
    #[error("couldn’t reach OpenRouter ({0})")]
    SignInUnreachable(String),
    #[error("OpenRouter refused the sign-in (HTTP {0})")]
    SignInRefused(u16),
    #[error("OpenRouter's answer had no key in it")]
    NoKey,
}

/// The environment a connection is resolved from. Any string map will do; the daemon
/// passes its own, in memory, where the vault fills the keys (ADR 0036).
pub trait Env {
    fn var(&self, key: &str) -> Option<&str>;
}

impl<K: Borrow<str> + Eq + Hash, V: AsRef<str>, S: BuildHasher> Env for HashMap<K, V, S> {
    fn var(&self, key: &str) -> Option<&str> {
        self.get(key).map(AsRef::as_ref)
    }
}

impl<K: Borrow<str> + Ord, V: AsRef<str>> Env for BTreeMap<K, V> {
    fn var(&self, key: &str) -> Option<&str> {
        self.get(key).map(AsRef::as_ref)
    }
}

impl<E: Env + ?Sized> Env for &E {
    fn var(&self, key: &str) -> Option<&str> {
        (**self).var(key)
    }
}

/// The value, stripped; "" when unset.
fn setting(env: &impl Env, key: &str) -> String {
    env.var(key).unwrap_or("").trim().to_string()
}

/// Which API a provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Anthropic,
    Openai,
}

impl Protocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Protocol::Anthropic => "anthropic",
            Protocol::Openai => "openai",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "anthropic" => Some(Protocol::Anthropic),
            "openai" => Some(Protocol::Openai),
            _ => None,
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One way to reach a model, as the desktop offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub protocol: Protocol,
    pub base_url: &'static str,
    /// The Password's name; "" when none is needed.
    pub password: &'static str,
    /// A local server, or a proxy that may take no key.
    pub password_optional: bool,
    /// OpenRouter: the key comes back from signing in.
    pub sign_in: bool,
    /// Runs on this machine (or the operator's network).
    pub local: bool,
    /// "" means the first the provider lists.
    pub default_model: &'static str,
    /// The Chat Completions field for the output cap: OpenAI's own API wants
    /// `max_completion_tokens`; the compatible servers know `max_tokens`.
    pub cap_param: &'static str,
}

const PROVIDER_DEFAULTS: Provider = Provider {
    id: "",
    title: "",
    blurb: "",
    protocol: Protocol::Anthropic,
    base_url: "",
    password: "",
    password_optional: false,
    sign_in: false,
    local: false,
    default_model: "",
    cap_param: "max_tokens",
};

/// The providers, in the order the desktop offers them.
pub static PROVIDERS: [Provider; 7] = [
    Provider {
        id: "anthropic",
        title: "Anthropic",
        blurb: "Claude, with a password (API key) from the Anthropic Console.",
        protocol: Protocol::Anthropic,
        base_url: "https://api.anthropic.com",
        password: "ANTHROPIC_API_KEY",
        default_model: DEFAULT_CLAUDE,
        ..PROVIDER_DEFAULTS
    },
    Provider {
        id: "openrouter",
        title: "OpenRouter",
        blurb: "Sign in, and Claude or any other model OpenRouter offers.",
        protocol: Protocol::Anthropic,
        base_url: OPENROUTER_ANTHROPIC_URL,
        password: "OPENROUTER_API_KEY",
        sign_in: true,
        default_model: "anthropic/claude-sonnet-5.5",
        ..PROVIDER_DEFAULTS
    },
    Provider {
        id: "cliproxy",
        title: "Claude subscription",
        blurb: "Your Claude plan, through CLIProxy running on your network.",
        protocol: Protocol::Anthropic,
        base_url: CLIPROXY_DEFAULT_URL,
        password: "ANTHROPIC_AUTH_TOKEN",
        password_optional: true,
        local: true,
        default_model: DEFAULT_CLAUDE,
        ..PROVIDER_DEFAULTS
    },
    Provider {
        id: "openai",
        title: "OpenAI",
        blurb: "GPT models, with a password (API key) from the OpenAI platform.",
        protocol: Protocol::Openai,
        base_url: "https://api.openai.com/v1",
        password: "OPENAI_API_KEY",
        default_model: "gpt-5",
        cap_param: "max_completion_tokens",
        ..PROVIDER_DEFAULTS
    },
    Provider {
        id: "ollama",
        title: "Ollama",
        blurb: "A model running on this computer. Nothing leaves it.",
        protocol: Protocol::Openai,
        base_url: "http://localhost:11434/v1",
        local: true,
        ..PROVIDER_DEFAULTS
    },
    Provider {
        id: "lmstudio",
        title: "LM Studio",
        blurb: "A model LM Studio serves on this computer. Nothing leaves it.",
        protocol: Protocol::Openai,
        base_url: "http://localhost:1234/v1",
        local: true,
        ..PROVIDER_DEFAULTS
    },
    Provider {
        id: "custom",
        title: "Your own address",
        blurb: "Any address that speaks Anthropic’s or OpenAI’s API.",
        protocol: Protocol::Anthropic,
        base_url: "",
        password: "LLM_API_KEY",
        password_optional: true,
        ..PROVIDER_DEFAULTS
    },
];

/// The provider with this id.
pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

fn provider_ids() -> String {
    PROVIDERS
        .iter()
        .map(|p| p.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every Password a provider may use by default: the desktop may set these.
pub static PASSWORD_NAMES: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
    PROVIDERS
        .iter()
        .filter(|p| !p.password.is_empty())
        .map(|p| p.password)
        .collect()
});

/// A Claude id as OpenRouter names it: `claude-sonnet-5-5` is
/// `anthropic/claude-sonnet-5.5`. An id that already has a vendor prefix is kept.
pub fn openrouter_model(model: &str) -> String {
    if model.contains('/') {
        return model.to_string();
    }
    let Some(m) = CLAUDE_ID.captures(model) else {
        return format!("anthropic/{model}");
    };
    let (name, major) = (&m[1], &m[2]);
    match m.get(3) {
        Some(minor) => format!("anthropic/{name}-{major}.{}", minor.as_str()),
        None => format!("anthropic/{name}-{major}"),
    }
}

pub fn is_password_name(name: &str) -> bool {
    NAME.is_match(name)
}

/// The Passwords a model connection may use: every provider's own, and the one the
/// settings name (`GHOST_LLM_PASSWORD`).
pub fn password_names(values: &impl Env) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = PASSWORD_NAMES.iter().map(|s| s.to_string()).collect();
    let named = setting(values, "GHOST_LLM_PASSWORD");
    if is_password_name(&named) {
        names.insert(named);
    }
    names
}

static HEADER_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9!#%&'*+.^_`|~-]+$").unwrap());
/// Kept out of a value: what settings.env would have to quote or escape, and what a shell
/// reading the file would expand. A header that carries a key belongs in the Password.
const HEADER_UNSAFE: [char; 6] = ['"', '\\', '$', ';', '\n', '\r'];

/// Python's `repr` of a string, for the messages that quote what the operator wrote.
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

/// Headers in the order written; a name written twice keeps its place with the later
/// value, as a Python dict would.
pub type Headers = Vec<(String, String)>;

fn put_header(out: &mut Headers, name: String, value: String) {
    match out.iter_mut().find(|(n, _)| *n == name) {
        Some(slot) => slot.1 = value,
        None => out.push((name, value)),
    }
}

/// `GHOST_LLM_HEADERS`: `Name: value` pairs separated by semicolons. Fails with a
/// sentence the operator can act on.
pub fn parse_headers(raw: Option<&str>) -> Result<Headers, ConnectionError> {
    let mut out = Headers::new();
    for part in raw.unwrap_or("").split(';') {
        if part.trim().is_empty() {
            continue;
        }
        let (name, value, has_sep) = match part.split_once(':') {
            Some((n, v)) => (n.trim(), v.trim(), true),
            None => (part.trim(), "", false),
        };
        if !has_sep || name.is_empty() {
            return Err(ConnectionError::Headers(
                "write extra headers as Name: value, separated by semicolons".into(),
            ));
        }
        if !HEADER_NAME.is_match(name) {
            return Err(ConnectionError::Headers(format!(
                "{} is not a header name",
                repr(name)
            )));
        }
        if value.chars().any(|c| HEADER_UNSAFE.contains(&c)) {
            return Err(ConnectionError::Headers(format!(
                "{name}'s value can't hold quotes, backslashes, $ or ;"
            )));
        }
        put_header(&mut out, name.to_string(), value.to_string());
    }
    Ok(out)
}

/// The inverse of `parse_headers`, checked the same way.
pub fn format_headers<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<String, ConnectionError> {
    let text = headers
        .into_iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("; ");
    parse_headers(Some(&text))?;
    Ok(text)
}

// --- models that reason ---------------------------------------------------------------

/// Room for a reasoning model's hidden tokens on top of the reply's own cap. OpenAI counts
/// reasoning against max_completion_tokens, so without it a tool-heavy turn could end at
/// finish_reason=length before a word or a call came out. A cap only bounds a reply: an
/// answer that needs less costs nothing more.
pub const REASONING_HEADROOM: u32 = 6000;

static OPENAI_O: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^o(\d+)(-[a-z0-9.-]+)?$").unwrap());
static OPENAI_GPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^gpt-(\d+)(?:\.(\d+))?(-[a-z0-9.-]+)?$").unwrap());

/// What a model does before it answers, as far as Ghost's request needs to know.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reasoning {
    /// Spends output tokens on reasoning before the reply.
    pub reasons: bool,
    /// The reasoning_effort to send; "" sends none.
    pub effort: &'static str,
}

impl Reasoning {
    const fn new(reasons: bool, effort: &'static str) -> Self {
        Reasoning { reasons, effort }
    }
}

/// OpenAI's reasoning models, by id (`openai/` prefix allowed), and the lowest
/// reasoning effort each takes on Chat Completions with tools, since Ghost answers by
/// voice and every token of thought is a pause (OpenAI's model docs, 2026-10):
///
/// - GPT-5.1 and later, and GPT-6: `none`, which is also the only effort GPT-5.4 and
///   later combine with function tools on Chat Completions. GPT-6 Astra and GPT-6.1 Sol
///   don't take `none`: `low`.
/// - The first GPT-5 models (gpt-5, -mini, -nano): `low`. Their `minimal` exists but
///   turns off parallel tool calls, and Ghost runs every call of a reply together.
/// - o-series: `low`. o1-mini and o1-preview take no effort at all.
/// - A `-pro` model only reasons at `high` and a `-chat` one is tuned for chat:
///   neither gets an effort, both get the headroom.
///
/// Anything else (Claude, a local model, an id this doesn't know) is not a reasoning model
/// here, and its request is as before.
pub fn reasoning(model: &str) -> Reasoning {
    let lower = model.trim().to_lowercase();
    let mid = lower.strip_prefix("openai/").unwrap_or(&lower);
    if OPENAI_O.is_match(mid) {
        if mid.starts_with("o1-mini") || mid.starts_with("o1-preview") {
            return Reasoning::new(true, "");
        }
        return Reasoning::new(true, "low");
    }
    let Some(m) = OPENAI_GPT.captures(mid) else {
        return Reasoning::default();
    };
    let major: u64 = m[1].parse().unwrap_or(u64::MAX);
    if major < 5 {
        return Reasoning::default();
    }
    let minor = m.get(2).map(|x| x.as_str());
    let rest = m.get(3).map(|x| x.as_str()).unwrap_or("");
    if rest.contains("-chat") || rest.contains("-pro") {
        return Reasoning::new(true, "");
    }
    if major == 5 && minor.is_none() {
        return Reasoning::new(true, "low");
    }
    if major == 6 && minor.is_none() && rest.starts_with("-astra") {
        return Reasoning::new(true, "low");
    }
    if major == 6 && minor == Some("1") && rest.starts_with("-sol") {
        return Reasoning::new(true, "low");
    }
    Reasoning::new(true, "none")
}

/// `scheme://host[:port]` of a URL, "" when it has no host.
fn host(url: &str) -> String {
    let Ok(u) = url::Url::parse(url) else {
        return String::new();
    };
    let Some(h) = u.host_str() else {
        return String::new();
    };
    let h = h.trim_start_matches('[').trim_end_matches(']');
    match u.port() {
        Some(p) => format!("{}://{}:{}", u.scheme(), h, p),
        None => format!("{}://{}", u.scheme(), h),
    }
}

/// A connection resolved for one call site: where, which model, the key. `api_key`
/// and the resolved `headers` are never in a `Debug`, and `describe` leaves them out.
#[derive(Clone)]
pub struct Target {
    pub provider: String,
    pub protocol: Protocol,
    pub base_url: String,
    /// As the provider names it on the wire.
    pub model: String,
    pub api_key: Zeroizing<String>,
    pub headers: Headers,
    pub timeout_secs: Option<f64>,
    pub cap_param: &'static str,
    /// Chat Completions only.
    pub reasoning: Reasoning,
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target")
            .field("provider", &self.provider)
            .field("protocol", &self.protocol)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout_secs", &self.timeout_secs)
            .field("cap_param", &self.cap_param)
            .field("reasoning", &self.reasoning)
            .finish_non_exhaustive()
    }
}

impl Target {
    /// The older name, for logs and reports.
    pub fn endpoint(&self) -> &str {
        &self.provider
    }

    /// The cap to send for a reply of up to `max_tokens` visible tokens: more for a
    /// model that spends some of it reasoning first.
    pub fn output_cap(&self, max_tokens: u32) -> u32 {
        if self.reasoning.reasons {
            max_tokens + REASONING_HEADROOM
        } else {
            max_tokens
        }
    }

    /// The Chat Completions fields for the cap and the reasoning effort.
    pub fn chat_params(&self, max_tokens: u32) -> Map<String, Value> {
        let mut out = Map::new();
        out.insert(
            self.cap_param.to_string(),
            json!(self.output_cap(max_tokens)),
        );
        if !self.reasoning.effort.is_empty() {
            out.insert("reasoning_effort".into(), json!(self.reasoning.effort));
        }
        out
    }

    pub fn describe(&self) -> String {
        let where_ = host(&self.base_url);
        let where_ = if where_.is_empty() {
            "no address"
        } else {
            &where_
        };
        format!(
            "{} via {} ({}, {})",
            self.model, self.provider, self.protocol, where_
        )
    }

    /// What tells two targets' clients apart, without holding the key.
    pub fn fingerprint(&self) -> String {
        let sorted: BTreeMap<&str, &str> = self
            .headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let headers = format!(
            "{{{}}}",
            sorted
                .iter()
                .map(|(k, v)| format!("{}: {}", json!(k), json!(v)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let timeout = match self.timeout_secs {
            Some(t) => format!("{t:?}"),
            None => "None".to_string(),
        };
        let mut h = Sha256::new();
        for part in [
            self.protocol.as_str(),
            &self.base_url,
            &self.api_key,
            &headers,
            &timeout,
        ] {
            h.update(part.as_bytes());
            h.update(b"\0");
        }
        format!("{:x}", h.finalize())
    }
}

/// The operator's model connection, before any Password is read.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelConnection {
    pub provider: String,
    pub protocol: Protocol,
    /// Resolved: the override, else the provider's.
    pub base_url: String,
    /// As the operator wrote it ("" means the provider's default).
    pub model: String,
    /// A name.
    pub password: String,
    /// Sent as written.
    pub headers: Headers,
    pub timeout_secs: Option<f64>,
    pub base_url_override: String,
    /// The older setting this came from, when it did.
    pub legacy: String,
    pub problems_in_settings: Vec<String>,
}

impl ModelConnection {
    pub fn spec(&self) -> &'static Provider {
        provider(&self.provider).unwrap_or_else(|| panic!("no provider {}", self.provider))
    }

    pub fn wire_model(&self) -> String {
        let mut model = if self.model.is_empty() {
            self.spec().default_model.to_string()
        } else {
            self.model.clone()
        };
        if model.is_empty() && self.protocol == Protocol::Anthropic {
            model = DEFAULT_CLAUDE.to_string(); // an Anthropic-compatible URL: Claude, as before
        }
        if self.provider == "openrouter" && !model.is_empty() {
            return openrouter_model(&model);
        }
        model
    }

    pub fn key(&self, env: &impl Env) -> Zeroizing<String> {
        if self.password.is_empty() {
            return Zeroizing::new(String::new());
        }
        Zeroizing::new(env.var(&self.password).unwrap_or("").to_string())
    }

    pub fn target(&self, env: &impl Env) -> Target {
        let model = self.wire_model();
        let (mut cap_param, mut thinks) = ("max_tokens", Reasoning::default());
        if self.protocol == Protocol::Openai {
            // A reasoning model is one wherever it is served, and OpenAI's take only
            // max_completion_tokens, so the model decides both, not the provider.
            thinks = reasoning(&model);
            cap_param = if thinks.reasons {
                "max_completion_tokens"
            } else {
                self.spec().cap_param
            };
        }
        Target {
            provider: self.provider.clone(),
            protocol: self.protocol,
            base_url: self.base_url.clone(),
            model,
            api_key: self.key(env),
            headers: self.headers.clone(),
            timeout_secs: self.timeout_secs,
            cap_param,
            reasoning: thinks,
        }
    }

    pub fn password_set(&self, env: &impl Env) -> bool {
        !self.key(env).is_empty()
    }

    /// What stops the model from being reachable, in words, without naming a value.
    /// `locked`: Ghost's passwords can't be read now (the keyring is locked), so a key
    /// missing from `env` may well be set; that is said instead of "not set".
    pub fn problems(&self, env: &impl Env, locked: bool) -> Vec<String> {
        let spec = self.spec();
        let mut out = self.problems_in_settings.clone();
        if self.base_url.is_empty() {
            out.push(format!(
                "{} needs an address (GHOST_LLM_BASE_URL)",
                spec.title
            ));
        }
        if self.wire_model().is_empty() {
            out.push("no model is chosen (GHOST_LLM_MODEL)".into());
        }
        let needs_key = !self.password.is_empty() && !spec.password_optional;
        if needs_key && !self.password_set(env) {
            if locked {
                out.push(format!(
                    "your keyring is locked, so Ghost can't read {}. Unlock it on Settings › Passwords",
                    self.password
                ));
            } else if !self.legacy.is_empty() {
                out.push(format!("{} but {} is not set", self.legacy, self.password));
            } else if spec.sign_in {
                out.push(format!(
                    "sign in to {} ({} is not set)",
                    spec.title, self.password
                ));
            } else {
                out.push(format!("{} is not set", self.password));
            }
        }
        out
    }

    pub fn describe(&self) -> String {
        let where_ = host(&self.base_url);
        let where_ = if where_.is_empty() {
            "no address"
        } else {
            &where_
        };
        let model = self.wire_model();
        let model = if model.is_empty() {
            "(no model)"
        } else {
            &model
        };
        format!(
            "{} via {} ({}, {})",
            model, self.provider, self.protocol, where_
        )
    }

    /// The connection the environment describes. Unknown values don't fail: they are
    /// problems, so a typo shows on the desktop instead of keeping Ghost from starting.
    pub fn from_env(env: &impl Env) -> Self {
        let mut problems: Vec<String> = Vec::new();
        let get = |k: &str| setting(env, k);
        let mut provider_id = get("GHOST_LLM_PROVIDER").to_lowercase();
        let mut legacy = String::new();
        let mut base_override = get("GHOST_LLM_BASE_URL");
        let mut password = get("GHOST_LLM_PASSWORD");
        let mut protocol = get("GHOST_LLM_PROTOCOL").to_lowercase();
        if !provider_id.is_empty() && provider(&provider_id).is_none() {
            problems.push(format!(
                "GHOST_LLM_PROVIDER={provider_id} is not one of {}",
                provider_ids()
            ));
            provider_id.clear();
        }
        if provider_id.is_empty() {
            // Before the connection: GHOST_LLM_ENDPOINT picked proxy or OpenRouter.
            let endpoint = get("GHOST_LLM_ENDPOINT").to_lowercase();
            let endpoint = if endpoint.is_empty() {
                "proxy"
            } else {
                &endpoint
            };
            if endpoint == "openrouter" {
                provider_id = "openrouter".into();
                legacy = "GHOST_LLM_ENDPOINT=openrouter".into();
            } else {
                let legacy_base = get("ANTHROPIC_BASE_URL");
                let cliproxy = get("CLIPROXY_BASE_URL");
                provider_id = if !legacy_base.is_empty()
                    && !cliproxy.is_empty()
                    && legacy_base.trim_end_matches('/') == cliproxy.trim_end_matches('/')
                {
                    "cliproxy"
                } else if !legacy_base.is_empty() {
                    "custom"
                } else {
                    "anthropic"
                }
                .into();
                if base_override.is_empty() {
                    base_override = legacy_base;
                }
                let has_token = env
                    .var("ANTHROPIC_AUTH_TOKEN")
                    .is_some_and(|v| !v.is_empty());
                if password.is_empty() && (provider_id != "anthropic" || has_token) {
                    password = "ANTHROPIC_AUTH_TOKEN".into();
                }
            }
        }
        let spec = provider(&provider_id).expect("a known provider");
        let resolved_protocol = if provider_id == "custom" {
            if !protocol.is_empty() && Protocol::parse(&protocol).is_none() {
                problems.push(format!(
                    "GHOST_LLM_PROTOCOL={protocol} is not anthropic or openai"
                ));
                protocol.clear();
            }
            Protocol::parse(&protocol).unwrap_or(Protocol::Anthropic)
        } else {
            spec.protocol
        };
        let mut base = base_override.clone();
        if base.is_empty() && provider_id == "cliproxy" {
            base = get("CLIPROXY_BASE_URL");
        }
        if base.is_empty() {
            base = spec.base_url.to_string();
        }
        let base = base.trim_end_matches('/').to_string();
        if !base_override.is_empty() && !FULL_URL.is_match(&base_override) {
            problems
                .push("GHOST_LLM_BASE_URL must be a full URL, like http://localhost:8080".into());
        }
        if !password.is_empty() && !is_password_name(&password) {
            problems.push("GHOST_LLM_PASSWORD must be a name like MY_MODEL_KEY".into());
            password.clear();
        }
        let headers = match parse_headers(env.var("GHOST_LLM_HEADERS")) {
            Ok(h) => h,
            Err(e) => {
                problems.push(format!("GHOST_LLM_HEADERS: {e}"));
                Headers::new()
            }
        };
        let mut timeout: Option<f64> = None;
        let raw = get("GHOST_LLM_TIMEOUT_SECS");
        if !raw.is_empty() {
            match raw.parse::<f64>() {
                Ok(t) if t > 0.0 => timeout = Some(t),
                _ => problems
                    .push("GHOST_LLM_TIMEOUT_SECS must be a number of seconds above 0".into()),
            }
        }
        ModelConnection {
            provider: provider_id,
            protocol: resolved_protocol,
            base_url: base,
            model: get("GHOST_LLM_MODEL"),
            password: if password.is_empty() {
                spec.password.to_string()
            } else {
                password
            },
            headers,
            timeout_secs: timeout,
            base_url_override: base_override,
            legacy,
            problems_in_settings: problems,
        }
    }
}

// --- the model list -------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

fn parse_models(data: &Value) -> Result<Vec<ModelOption>, ConnectionError> {
    let rows = match data {
        Value::Object(o) => o.get("data"),
        other => Some(other),
    };
    let rows = match rows {
        Some(Value::Array(a)) => a,
        // Ollama's own /api/tags shape, should a base URL point there.
        _ => match data.get("models") {
            Some(Value::Array(a)) => a,
            _ => return Err(ConnectionError::NoModelList),
        },
    };
    let mut out: Vec<ModelOption> = Vec::new();
    for r in rows {
        let Value::Object(r) = r else { continue };
        let mid = ["id", "name", "model"]
            .iter()
            .find_map(|k| r.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()));
        let Some(mid) = mid else { continue };
        let name = ["display_name", "name"]
            .iter()
            .find_map(|k| r.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
            .filter(|n| *n != mid)
            .unwrap_or("");
        let option = ModelOption {
            id: mid.to_string(),
            name: name.to_string(),
        };
        match out.iter_mut().find(|m| m.id == mid) {
            Some(slot) => *slot = option,
            None => out.push(option),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Where the provider lists its models: `/v1/models` beside the Messages API, or
/// `/models` under an OpenAI-compatible base (which already ends in `/v1`).
pub fn models_url(conn: &ModelConnection) -> String {
    let path = match conn.protocol {
        Protocol::Anthropic => "/v1/models",
        Protocol::Openai => "/models",
    };
    format!("{}{}", conn.base_url, path)
}

/// The request `list_models` sends, before it is sent: the key goes only to the
/// provider's own address, as a header.
#[derive(Clone)]
pub struct ModelsRequest {
    pub url: String,
    pub headers: Headers,
    pub query: Vec<(String, String)>,
}

impl ModelsRequest {
    /// A header by name, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub fn models_request(conn: &ModelConnection, env: &impl Env) -> ModelsRequest {
    let mut headers = conn.headers.clone();
    let key = conn.key(env);
    match conn.protocol {
        Protocol::Anthropic => {
            put_header(
                &mut headers,
                "anthropic-version".into(),
                ANTHROPIC_VERSION.into(),
            );
            if !key.is_empty() {
                put_header(&mut headers, "x-api-key".into(), key.to_string());
                if conn.provider != "anthropic" {
                    // OpenRouter, most proxies
                    put_header(
                        &mut headers,
                        "Authorization".into(),
                        format!("Bearer {}", *key),
                    );
                }
            }
        }
        Protocol::Openai => {
            if !key.is_empty() {
                put_header(
                    &mut headers,
                    "Authorization".into(),
                    format!("Bearer {}", *key),
                );
            }
        }
    }
    let query = if conn.provider == "anthropic" {
        vec![("limit".to_string(), "1000".to_string())]
    } else {
        Vec::new()
    };
    ModelsRequest {
        url: models_url(conn),
        headers,
        query,
    }
}

fn header_map(headers: &Headers) -> Result<HeaderMap, ConnectionError> {
    let mut out = HeaderMap::new();
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| ConnectionError::Headers(format!("{} is not a header name", repr(k))))?;
        let value = HeaderValue::from_str(v).map_err(|_| {
            ConnectionError::Headers(format!("{k}'s value can't be sent as a header"))
        })?;
        out.insert(name, value);
    }
    Ok(out)
}

/// httpx's name for what went wrong, as the Python said it.
fn error_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "ReadTimeout"
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_request() {
        "RequestError"
    } else {
        "HTTPError"
    }
}

fn own_client(timeout_secs: f64) -> Result<reqwest::Client, ConnectionError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs_f64(timeout_secs))
        .build()
        .map_err(|e| ConnectionError::Unreachable {
            host: "it".into(),
            kind: error_kind(&e).into(),
        })
}

/// The provider's models. Fails with a short reason; the key is sent only to the
/// provider's own address and never appears in the reason.
pub async fn list_models(
    conn: &ModelConnection,
    env: &impl Env,
    client: Option<&reqwest::Client>,
) -> Result<Vec<ModelOption>, ConnectionError> {
    let req = models_request(conn, env);
    let headers = header_map(&req.headers)?;
    let own;
    let http = match client {
        Some(c) => c,
        None => {
            own = own_client(LIST_TIMEOUT_SECS)?;
            &own
        }
    };
    let where_ = host(&conn.base_url);
    let r = http
        .get(&req.url)
        .headers(headers)
        .query(&req.query)
        .send()
        .await
        .map_err(|e| ConnectionError::Unreachable {
            host: if where_.is_empty() {
                "it".into()
            } else {
                where_.clone()
            },
            kind: error_kind(&e).into(),
        })?;
    let status = r.status().as_u16();
    if status == 401 || status == 403 {
        return Err(ConnectionError::Refused {
            title: conn.spec().title.into(),
            status,
        });
    }
    if status >= 400 {
        return Err(ConnectionError::Status {
            host: where_,
            status,
        });
    }
    let data: Value = r.json().await.map_err(|_| ConnectionError::NotJson)?;
    parse_models(&data)
}

// --- signing in to OpenRouter (OAuth PKCE) --------------------------------------------

/// The verifier and the state are never in a `Debug`.
#[derive(Clone)]
pub struct Pkce {
    pub verifier: Zeroizing<String>,
    pub challenge: String,
    pub state: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

pub fn new_pkce() -> Pkce {
    let mut raw = Zeroizing::new([0u8; 48]);
    rand::rng().fill_bytes(raw.as_mut());
    let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(raw.as_ref()));
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut state_raw = [0u8; 24];
    rand::rng().fill_bytes(&mut state_raw);
    Pkce {
        verifier,
        challenge,
        state: URL_SAFE_NO_PAD.encode(state_raw),
    }
}

/// The page the browser opens. OpenRouter takes a localhost callback on any port and
/// returns `code` (and our `state`) to it (openrouter.ai/docs, OAuth PKCE).
pub fn openrouter_auth_url(callback_url: &str, pkce: &Pkce) -> String {
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("callback_url", callback_url)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &pkce.state)
        .finish();
    format!("{OPENROUTER_AUTH_URL}?{q}")
}

/// Trade the code for a key. Fails with a reason that holds no secret.
pub async fn openrouter_exchange(
    code: &str,
    pkce: &Pkce,
    client: Option<&reqwest::Client>,
) -> Result<Zeroizing<String>, ConnectionError> {
    openrouter_exchange_at(OPENROUTER_KEYS_URL, code, pkce, client).await
}

/// `openrouter_exchange` against another address (a test's).
pub async fn openrouter_exchange_at(
    keys_url: &str,
    code: &str,
    pkce: &Pkce,
    client: Option<&reqwest::Client>,
) -> Result<Zeroizing<String>, ConnectionError> {
    let own;
    let http = match client {
        Some(c) => c,
        None => {
            own = own_client(EXCHANGE_TIMEOUT_SECS)
                .map_err(|_| ConnectionError::SignInUnreachable("HTTPError".into()))?;
            &own
        }
    };
    let body = json!({
        "code": code,
        "code_verifier": *pkce.verifier,
        "code_challenge_method": "S256",
    });
    let r = http
        .post(keys_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| ConnectionError::SignInUnreachable(error_kind(&e).into()))?;
    let status = r.status().as_u16();
    if status >= 400 {
        return Err(ConnectionError::SignInRefused(status));
    }
    let data: Value = r.json().await.unwrap_or(Value::Null);
    match data.get("key").and_then(Value::as_str) {
        Some(key) if !key.is_empty() => Ok(Zeroizing::new(key.to_string())),
        _ => Err(ConnectionError::NoKey),
    }
}

#[cfg(test)]
mod tests {
    //! The model connection: providers, the settings from before, Passwords by name, the
    //! model list and OpenRouter's sign-in.

    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const OR_KEY: &str = "sk-or-v1-fedcba9876543210";
    const TOKEN: &str = "sk-proxy-0123456789abcdef";

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_operators_setup_from_before_is_openrouter_unchanged() {
        // Their env on 2026-10-28: GHOST_LLM_ENDPOINT=openrouter, no model set, and
        // ANTHROPIC_BASE_URL pointing at CLIProxy (unused by the endpoint).
        let env = env(&[
            ("GHOST_LLM_ENDPOINT", "openrouter"),
            ("OPENROUTER_API_KEY", OR_KEY),
            ("ANTHROPIC_BASE_URL", "http://192.168.4.67:8317"),
            ("CLIPROXY_BASE_URL", "http://192.168.4.67:8317"),
        ]);
        let c = ModelConnection::from_env(&env);
        let t = c.target(&env);
        assert_eq!(
            (c.provider.as_str(), t.protocol, t.base_url.as_str()),
            (
                "openrouter",
                Protocol::Anthropic,
                "https://openrouter.ai/api"
            )
        );
        assert_eq!(t.model, "anthropic/claude-sonnet-5.5");
        assert_eq!(*t.api_key, OR_KEY);
        assert_eq!(c.legacy, "GHOST_LLM_ENDPOINT=openrouter");
        assert!(c.problems(&env, false).is_empty());
    }

    #[test]
    fn the_proxy_from_before_is_the_anthropic_base_url_with_its_token() {
        let mut env = env(&[
            ("ANTHROPIC_BASE_URL", "http://192.168.1.5:8317/"),
            ("ANTHROPIC_AUTH_TOKEN", TOKEN),
        ]);
        let c = ModelConnection::from_env(&env);
        let t = c.target(&env);
        assert_eq!(
            (
                c.provider.as_str(),
                t.base_url.as_str(),
                t.model.as_str(),
                t.api_key.as_str()
            ),
            (
                "custom",
                "http://192.168.1.5:8317",
                "claude-sonnet-5-5",
                TOKEN
            )
        );
        assert_eq!(t.protocol, Protocol::Anthropic);
        // Said to be CLIProxy when it is the CLIProxy address.
        env.insert("CLIPROXY_BASE_URL".into(), "http://192.168.1.5:8317".into());
        assert_eq!(ModelConnection::from_env(&env).provider, "cliproxy");
    }

    #[test]
    fn no_settings_at_all_is_anthropic_and_says_what_is_missing() {
        let empty = env(&[]);
        let c = ModelConnection::from_env(&empty);
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.base_url, "https://api.anthropic.com");
        assert_eq!(
            c.problems(&empty, false),
            vec!["ANTHROPIC_API_KEY is not set"]
        );
        // A legacy token with no address is Anthropic itself, as the SDK default was.
        let env = env(&[("ANTHROPIC_AUTH_TOKEN", TOKEN)]);
        assert_eq!(*ModelConnection::from_env(&env).target(&env).api_key, TOKEN);
    }

    #[test]
    fn a_provider_wins_over_the_older_settings() {
        let env = env(&[
            ("GHOST_LLM_PROVIDER", "ollama"),
            ("GHOST_LLM_ENDPOINT", "openrouter"),
            ("GHOST_LLM_MODEL", "qwen3:8b"),
        ]);
        let c = ModelConnection::from_env(&env);
        let t = c.target(&env);
        assert_eq!(
            (
                t.provider.as_str(),
                t.protocol,
                t.base_url.as_str(),
                t.model.as_str(),
                t.api_key.as_str()
            ),
            (
                "ollama",
                Protocol::Openai,
                "http://localhost:11434/v1",
                "qwen3:8b",
                ""
            )
        );
        assert!(c.problems(&env, false).is_empty()); // a local server needs no key
    }

    #[test]
    fn every_provider_has_a_protocol_and_an_address_or_says_so() {
        for p in PROVIDERS.iter() {
            let env = env(&[("GHOST_LLM_PROVIDER", p.id), ("GHOST_LLM_MODEL", "m")]);
            let c = ModelConnection::from_env(&env);
            assert!(matches!(c.protocol, Protocol::Anthropic | Protocol::Openai));
            if p.id == "custom" {
                assert!(
                    c.problems(&HashMap::<String, String>::new(), false)
                        .iter()
                        .any(|x| x.contains("needs an address"))
                );
            } else {
                assert!(c.base_url.starts_with("http"));
            }
        }
    }

    #[test]
    fn openai_uses_its_own_cap_field() {
        let env1 = env(&[("GHOST_LLM_PROVIDER", "openai"), ("OPENAI_API_KEY", "sk-x")]);
        assert_eq!(
            ModelConnection::from_env(&env1).target(&env1).cap_param,
            "max_completion_tokens"
        );
        let env2 = env(&[("GHOST_LLM_PROVIDER", "lmstudio"), ("GHOST_LLM_MODEL", "m")]);
        assert_eq!(
            ModelConnection::from_env(&env2).target(&env2).cap_param,
            "max_tokens"
        );
    }

    #[test]
    fn reasoning_is_a_property_of_the_model() {
        let cases: &[(&str, bool, &str)] = &[
            // From 5.1 on, and GPT-6: no reasoning, the only effort 5.4+ takes with tools.
            ("gpt-5.1", true, "none"),
            ("gpt-5.5", true, "none"),
            ("gpt-5.6-terra", true, "none"),
            ("gpt-6-sol", true, "none"),
            ("gpt-6.1-terra", true, "none"),
            ("openai/gpt-5.4-mini", true, "none"),
            // No "none" on these: low.
            ("gpt-6-astra", true, "low"),
            ("gpt-6.1-sol", true, "low"),
            // The first GPT-5s: low, since minimal turns off parallel tool calls.
            ("gpt-5", true, "low"),
            ("gpt-5-mini", true, "low"),
            ("gpt-5-nano-2025-08-07", true, "low"),
            ("o3", true, "low"),
            ("o4-mini", true, "low"),
            ("o1-mini", true, ""),
            // Reason, but take no lower effort than their own.
            ("gpt-5-pro", true, ""),
            ("gpt-5.1-chat-latest", true, ""),
            // Not reasoning models.
            ("gpt-4.1", false, ""),
            ("gpt-4o-mini", false, ""),
            ("claude-sonnet-5-5", false, ""),
            ("qwen3:8b", false, ""),
            ("llama3.1", false, ""),
        ];
        for (model, reasons, effort) in cases {
            assert_eq!(reasoning(model).reasons, *reasons, "{model}");
            assert_eq!(reasoning(model).effort, *effort, "{model}");
        }
    }

    #[test]
    fn a_reasoning_model_gets_its_lowest_effort_and_room_to_reason() {
        let env1 = env(&[
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_MODEL", "gpt-5.5"),
            ("OPENAI_API_KEY", "k"),
        ]);
        let t = ModelConnection::from_env(&env1).target(&env1);
        assert_eq!(
            Value::Object(t.chat_params(2000)),
            json!({"max_completion_tokens": 2000 + REASONING_HEADROOM, "reasoning_effort": "none"})
        );
        // The model decides, not the provider: a gpt-5 behind your own address too ...
        let env2 = env(&[
            ("GHOST_LLM_PROVIDER", "custom"),
            ("GHOST_LLM_PROTOCOL", "openai"),
            ("GHOST_LLM_BASE_URL", "http://localhost:4000/v1"),
            ("GHOST_LLM_MODEL", "gpt-5"),
        ]);
        let t = ModelConnection::from_env(&env2).target(&env2);
        assert_eq!(
            Value::Object(t.chat_params(2000)),
            json!({"max_completion_tokens": 2000 + REASONING_HEADROOM, "reasoning_effort": "low"})
        );
        // ... and a model that doesn't reason, even at OpenAI, keeps the plain cap.
        let env3 = env(&[
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_MODEL", "gpt-4.1"),
            ("OPENAI_API_KEY", "k"),
        ]);
        assert_eq!(
            Value::Object(
                ModelConnection::from_env(&env3)
                    .target(&env3)
                    .chat_params(2000)
            ),
            json!({"max_completion_tokens": 2000})
        );
        let env4 = env(&[
            ("GHOST_LLM_PROVIDER", "ollama"),
            ("GHOST_LLM_MODEL", "qwen3:8b"),
        ]);
        assert_eq!(
            Value::Object(
                ModelConnection::from_env(&env4)
                    .target(&env4)
                    .chat_params(2000)
            ),
            json!({"max_tokens": 2000})
        );
    }

    #[test]
    fn the_messages_api_never_gets_a_reasoning_effort() {
        // An OpenAI id named on an Anthropic-protocol provider (OpenRouter's Messages API).
        let env = env(&[
            ("GHOST_LLM_PROVIDER", "openrouter"),
            ("GHOST_LLM_MODEL", "openai/gpt-5.5"),
        ]);
        let t = ModelConnection::from_env(&env).target(&env);
        assert!(!t.reasoning.reasons);
        assert_eq!(t.output_cap(2000), 2000);
    }

    #[test]
    fn a_custom_url_picks_its_protocol_and_password_by_name() {
        let env = env(&[
            ("GHOST_LLM_PROVIDER", "custom"),
            ("GHOST_LLM_PROTOCOL", "openai"),
            ("GHOST_LLM_BASE_URL", "http://gpu-box:8000/v1"),
            ("GHOST_LLM_MODEL", "llama"),
            ("GHOST_LLM_PASSWORD", "GPU_BOX_KEY"),
            ("GPU_BOX_KEY", "k-123"),
            (
                "GHOST_LLM_HEADERS",
                "X-Team: ghost; HTTP-Referer: https://ghost.local/app",
            ),
            ("GHOST_LLM_TIMEOUT_SECS", "30"),
        ]);
        let c = ModelConnection::from_env(&env);
        let t = c.target(&env);
        assert_eq!(
            (
                t.protocol,
                t.base_url.as_str(),
                t.api_key.as_str(),
                t.timeout_secs
            ),
            (
                Protocol::Openai,
                "http://gpu-box:8000/v1",
                "k-123",
                Some(30.0)
            )
        );
        assert_eq!(
            t.headers,
            vec![
                ("X-Team".to_string(), "ghost".to_string()),
                (
                    "HTTP-Referer".to_string(),
                    "https://ghost.local/app".to_string()
                ),
            ]
        );
    }

    #[test]
    fn bad_settings_are_problems_not_crashes() {
        let env = env(&[
            ("GHOST_LLM_PROVIDER", "skynet"),
            ("GHOST_LLM_HEADERS", "X-Title Ghost"),
            ("GHOST_LLM_TIMEOUT_SECS", "soon"),
            ("GHOST_LLM_PASSWORD", "my key"),
            ("GHOST_LLM_BASE_URL", "localhost:8080"),
        ]);
        let c = ModelConnection::from_env(&env);
        let words = c.problems(&env, false).join(" ");
        for bit in [
            "GHOST_LLM_PROVIDER=skynet",
            "Name: value",
            "seconds",
            "a name like",
            "full URL",
        ] {
            assert!(words.contains(bit), "{bit} not in {words}");
        }
    }

    #[test]
    fn keys_never_show() {
        let env = env(&[
            ("GHOST_LLM_PROVIDER", "openrouter"),
            ("OPENROUTER_API_KEY", OR_KEY),
        ]);
        let c = ModelConnection::from_env(&env);
        let t = c.target(&env);
        let mut shown = vec![
            format!("{c:?}"),
            format!("{t:?}"),
            t.describe(),
            c.describe(),
        ];
        shown.extend(c.problems(&env, false));
        let shown = shown.join(" ");
        assert!(!shown.contains(OR_KEY));
        assert!(t.describe().contains("via openrouter"));
        let p = new_pkce();
        let shown = format!("{p:?}");
        assert!(!shown.contains(p.verifier.as_str()) && !shown.contains(&p.state));
    }

    #[test]
    fn problems_name_the_missing_password_not_a_value() {
        let env1 = env(&[("GHOST_LLM_ENDPOINT", "openrouter")]);
        assert_eq!(
            ModelConnection::from_env(&env1).problems(&env1, false),
            vec!["GHOST_LLM_ENDPOINT=openrouter but OPENROUTER_API_KEY is not set"]
        );
        let env2 = env(&[("GHOST_LLM_PROVIDER", "openrouter")]);
        assert_eq!(
            ModelConnection::from_env(&env2).problems(&env2, false),
            vec!["sign in to OpenRouter (OPENROUTER_API_KEY is not set)"]
        );
        let env3 = env(&[
            ("GHOST_LLM_PROVIDER", "custom"),
            ("GHOST_LLM_BASE_URL", "http://x:1"),
        ]);
        // a URL of your own may take no key
        assert!(
            ModelConnection::from_env(&env3)
                .problems(&env3, false)
                .is_empty()
        );
    }

    #[test]
    fn openrouter_model_ids() {
        assert_eq!(
            openrouter_model("claude-sonnet-5-5"),
            "anthropic/claude-sonnet-5.5"
        );
        assert_eq!(
            openrouter_model("claude-haiku-4-5"),
            "anthropic/claude-haiku-4.5"
        );
        assert_eq!(
            openrouter_model("claude-sonnet-5"),
            "anthropic/claude-sonnet-5"
        );
        assert_eq!(openrouter_model("openai/gpt-5"), "openai/gpt-5");
    }

    #[test]
    fn headers_parse() {
        assert_eq!(parse_headers(Some("")).unwrap(), Headers::new());
        assert_eq!(
            parse_headers(Some("X-Title: Ghost; ;HTTP-Referer: https://a.b/c")).unwrap(),
            vec![
                ("X-Title".to_string(), "Ghost".to_string()),
                ("HTTP-Referer".to_string(), "https://a.b/c".to_string()),
            ]
        );
        assert_eq!(
            format_headers([("X-Title", "Ghost"), ("X-B", "1")]).unwrap(),
            "X-Title: Ghost; X-B: 1"
        );
        let e = parse_headers(Some("bad name: x")).unwrap_err();
        assert!(e.to_string().contains("not a header name"), "{e}");
        // What settings.env would quote, or a shell reading it would expand, stays out.
        for value in ["\"x\"", "$KEY", "${KEY}", "a\\b"] {
            let e = parse_headers(Some(&format!("X-A: {value}"))).unwrap_err();
            assert!(e.to_string().contains("can't hold"), "{e}");
        }
    }

    // --- the model list ----------------------------------------------------------------

    #[tokio::test]
    async fn models_from_anthropic_send_the_key_to_its_own_address_only() {
        let env1 = env(&[
            ("GHOST_LLM_PROVIDER", "anthropic"),
            ("ANTHROPIC_API_KEY", "sk-ant-1"),
        ]);
        // What goes out, before it goes: Anthropic's own address, the key as x-api-key.
        let req = models_request(&ModelConnection::from_env(&env1), &env1);
        assert!(req.url.starts_with("https://api.anthropic.com/v1/models"));
        assert_eq!(req.header("x-api-key"), Some("sk-ant-1"));
        assert_eq!(req.header("authorization"), None);
        assert_eq!(req.query, vec![("limit".to_string(), "1000".to_string())]);

        let server = MockServer::start().await;
        let body = json!({"data": [
            {"id": "claude-sonnet-5-5", "display_name": "Claude Sonnet 5.5"},
            {"id": "claude-haiku-5-5"},
        ]});
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let env2 = env(&[
            ("GHOST_LLM_PROVIDER", "anthropic"),
            ("GHOST_LLM_BASE_URL", &server.uri()),
            ("ANTHROPIC_API_KEY", "sk-ant-1"),
        ]);
        let models = list_models(&ModelConnection::from_env(&env2), &env2, None)
            .await
            .unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["claude-haiku-5-5", "claude-sonnet-5-5"]
        );
        assert_eq!(models[1].name, "Claude Sonnet 5.5");
        let seen = server.received_requests().await.unwrap();
        let r = &seen[0];
        assert_eq!(r.url.path(), "/v1/models");
        assert_eq!(r.url.query(), Some("limit=1000"));
        assert_eq!(r.headers.get("x-api-key").unwrap(), "sk-ant-1");
        assert!(r.headers.get("authorization").is_none());
    }

    #[tokio::test]
    async fn models_from_an_openai_compatible_server() {
        let server = MockServer::start().await;
        let body = json!({"object": "list", "data": [{"id": "qwen3:8b", "object": "model"}]});
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let plain = env(&[("GHOST_LLM_PROVIDER", "ollama")]);
        assert_eq!(
            models_url(&ModelConnection::from_env(&plain)),
            "http://localhost:11434/v1/models"
        );
        let env = env(&[
            ("GHOST_LLM_PROVIDER", "ollama"),
            ("GHOST_LLM_BASE_URL", &format!("{}/v1", server.uri())),
        ]);
        let models = list_models(&ModelConnection::from_env(&env), &env, None)
            .await
            .unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["qwen3:8b"]
        );
        let seen = server.received_requests().await.unwrap();
        assert_eq!(seen[0].url.path(), "/v1/models");
        assert!(seen[0].url.query().is_none());
        assert!(seen[0].headers.get("authorization").is_none());
    }

    #[tokio::test]
    async fn a_refused_key_is_said_without_the_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let env = env(&[
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_BASE_URL", &format!("{}/v1", server.uri())),
            ("OPENAI_API_KEY", "sk-secret-1"),
        ]);
        let e = list_models(&ModelConnection::from_env(&env), &env, None)
            .await
            .unwrap_err();
        let said = e.to_string();
        assert!(
            said.contains("refused the key") && !said.contains("sk-secret-1"),
            "{said}"
        );
        assert_eq!(said, "OpenAI refused the key (HTTP 401)");
    }

    // --- OpenRouter's sign-in ----------------------------------------------------------

    #[test]
    fn the_sign_in_address_carries_the_challenge_and_state() {
        let p = new_pkce();
        let u =
            url::Url::parse(&openrouter_auth_url("http://localhost:47823/callback", &p)).unwrap();
        let q: BTreeMap<String, String> = u.query_pairs().into_owned().collect();
        assert_eq!(
            (u.scheme(), u.host_str(), u.path()),
            ("https", Some("openrouter.ai"), "/auth")
        );
        let want: BTreeMap<String, String> = [
            ("callback_url", "http://localhost:47823/callback"),
            ("code_challenge", p.challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", p.state.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(q, want);
        assert!(!u.query().unwrap().contains(p.verifier.as_str()));
        let want = URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, want);
    }

    #[tokio::test]
    async fn the_code_is_traded_for_a_key() {
        assert_eq!(
            OPENROUTER_KEYS_URL,
            "https://openrouter.ai/api/v1/auth/keys"
        );
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"key": OR_KEY})))
            .mount(&server)
            .await;
        let keys_url = format!("{}/api/v1/auth/keys", server.uri());
        let p = new_pkce();
        let key = openrouter_exchange_at(&keys_url, "code-1", &p, None)
            .await
            .unwrap();
        assert_eq!(*key, OR_KEY);
        let seen = server.received_requests().await.unwrap();
        let r = &seen[0];
        assert_eq!(
            (r.method.as_str(), r.url.path()),
            ("POST", "/api/v1/auth/keys")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&r.body).unwrap(),
            json!({
                "code": "code-1",
                "code_verifier": *p.verifier,
                "code_challenge_method": "S256",
            })
        );
        let refusing = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&refusing)
            .await;
        let keys_url = format!("{}/api/v1/auth/keys", refusing.uri());
        let e = openrouter_exchange_at(&keys_url, "code-1", &p, None)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("refused"), "{e}");
    }
}
