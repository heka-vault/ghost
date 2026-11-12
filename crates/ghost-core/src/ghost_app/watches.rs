//! Watches on Ghost's own app (ADR 0028, ADR 0052): `watches_list`, a read, and
//! `watch_manage`, a change through the gate.
//!
//! An agent may make a polled watch (one of a connected app's read-only tools, called on an
//! interval by the daemon with no model, its fields kept and checked deterministically,
//! then told, recorded or acted on), change it, pause, resume and remove it. Making one and
//! changing one wait for the operator's yes unless their permissions say otherwise
//! (`more::asks_by_default`); the rest is quiet. Each entry an agent writes says who made it
//! (`made_by`). An agent manages the watches it made; Ghost (agent zero) manages any
//! agent's; the operator's own are theirs alone. A watch an agent makes declares no Jev or
//! model step: those are the operator's to declare.
//!
//! `watches.json` is written by one writer: only the entry named changes, the operator's
//! other entries and their order are kept, and the result is refused whole when it would
//! not pass what `ghostd check` applies (the same parser) or names an app or a tool that is
//! not a connected app's read. Webhooks (`hooks.json`, signatures, parsers) stay the
//! operator's: nothing here reads or writes that file.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{AGENT_SOURCE, GhostServer, reply};
use crate::apps::{edit_keyed, write_folder_file};
use crate::conversations::GHOST;
use crate::folder::WATCHES_FILE;
use crate::time::iso;
use crate::watches::{Watch, parse_watch, parse_watches};

/// Keys an agent's watch may not set: who made it and whether it is paused are Ghost's
/// to write, and a Jev or model step is the operator's to declare.
const NOT_AN_AGENTS: [&str; 5] = ["made_by", "paused", "jev", "model", "server"];

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct WatchManage {
    /// create, update, pause, resume or remove.
    pub action: String,
    /// The watch's name: lowercase letters, digits, - and _.
    pub name: String,
    /// For create and update, the declaration as watches.json has it: app, read (a read-only tool of that app), args, every (seconds, 15 to 86400), items (where the list is), fields ({name: {at, type}}), when ([{field, op, value}]), key and title (templates over the fields), response (tell, record or act), act ({agent, instruction}), urgency (digest, notify or call). Update merges what is given.
    #[serde(default)]
    pub watch: Option<Map<String, Value>>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

fn watch_view(w: &Watch) -> Value {
    json!({
        "name": w.name,
        "reads": format!("{}.{}", w.app, w.read),
        "every_secs": w.every,
        "response": match (&w.act, w.response) {
            (Some(_), crate::watches::Choice::Record) => "act".to_string(),
            (Some(_), r) => format!("{} and act", r.as_str()),
            (None, r) => r.as_str().to_string(),
        },
        "acts_through": w.act.as_ref().map(|a| a.agent.clone()),
        "urgency": w.urgency,
        "made_by": w.made_by.clone().unwrap_or_else(|| "operator".into()),
        "state": if w.paused { "paused" } else { "on" },
    })
}

impl GhostServer {
    fn watches_path(&self) -> Result<std::path::PathBuf, String> {
        Ok(self
            .more()
            .config_dir
            .as_ref()
            .ok_or("Ghost's folder is not available here")?
            .join(WATCHES_FILE))
    }

    fn read_watches(&self) -> Result<(String, Value), String> {
        let path = self.watches_path()?;
        let text = if path.is_file() {
            std::fs::read_to_string(&path).map_err(|e| e.to_string())?
        } else {
            String::new()
        };
        let body = if text.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str(&text)
                .map_err(|e| format!("{WATCHES_FILE} is not JSON ({e}); the operator fixes it"))?
        };
        Ok((text, body))
    }

    pub(crate) fn do_watches_list(&self) -> Result<Value, String> {
        let (_, body) = self.read_watches()?;
        let (watches, problems) = parse_watches(&body, WATCHES_FILE);
        let runner = self
            .more()
            .watches
            .as_ref()
            .and_then(|late| late.get().cloned());
        let rows: Vec<Value> = watches
            .values()
            .map(|w| {
                let mut row = watch_view(w);
                if let Some(st) = runner.as_ref().and_then(|r| r.state_of(&w.source())) {
                    row["last_check"] = json!(st.last_run.as_ref().map(iso));
                    row["last_ok"] = json!(st.last_ok.as_ref().map(iso));
                    row["error"] = json!(st.error);
                    row["items_seen"] = json!(st.seen);
                    row["items_open"] = json!(st.open);
                }
                row
            })
            .collect();
        Ok(json!({"watches": rows, "problems": problems}))
    }

    /// The new entry, checked as `ghostd check` checks it, and a read-only tool of a
    /// connected app.
    fn checked(&self, name: &str, entry: &Value) -> Result<Watch, String> {
        let w = parse_watch(name, entry, &format!("watch {name}")).map_err(|e| e.to_string())?;
        if let Some(apps) = self.parts.apps.upgrade() {
            let app = apps
                .app(&w.app)
                .filter(|a| !a.is_own())
                .ok_or_else(|| format!("{} is not a connected app", w.app))?;
            if !app.reads().iter().any(|t| t.name == w.read) {
                return Err(format!(
                    "{} is not one of {}'s read-only tools; a watch only reads",
                    w.read, w.app
                ));
            }
        }
        Ok(w)
    }

    pub(crate) fn do_watch_manage(&self, p: WatchManage) -> Result<Value, String> {
        let slug = self.agent_only(p.caller.clone())?;
        let me = format!("{AGENT_SOURCE}{slug}");
        let action = p.action.trim().to_lowercase();
        let name = p.name.trim().to_lowercase();
        let (text, body) = self.read_watches()?;
        let Value::Object(entries) = &body else {
            return Err(format!(
                "{WATCHES_FILE} is not an object keyed by watch name; the operator fixes it"
            ));
        };
        let old = entries.get(&name).cloned();
        let given = p.watch.clone().unwrap_or_default();
        if let Some(k) = NOT_AN_AGENTS.iter().find(|k| given.contains_key(**k)) {
            return Err(format!("{k} is not something an agent's watch sets"));
        }
        // Who may touch an existing one: who made it, or Ghost for any agent's.
        let may = |entry: &Value| -> Result<(), String> {
            match entry.get("made_by").and_then(Value::as_str) {
                Some(by) if by == me || (slug == GHOST && by.starts_with(AGENT_SOURCE)) => Ok(()),
                Some(by) => Err(format!(
                    "{name} is {}'s watch; an agent manages only its own",
                    &by[AGENT_SOURCE.len()..]
                )),
                None => Err(format!(
                    "{name} is the operator's own watch; only they change it"
                )),
            }
        };
        let entry: Option<Value> = match action.as_str() {
            "create" => {
                if old.is_some() {
                    return Err(format!("there is already a watch called {name}"));
                }
                if given.is_empty() {
                    return Err("say what to watch: app, read, fields, key and title".into());
                }
                let mut entry = given;
                entry.insert("made_by".into(), json!(me));
                Some(Value::Object(entry))
            }
            "update" | "pause" | "resume" => {
                let old = old.ok_or_else(|| format!("there is no watch called {name}"))?;
                may(&old)?;
                let Value::Object(mut entry) = old else {
                    return Err(format!("{name} is not an object"));
                };
                match action.as_str() {
                    "update" => entry.extend(given),
                    "pause" => {
                        entry.insert("paused".into(), json!(true));
                    }
                    _ => {
                        entry.remove("paused");
                    }
                }
                Some(Value::Object(entry))
            }
            "remove" => {
                let old = old.ok_or_else(|| format!("there is no watch called {name}"))?;
                may(&old)?;
                None
            }
            other => {
                return Err(format!(
                    "action is create, update, pause, resume or remove, not {other}"
                ));
            }
        };
        let w = match &entry {
            Some(e) => Some(self.checked(&name, e)?),
            None => None,
        };
        // The whole file as it would be: refused whole if it has a problem it did not have.
        let new_text =
            edit_keyed(&text, &name, entry).map_err(|e| format!("{WATCHES_FILE}: {e}"))?;
        let new_body: Value = serde_json::from_str(&new_text).map_err(|e| e.to_string())?;
        let (_, before) = parse_watches(&body, WATCHES_FILE);
        let (_, after) = parse_watches(&new_body, WATCHES_FILE);
        if let Some(problem) = after.iter().find(|p| !before.contains(p)) {
            return Err(format!("not written: {problem}"));
        }
        write_folder_file(&self.watches_path()?, &new_text).map_err(|e| e.to_string())?;
        if let Some(refresh) = &self.more().refresh {
            refresh(); // the daemon follows: the watch starts, changes or stops
        }
        let status = match action.as_str() {
            "create" => "created",
            "update" => "updated",
            "pause" => "paused",
            "resume" => "resumed",
            _ => "removed",
        };
        let mut out = w.as_ref().map_or_else(|| json!({"name": name}), watch_view);
        out["status"] = json!(status);
        Ok(out)
    }
}

/// What the operator does to a watch an agent made, from the desktop (ADR 0052): pause it,
/// resume it or remove it. Their own watches are theirs to edit in `watches.json`; this
/// touches only an entry with `made_by`, and only that entry, through the same writer an
/// agent's change goes through (the operator's other entries and their order are kept,
/// and a file that would gain a problem is not written).
pub fn operator_sets_agents_watch(
    watches_path: &std::path::Path,
    name: &str,
    action: &str,
) -> Result<(), String> {
    let text = if watches_path.is_file() {
        std::fs::read_to_string(watches_path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let body: Value = if text.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&text).map_err(|e| format!("{WATCHES_FILE} is not JSON ({e})"))?
    };
    let old = body
        .get(name)
        .cloned()
        .ok_or_else(|| format!("there is no watch called {name}"))?;
    if old.get("made_by").and_then(Value::as_str).is_none() {
        return Err(format!(
            "{name} is your own watch: change it in {WATCHES_FILE}"
        ));
    }
    let entry = match action {
        "pause" | "resume" => {
            let Value::Object(mut entry) = old else {
                return Err(format!("{name} is not an object"));
            };
            if action == "pause" {
                entry.insert("paused".into(), json!(true));
            } else {
                entry.remove("paused");
            }
            Some(Value::Object(entry))
        }
        "remove" => None,
        other => return Err(format!("pause, resume or remove, not {other}")),
    };
    let new_text = edit_keyed(&text, name, entry).map_err(|e| format!("{WATCHES_FILE}: {e}"))?;
    let new_body: Value = serde_json::from_str(&new_text).map_err(|e| e.to_string())?;
    let (_, before) = parse_watches(&body, WATCHES_FILE);
    let (_, after) = parse_watches(&new_body, WATCHES_FILE);
    if let Some(problem) = after.iter().find(|p| !before.contains(p)) {
        return Err(format!("not written: {problem}"));
    }
    write_folder_file(watches_path, &new_text).map_err(|e| e.to_string())
}

#[tool_router(router = watches_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "The watches: what each one polls and how often, what it does with what it finds, who made it, and its last check and item counts.",
        annotations(read_only_hint = true)
    )]
    fn watches_list(&self) -> CallToolResult {
        reply(self.do_watches_list())
    }

    #[tool(
        description = "Create, change, pause, resume or remove a watch: a connected app's read-only tool that Ghost polls with no model and checks by fixed rules, then tells the operator, records, or has an agent act. Making or changing one waits for the operator's yes unless their permissions say otherwise. See ghost_docs_search 'watches' for the declaration."
    )]
    fn watch_manage(&self, Parameters(p): Parameters<WatchManage>) -> CallToolResult {
        reply(self.do_watch_manage(p))
    }
}

#[cfg(test)]
mod operator_tests {
    use super::*;

    #[test]
    fn the_operator_pauses_resumes_and_removes_an_agents_watch_and_not_their_own() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(WATCHES_FILE);
        let decl = |made_by: Option<&str>| {
            let mut w = json!({
                "app": "github", "read": "list_runs", "every": 300,
                "fields": {"id": {"at": "id", "type": "string"}},
                "key": "{id}", "title": "run {id}"
            });
            if let Some(by) = made_by {
                w["made_by"] = json!(by);
            }
            w
        };
        let text = serde_json::to_string_pretty(&json!({
            "mine": decl(None),
            "ledgers": decl(Some("agent:ledger")),
        }))
        .unwrap();
        std::fs::write(&path, &text).unwrap();
        let read =
            || -> Value { serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap() };
        operator_sets_agents_watch(&path, "ledgers", "pause").unwrap();
        assert_eq!(read()["ledgers"]["paused"], json!(true));
        assert_eq!(
            read()["mine"],
            decl(None),
            "the operator's own is untouched"
        );
        operator_sets_agents_watch(&path, "ledgers", "resume").unwrap();
        assert!(read()["ledgers"].get("paused").is_none());
        let e = operator_sets_agents_watch(&path, "mine", "pause").unwrap_err();
        assert!(e.contains("your own watch"), "{e}");
        assert!(operator_sets_agents_watch(&path, "nope", "pause").is_err());
        operator_sets_agents_watch(&path, "ledgers", "remove").unwrap();
        assert!(read().get("ledgers").is_none());
        assert!(read().get("mine").is_some());
    }
}
