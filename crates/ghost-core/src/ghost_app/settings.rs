//! The routes, quiet, the permissions and the settings on Ghost's own app (ADR 0031,
//! ADR 0034, ADR 0052).
//!
//! `routes_read` says how Ghost reaches the operator now; `quiet_until` and `quiet_off` set
//! do-not-disturb, the same effect as the fixed spoken intent "nothing until two". Which
//! routes are enabled and in what order stays the operator's file.
//!
//! `permissions_read` and `settings_read` are reads only, so an agent can say why something
//! was blocked or asked about. A model never changes a permission: there is no tool for it
//! here, and `tests::no_tool_changes_a_permission` holds that. Neither read returns a
//! password (name of a vault entry at most, never a value), a private address, or the
//! policy file's text beyond the permissions and the routes.

use chrono::{DateTime, Local, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{GhostServer, reply};
use crate::passwords::is_password_name;
use crate::redact::{password_names, redact_text};
use crate::routes::{Quiet, RoutePolicy, parse_quiet};
use crate::time::{iso, parse_iso};

/// The longest quiet a tool sets: a day, as the spoken intent's.
pub const QUIET_MOST_HOURS: i64 = 24;
/// A setting whose key has one of these words says it is an address or a place on the
/// network, and is left out.
const ADDRESS_WORDS: [&str; 13] = [
    "LISTEN",
    "ADDR",
    "ADDRS",
    "ADDRESS",
    "HOST",
    "HOSTS",
    "URL",
    "URI",
    "ENDPOINT",
    "PEER",
    "WIREGUARD",
    "IP",
    "SOCKET",
];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuietUntil {
    /// When the quiet ends: a time ("14:00", "2pm", "two"), a length ("45m", "2h", "for an hour"), or a UTC time (2026-11-12T14:00:00Z). A day at most.
    pub until: String,
}

fn when_local(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}

/// When a quiet asked for in `text` ends: the spoken intent's parser first, then a length
/// written short, then a UTC time.
pub fn quiet_end(text: &str, now: DateTime<Local>) -> Result<DateTime<Utc>, String> {
    let t = text.trim().to_lowercase();
    let short = regex::Regex::new(r"^(\d{1,4})\s*(m|min|mins|minutes?|h|hr|hrs|hours?)$")
        .expect("a fixed pattern");
    let end = if let Some(c) = short.captures(&t) {
        let n: i64 = c[1].parse().map_err(|_| "not a length")?;
        let mins = if c[2].starts_with('h') { n * 60 } else { n };
        Some(now.with_timezone(&Utc) + chrono::Duration::minutes(mins))
    } else if let Some(at) = parse_iso(&t.to_uppercase()) {
        Some(at)
    } else {
        let said = if t.starts_with("for ") || t.starts_with("until ") {
            format!("nothing {t}")
        } else {
            format!("nothing until {t}")
        };
        match parse_quiet(&said, now) {
            Some(Quiet::Until(at)) => Some(at),
            _ => None,
        }
    };
    let end = end.ok_or_else(|| {
        format!("{text} is not a time or a length Ghost reads (14:00, 2pm, 45m, 2h)")
    })?;
    let now = now.with_timezone(&Utc);
    if end <= now {
        return Err("that time has passed".into());
    }
    if end - now > chrono::Duration::hours(QUIET_MOST_HOURS) {
        return Err(format!("quiet is {QUIET_MOST_HOURS} hours at most"));
    }
    Ok(end)
}

/// The settings that may be shown: no password (Ghost's, an app's, one shaped like one),
/// no address, every value redacted.
pub fn shown_settings(all: &std::collections::BTreeMap<String, String>) -> Map<String, Value> {
    let held: Vec<String> = password_names().into_iter().collect();
    let held: Vec<&str> = held.iter().map(String::as_str).collect();
    all.iter()
        .filter(|(k, _)| !is_password_name(k, &held))
        .filter(|(k, _)| {
            let upper = k.to_uppercase();
            !upper.split('_').any(|w| ADDRESS_WORDS.contains(&w))
        })
        .map(|(k, v)| (k.clone(), json!(redact_text(v))))
        .collect()
}

fn routes_view(p: &RoutePolicy) -> Value {
    json!({
        "enabled": p.enabled.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
        "quiet_hours": p.quiet_hours.map(|(a, b)| format!("{}-{}", a.format("%H:%M"), b.format("%H:%M"))),
        "escalate_after_minutes": p.escalate_after,
        "batch_secs": p.batch_secs,
        "calls_per_hour": p.calls_per_hour,
        "calls_per_day": p.calls_per_day,
        "outside_routes_carry": p.carry.as_str(),
        "ntfy": if p.ntfy.is_some() { "set up" } else { "off" },
        "urgency": p.urgency.iter().map(|(n, u)| json!({"source": n, "urgency": u.as_str()})).collect::<Vec<_>>(),
    })
}

impl GhostServer {
    fn route_engine(&self) -> Result<std::sync::Arc<crate::routes::RouteEngine>, String> {
        self.more()
            .routes
            .clone()
            .ok_or_else(|| "the routes are not available here".to_string())
    }

    pub(crate) fn do_routes_read(&self) -> Result<Value, String> {
        let routes = self.route_engine()?;
        let now = Utc::now();
        let mut out = routes_view(&routes.policy());
        let dnd = routes.dnd_until().filter(|t| *t > now);
        out["do_not_disturb_until"] = json!(dnd.map(when_local));
        out["quiet_now"] = json!(routes.quiet(now));
        out["note"] = json!(
            "Which routes are on and their order are the operator's, in their policy file; quiet_until sets do-not-disturb."
        );
        Ok(out)
    }

    pub(crate) fn do_quiet_until(&self, p: &QuietUntil) -> Result<Value, String> {
        let routes = self.route_engine()?;
        let until = quiet_end(&p.until, Local::now())?;
        routes.quiet_until(Some(until)).map_err(|e| e.to_string())?;
        Ok(json!({"quiet_until": when_local(until), "utc": iso(&until), "status": "quiet"}))
    }

    pub(crate) fn do_quiet_off(&self) -> Result<Value, String> {
        self.route_engine()?
            .quiet_until(None)
            .map_err(|e| e.to_string())?;
        Ok(json!({"status": "not quiet"}))
    }

    pub(crate) fn do_permissions_read(&self) -> Result<Value, String> {
        let policy = (self
            .more()
            .policy
            .as_ref()
            .ok_or("the permissions are not available here")?)();
        let rules: Map<String, Value> = policy
            .permissions
            .rules()
            .into_iter()
            .map(|(list, rules)| (list.to_string(), json!(rules)))
            .collect();
        Ok(json!({
            "mode": policy.permissions.mode.as_str(),
            "rules": rules,
            "how": "The most careful rule that names a call wins: block, then ask, then quiet, then allow. What no rule names follows the mode. Ghost's own changes run quietly under the open mode, except connecting or disconnecting an app, writing a skill, and making or changing a watch, which ask.",
            "note": "Only the operator changes these, in their policy file or the desktop's Apps screen.",
        }))
    }

    pub(crate) fn do_settings_read(&self) -> Result<Value, String> {
        let settings = self
            .more()
            .settings
            .as_ref()
            .map(|f| shown_settings(&f()))
            .unwrap_or_default();
        let status = (self.parts.status)();
        Ok(json!({
            "settings": settings,
            "model": status.get("model").cloned().unwrap_or(Value::Null),
            "version": status.get("version").cloned().unwrap_or(Value::Null),
            "note": "Passwords and addresses are never shown. The operator changes settings in the desktop's Settings.",
        }))
    }
}

#[tool_router(router = routes_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "How Ghost reaches the operator now: the routes that are on, quiet hours, do-not-disturb, and the caps on calls.",
        annotations(read_only_hint = true)
    )]
    fn routes_read(&self) -> CallToolResult {
        reply(self.do_routes_read())
    }

    #[tool(
        description = "Do not disturb the operator until a time or for a while, as their \"nothing until two\" does: nothing is said, shown or rung but the digest."
    )]
    fn quiet_until(&self, Parameters(p): Parameters<QuietUntil>) -> CallToolResult {
        reply(self.do_quiet_until(&p))
    }

    #[tool(description = "End do-not-disturb now.")]
    fn quiet_off(&self) -> CallToolResult {
        reply(self.do_quiet_off())
    }
}

#[tool_router(router = settings_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "The operator's permissions: the mode and the block, ask, quiet and allow rules, to explain why a call was blocked or asked about. Read only.",
        annotations(read_only_hint = true)
    )]
    fn permissions_read(&self) -> CallToolResult {
        reply(self.do_permissions_read())
    }

    #[tool(
        description = "Ghost's settings that are not passwords, and the model connection's provider and model.",
        annotations(read_only_hint = true)
    )]
    fn settings_read(&self) -> CallToolResult {
        reply(self.do_settings_read())
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn a_quiet_reads_like_the_spoken_one_and_is_a_day_at_most() {
        let now = Local.with_ymd_and_hms(2026, 11, 12, 9, 0, 0).unwrap();
        let at = |t: &str| {
            quiet_end(t, now).map(|u| u.with_timezone(&Local).format("%H:%M").to_string())
        };
        assert_eq!(at("45m").unwrap(), "09:45");
        assert_eq!(at("2h").unwrap(), "11:00");
        assert_eq!(at("for an hour").unwrap(), "10:00");
        assert_eq!(at("two").unwrap(), "14:00");
        assert_eq!(at("14:30").unwrap(), "14:30");
        assert!(at("30h").unwrap_err().contains("24 hours"));
        assert!(at("later").is_err());
    }
}
