//! What one route (a provider, its address and a model) has shown it takes, so every
//! request is shaped to it (ADR 0056).
//!
//! A route is learned two ways. The provider check (`check`, the desktop's Test and
//! `ghostd llm test`) asks each thing once with a fixed synthetic prompt and the daemon
//! seeds what it found here at start from the kept results. And a request refused with
//! HTTP 400 for a field Ghost sent (the thinking setting, `reasoning_effort`, the cache
//! markers) is sent again once without it, and the route remembers, so the next turn does
//! not pay for the refusal: the Haiku 5.5 `between_tools` 400 of 2026-11-12, which failed
//! every turn of a fast model, is the case this was written for. Hermes Agent keeps the
//! same lesson as a failover reason ("send the disable no more on this route and retry",
//! `reasoning_mandatory` in its `agent/error_classifier.py`, MIT).
//!
//! Nothing here picks a provider or a model: what a route can't take is left out of its
//! requests, never swapped for another route.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use ghost_core::connection::Target;
use serde::{Deserialize, Serialize};

/// What a route has shown. `None` is not known yet: the request is sent as configured.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// A thinking setting the route refused (`between_tools`, or `reasoning_effort` on
    /// Chat Completions, as `"reasoning_effort"`); it is not sent again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejects_thinking: Option<String>,
    /// The route refused the cache markers; they are not sent again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rejects_cache_markers: bool,
    /// A second identical request read from the cache; None when not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caches: Option<bool>,
    /// It called the check's tool; None when not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    /// The reply came in more than one piece; None when not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streams: Option<bool>,
}

impl Capabilities {
    /// Whether `thinking` may be sent on this route.
    pub fn takes_thinking(&self, thinking: &str) -> bool {
        self.rejects_thinking.as_deref() != Some(thinking)
    }
}

/// A route's key: provider, protocol, address (without any credentials in it) and model.
/// Never the key.
pub fn route_key(target: &Target) -> String {
    format!(
        "{}|{}|{}|{}",
        target.provider,
        target.protocol,
        address(&target.base_url),
        target.model
    )
}

/// A URL with no user or password in it, and no trailing slash.
pub fn address(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => String::new(),
    }
}

static LEARNED: LazyLock<Mutex<HashMap<String, Capabilities>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn table() -> std::sync::MutexGuard<'static, HashMap<String, Capabilities>> {
    LEARNED.lock().unwrap_or_else(|e| e.into_inner())
}

/// What `target`'s route has shown so far.
pub fn learned(target: &Target) -> Capabilities {
    table().get(&route_key(target)).cloned().unwrap_or_default()
}

/// What a route showed, by key (the daemon's kept check results, at start).
pub fn seed(route: &str, caps: Capabilities) {
    table().insert(route.to_string(), caps);
}

/// Change what `target`'s route has shown.
pub fn learn(target: &Target, f: impl FnOnce(&mut Capabilities)) {
    let mut t = table();
    f(t.entry(route_key(target)).or_default());
}

/// Forget a route (a test's).
pub fn forget(target: &Target) {
    table().remove(&route_key(target));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_loses_its_credentials_and_query() {
        assert_eq!(
            address("http://user:pw@proxy:8317/v1/?x=1"),
            "http://proxy:8317/v1"
        );
        assert_eq!(
            address("https://openrouter.ai/api"),
            "https://openrouter.ai/api"
        );
        assert_eq!(address("nonsense"), "");
    }

    #[test]
    fn a_refused_thinking_mode_is_only_that_mode() {
        let c = Capabilities {
            rejects_thinking: Some("between_tools".into()),
            ..Capabilities::default()
        };
        assert!(!c.takes_thinking("between_tools"));
        assert!(c.takes_thinking("adaptive"));
    }
}
