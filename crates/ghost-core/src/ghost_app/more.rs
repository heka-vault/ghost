//! What the wider tools of Ghost's own app reach (ADR 0052), and the three lists that
//! shape them: the core every turn carries, the changes that ask by default, and the tools
//! an outside client is never offered.
//!
//! Agents reach everything Ghost can do through these tools, under the same gate, audit
//! and permissions as any app. What still holds: no tool sets a permission or reads a
//! password, `policy.toml` and `hooks.json` are never written, and nothing returns a
//! passive transcript or audio.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::GhostServer;
use super::outside::AgentRunner;
use crate::catalog::Catalog;
use crate::conversations::Conversations;
use crate::policy::AutonomyPolicy;
use crate::routes::RouteEngine;
use crate::skills::Skills;
use crate::watches::WatchRunner;

/// The tools every turn carries by name; the rest of Ghost's own are found with
/// `ghost_tools_search` and offered by name from the next model call (ADR 0046's
/// deferral, applied to Ghost's own app by ADR 0052). Why each is here:
///
/// - `notify`, `resolve`, `ask`, `answer`: how an agent reaches the operator; `answer` must
///   be there in the turn the operator answers, and the prompt names all four.
/// - `waiting_ack`: "got it" at the desk, in the same turn.
/// - `status`, `docs_search`: the prompt sends every "can Ghost …?" there first.
/// - `recall`: "what did we say about …" is a spoken question, answered in one turn.
/// - `schedule_manage`: "check the build every morning" is the commonest thing asked of an
///   agent, and a search first would cost a model call every time.
/// - `tools_search`: how the rest are found.
pub const CORE: [&str; 10] = [
    "notify",
    "resolve",
    "ask",
    "answer",
    "waiting_ack",
    "status",
    "docs_search",
    "recall",
    "schedule_manage",
    "tools_search",
];

/// Changes of Ghost's own app that wait for the operator's yes when no rule names them
/// (ADR 0052): the rest are `quiet` by ADR 0045. Each one runs or contacts code the
/// operator did not write, or adds instructions every agent may follow.
pub fn asks_by_default(tool: &str, args: Option<&Map<String, Value>>) -> bool {
    match tool {
        "app_connect" | "app_disconnect" | "skill_write" => true,
        // A new watch or a changed one polls an app on its own from then on; pausing,
        // resuming or removing one an agent made is housekeeping. With no arguments to
        // judge by, the careful answer.
        "watch_manage" => args
            .and_then(|a| a.get("action"))
            .and_then(Value::as_str)
            .is_none_or(|a| matches!(a.trim().to_lowercase().as_str(), "create" | "update")),
        _ => false,
    }
}

/// Ghost's own tools an outside client is never offered (ADR 0052): what reads an agent's
/// private memory, the operator's settings or permissions, the apps' own text; and every
/// change to apps, watches, quiet, outputs or skills. `outside::tests` and ghostd's
/// `nothing_an_outside_client_must_not_reach_is_offered` hold this line.
pub const NOT_FOR_OUTSIDE: [&str; 15] = [
    "memory_read",
    "app_connect",
    "app_disconnect",
    "app_tools",
    "apps_catalog_search",
    "watch_manage",
    "permissions_read",
    "settings_read",
    "skill_write",
    "quiet_until",
    "quiet_off",
    "output_read",
    "output_delete",
    "ask_agent",
    "tools_search",
];

/// The operator's policy as it is now (the gate's), for `permissions_read` and
/// `routes_read`.
pub type PolicyFn = Arc<dyn Fn() -> AutonomyPolicy + Send + Sync>;
/// `settings.env` as it is now: every key, passwords included; `settings_read` keeps the
/// ones that may be shown.
pub type SettingsFn = Arc<dyn Fn() -> BTreeMap<String, String> + Send + Sync>;
/// Ghost's folder was changed by a tool: read it again (the watches restart).
pub type Refresh = Arc<dyn Fn() + Send + Sync>;
/// What a disconnected app leaves behind that the desktop's disconnect also forgets: its
/// sign-in and the passwords Ghost kept for it.
pub type Forget = Arc<dyn Fn(&crate::apps::AppSpec) + Send + Sync>;

/// What the wider tools reach. Each is optional: a server without one does not have the
/// tools that need it, so a test or a smaller daemon builds only what it gives.
#[derive(Clone, Default)]
pub struct More {
    /// The agents' folders (memory, notes, persona).
    pub agents: Option<Arc<Conversations>>,
    /// Ghost's folder (`~/.config/ghost`): `apps.json`, `watches.json`, `out/`.
    pub config_dir: Option<PathBuf>,
    pub catalog: Option<Arc<Catalog>>,
    pub forget: Option<Forget>,
    pub watches: Option<Arc<WatchRunner>>,
    pub refresh: Option<Refresh>,
    pub routes: Option<Arc<RouteEngine>>,
    pub policy: Option<PolicyFn>,
    pub settings: Option<SettingsFn>,
    pub skills: Option<Arc<Skills>>,
    /// A written turn with an agent (`ask_agent`): the run with nobody there.
    pub runner: Option<Arc<dyn AgentRunner>>,
}

impl GhostServer {
    /// The same server with what the wider tools reach, and those tools.
    pub fn with_more(mut self, more: More) -> Self {
        if more.agents.is_some() {
            self.tool_router.merge(Self::memory_router());
        }
        if more.config_dir.is_some() {
            self.tool_router.merge(Self::apps_router());
            self.tool_router.merge(Self::watches_router());
            self.tool_router.merge(Self::outputs_router());
        }
        if more.routes.is_some() {
            self.tool_router.merge(Self::routes_router());
        }
        if more.policy.is_some() {
            self.tool_router.merge(Self::settings_router());
        }
        if more.skills.is_some() {
            self.tool_router.merge(Self::skills_router());
        }
        if more.runner.is_some() {
            self.tool_router.merge(Self::agents_router());
        }
        self.more = Arc::new(more);
        self
    }

    pub(super) fn more(&self) -> &More {
        &self.more
    }

    /// The calling agent's slug, for a tool only Ghost's own agents have.
    pub(super) fn agent_only(&self, said: Option<String>) -> Result<String, String> {
        let who = self.agent(said)?;
        match who.strip_prefix(super::AGENT_SOURCE) {
            Some(slug) => Ok(slug.to_string()),
            None => Err("this is for Ghost's own agents".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_asks_by_default() {
        let args = |a: &str| json!({"action": a}).as_object().cloned().unwrap();
        for tool in ["app_connect", "app_disconnect", "skill_write"] {
            assert!(asks_by_default(tool, None));
        }
        assert!(asks_by_default("watch_manage", None));
        assert!(asks_by_default("watch_manage", Some(&args("create"))));
        assert!(asks_by_default("watch_manage", Some(&args(" Update"))));
        for a in ["pause", "resume", "remove"] {
            assert!(!asks_by_default("watch_manage", Some(&args(a))));
        }
        for tool in ["notify", "quiet_until", "output_delete", "ask_agent"] {
            assert!(!asks_by_default(tool, None));
        }
    }
}
