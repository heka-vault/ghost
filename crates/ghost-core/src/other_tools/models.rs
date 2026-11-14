//! A model connection that is already there (ADR 0068): a local server answering on its
//! default port, or a provider's key in the environment Ghost was started in, which Ghost
//! reads already (ADR 0037). Never another tool's sign-in: Claude Code's and Codex's
//! subscriptions are licensed to those tools, and nothing here opens their files.

use std::collections::HashMap;

use serde_json::Value;

/// A local server Ghost looks for: the provider id, its title, and where it answers.
pub struct LocalServer {
    pub provider: &'static str,
    pub title: &'static str,
    /// The address asked for the model list; loopback only.
    pub list_url: &'static str,
}

pub const LOCAL_SERVERS: [LocalServer; 2] = [
    LocalServer {
        provider: "ollama",
        title: "Ollama",
        list_url: "http://127.0.0.1:11434/api/tags",
    },
    LocalServer {
        provider: "lmstudio",
        title: "LM Studio",
        list_url: "http://127.0.0.1:1234/v1/models",
    },
];

/// Models listed at most, and characters of one model's name.
pub const MODELS_MAX: usize = 50;
const NAME_MAX: usize = 200;

/// The provider each key variable is for, in the order offered.
pub const KEY_VARIABLES: [(&str, &str); 3] = [
    ("ANTHROPIC_API_KEY", "anthropic"),
    ("OPENROUTER_API_KEY", "openrouter"),
    ("OPENAI_API_KEY", "openai"),
];

/// The model names in what a local server answered: Ollama's `{"models": [{"name"}]}` or
/// the OpenAI-shaped `{"data": [{"id"}]}` LM Studio sends. Bounded; never an error.
pub fn model_names(body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let items = v
        .get("models")
        .or_else(|| v.get("data"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|m| {
            m.get("name")
                .or_else(|| m.get("model"))
                .or_else(|| m.get("id"))
                .and_then(Value::as_str)
        })
        .filter(|n| !n.trim().is_empty())
        .take(MODELS_MAX)
        .map(|n| crate::other_tools::bounded(&n.chars().take(NAME_MAX).collect::<String>()))
        .collect()
}

/// The providers whose key is set in `started`, the environment Ghost was started in:
/// (variable, provider). Only whether it is set; never its value.
pub fn keys_present(started: &HashMap<String, String>) -> Vec<(&'static str, &'static str)> {
    KEY_VARIABLES
        .into_iter()
        .filter(|(var, _)| started.get(*var).is_some_and(|v| !v.trim().is_empty()))
        .collect()
}

/// How long a local server has to answer.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);

/// One GET of a loopback address, bounded in time and size; None for anything that is not
/// loopback, that does not answer, or that answers with an error.
pub async fn get_local(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "http" || !matches!(parsed.host_str(), Some("127.0.0.1") | Some("[::1]"))
    {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > 512 * 1024 {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}
