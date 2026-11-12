//! What the daemon gives Ghost's own app (ADR 0045, `ghost_core::ghost_app`): the part of
//! `status` only it knows, built from the control API's own `Status`.

use std::sync::{Arc, Weak};

use ghost_api::Phase;
use ghost_api::models::Status;
use ghost_core::ghost_app::StatusFn;
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;
use crate::phone::PhoneHost;

/// The control API's `Status` as an agent says it: the version and commit, the phase,
/// the session and where it is, the agent, and the model connection by provider and model
/// name. Never a key, an address or a header.
pub fn status_fields(status: &Status, provider: &str, model: &str) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("version".into(), json!(status.version));
    out.insert("commit".into(), json!(status.build.commit));
    out.insert("build".into(), json!(status.build.kind));
    out.insert("phase".into(), json!(status.phase));
    out.insert("models_ready".into(), json!(status.models_ready));
    let open =
        status.session_id.is_some() && !matches!(status.phase, Phase::Passive | Phase::Starting);
    out.insert(
        "session".into(),
        json!({"open": open, "origin": status.session_origin}),
    );
    out.insert("agent".into(), json!(status.conversation));
    out.insert(
        "change_waiting_for_a_yes".into(),
        json!(status.pending.as_ref().map(|p| p.summary.clone())),
    );
    out.insert(
        "model".into(),
        json!({"provider": provider, "model": model}),
    );
    out
}

/// `status`'s host part for a running daemon.
pub fn status_fn(daemon: &Arc<Daemon>) -> StatusFn {
    let weak: Weak<Daemon> = Arc::downgrade(daemon);
    Arc::new(move || {
        let Some(d) = weak.upgrade() else {
            return Map::new();
        };
        let target = d.voice().llm_target(Some(&d.env()));
        status_fields(&PhoneHost::status(&*d), &target.provider, &target.model)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_api::models::SessionOrigin;

    #[test]
    fn status_says_the_session_the_agent_and_the_model_and_never_a_key() {
        let tmp = tempfile::tempdir().unwrap();
        let d = crate::daemon::tests::daemon(tmp.path());
        let mut status = PhoneHost::status(&*d);
        let quiet = status_fields(&status, "anthropic", "claude-sonnet-5-5");
        assert_eq!(quiet["session"], json!({"open": false, "origin": null}));
        assert_eq!(quiet["version"], json!(status.version));
        assert_eq!(
            quiet["model"],
            json!({"provider": "anthropic", "model": "claude-sonnet-5-5"})
        );
        status.phase = Phase::Listening;
        status.session_id = Some("s1".into());
        status.session_origin = Some(SessionOrigin::Phone);
        status.conversation = Some("ledger".into());
        let live = status_fields(&status, "openrouter", "x/y");
        assert_eq!(live["phase"], "listening");
        assert_eq!(live["session"], json!({"open": true, "origin": "phone"}));
        assert_eq!(live["agent"], "ledger");

        // The daemon's own: the provider and the model's name, with a key in the
        // environment that never shows.
        d.environ
            .lock()
            .unwrap()
            .insert("ANTHROPIC_API_KEY".into(), "sk-ant-not-for-agents".into());
        let said = Value::Object(status_fn(&d)()).to_string();
        assert!(said.contains("\"provider\""));
        assert!(!said.contains("sk-ant-not-for-agents"));
    }
}

/// What an agent's `app_disconnect` forgets with the app (ADR 0052), as the desktop's
/// disconnect does: its sign-in, and the passwords Ghost kept for it that no other app
/// names (`GHOST_APP_*`). A vault that is locked keeps them; the operator removes them in
/// Settings › Passwords.
pub fn forget_app(
    apps: &Arc<ghost_core::Apps>,
    signins: &Arc<ghost_core::signin::SignIns>,
    passwords: &Arc<ghost_core::Passwords>,
) -> ghost_core::ghost_app::more::Forget {
    let (apps, signins, passwords) = (
        Arc::downgrade(apps),
        signins.clone(),
        Arc::downgrade(passwords),
    );
    Arc::new(move |spec: &ghost_core::AppSpec| {
        signins.forget(&spec.name);
        let others: Vec<String> = apps
            .upgrade()
            .map(|a| {
                a.specs()
                    .iter()
                    .filter(|s| s.name != spec.name)
                    .flat_map(ghost_core::AppSpec::secrets)
                    .collect()
            })
            .unwrap_or_default();
        let gone: Vec<String> = spec
            .secrets()
            .into_iter()
            .filter(|k| k.starts_with("GHOST_APP_") && !others.contains(k))
            .collect();
        if let Some(vault) = passwords.upgrade()
            && vault.is_open()
            && !gone.is_empty()
        {
            let names: Vec<&str> = gone.iter().map(String::as_str).collect();
            if let Err(e) = vault.delete(&names) {
                tracing::warn!("apps: {}'s passwords were not forgotten: {e}", spec.name);
            }
        }
    })
}
