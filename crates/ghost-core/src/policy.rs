//! The operator's file: what Ghost may do, how long a change waits for a yes, and how
//! Ghost reaches the operator (ADR 0011, ADR 0030, ADR 0031, ADR 0034).
//!
//! Everything runs unless the operator's permissions say otherwise (`permissions`):
//! `[permissions]` holds the mode and the `block`, `ask`, `quiet` and `allow` lists, one
//! set for every agent (ADR 0035). The model cannot read or write any of it, and nothing
//! an app returns reaches it.
//!
//! The older layout, a table per source with a level per operation (`[conversations]
//! delete = "confirm"`, `[github.targets."acme/app"] close = "deny"`), still reads: each
//! entry becomes a rule in the list its level names.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use tracing::warn;

use crate::levels::Autonomy;
use crate::permissions::{Condition, LISTS, Mode, Permissions, PermissionsError, Rule};

pub const ANY: &str = "*";

/// Sections of earlier layouts that this file no longer carries: the ability packages
/// (ADR 0022), the servers and toolkits (ADR 0024, then apps.json, ADR 0034). Ignored
/// with a warning so an older file still loads.
pub const RETIRED_SECTIONS: [(&str, &str); 3] = [
    ("abilities", "ADR 0022"),
    ("mcp", "ADR 0034: apps.json"),
    ("toolkit", "ADR 0024"),
];

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// The file does not read.
    #[error("{0}")]
    Invalid(String),
    /// The operator's permissions refuse the call, or a source's own limit does.
    #[error("{0}")]
    Refused(String),
}

impl From<PermissionsError> for PolicyError {
    fn from(e: PermissionsError) -> Self {
        PolicyError::Invalid(e.0)
    }
}

fn legacy_list(level: Autonomy) -> &'static str {
    match level {
        Autonomy::Deny => "block",
        Autonomy::Confirm => "ask",
        Autonomy::Quiet => "quiet",
        Autonomy::Announce => "allow",
    }
}

fn legacy_mode(level: Autonomy) -> Mode {
    match level {
        Autonomy::Deny => Mode::ReadOnly,
        Autonomy::Confirm => Mode::Ask,
        Autonomy::Announce | Autonomy::Quiet => Mode::Open,
    }
}

/// File shape:
///
/// ```toml
/// [permissions]
/// mode = "open"                 # open | ask | read-only: a change no rule names
/// block = ["gmail.delete_*"]
/// ask = ["github.issue_write(state=closed)"]
/// quiet = ["github.add_issue_labels"]
/// allow = []                    # exceptions, when mode is ask or read-only
///
/// [defaults]
/// pending_ttl = 3600            # seconds a change Ghost asked about waits (ADR 0030)
///
/// [routes]                      # ADR 0031
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AutonomyPolicy {
    pub permissions: Permissions,
    /// ADR 0030: how long a change at `confirm` waits on the waiting list; None is the
    /// default (`PENDING_TTL_SECS`).
    pub pending_ttl: Option<i64>,
    /// ADR 0031: how Ghost reaches the operator, as the file has it (`routes` parses it);
    /// None is the default policy. Kept here so it lives in the operator's one file.
    pub routes: Option<toml::Table>,
    pub source: String,
}

impl Default for AutonomyPolicy {
    fn default() -> Self {
        Self {
            permissions: Permissions::default(),
            pending_ttl: None,
            routes: None,
            source: "<none>".into(),
        }
    }
}

impl AutonomyPolicy {
    /// A missing file is an empty policy: everything runs and is announced.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PolicyError> {
        let p = path.as_ref();
        if !p.exists() {
            return Ok(Self {
                source: format!("{} (missing)", p.display()),
                ..Default::default()
            });
        }
        let text = std::fs::read_to_string(p)
            .map_err(|e| PolicyError::Invalid(format!("{}: {e}", p.display())))?;
        Self::parse(&text, &p.display().to_string())
    }

    pub fn parse(text: &str, source: &str) -> Result<Self, PolicyError> {
        let raw: toml::Table = toml::from_str(text)
            .map_err(|e| PolicyError::Invalid(format!("{source}: {}", e.message())))?;
        let mut permissions = Permissions::default();
        let mut pending_ttl = None;
        let mut routes = None;
        let mut legacy: Vec<(&str, Vec<Rule>)> = LISTS.iter().map(|k| (*k, Vec::new())).collect();
        let mut legacy_mode_found: Option<Mode> = None;
        for (section, body) in &raw {
            let body = body.as_table().ok_or_else(|| {
                PolicyError::Invalid(format!("{source}: [{section}] must be a table"))
            })?;
            match section.as_str() {
                "permissions" => {
                    permissions = Permissions::parse(body, &format!("{source}: [permissions]"))?;
                }
                "defaults" => {
                    if let Some(a) = body.get("autonomy") {
                        let level = parse_level(a, &format!("{source}: defaults.autonomy"))?;
                        legacy_mode_found = Some(legacy_mode(level));
                        if level == Autonomy::Quiet {
                            push_legacy(&mut legacy, "quiet", Rule::app(ANY));
                        }
                    }
                    if let Some(v) = body.get("pending_ttl") {
                        pending_ttl = Some(seconds(v, &format!("{source}: defaults.pending_ttl"))?);
                    }
                }
                "routes" => routes = Some(body.clone()),
                s if RETIRED_SECTIONS.iter().any(|(name, _)| name == &s) => {
                    let (_, why) = RETIRED_SECTIONS
                        .iter()
                        .find(|(name, _)| name == &s)
                        .unwrap();
                    warn!("{source}: [{s}] is no longer read ({why}); ignored");
                }
                s => legacy_rules(s, body, &mut legacy, source)?,
            }
        }
        if let Some(mode) = legacy_mode_found
            && !raw.contains_key("permissions")
        {
            permissions.mode = mode;
        }
        if legacy.iter().any(|(_, rules)| !rules.is_empty()) {
            for (k, rules) in legacy {
                let mut merged = rules;
                merged.extend(permissions.list(k).iter().cloned());
                match k {
                    "block" => permissions.block = merged,
                    "ask" => permissions.ask = merged,
                    "quiet" => permissions.quiet = merged,
                    _ => permissions.allow = merged,
                }
            }
        }
        Ok(Self {
            permissions,
            pending_ttl,
            routes,
            source: source.to_string(),
        })
    }

    /// The level for this change. `args` are the call's arguments, for the rules with
    /// argument patterns; None decides on the name alone.
    pub fn resolve(
        &self,
        source: &str,
        operation: &str,
        args: Option<&Map<String, Value>>,
    ) -> Autonomy {
        self.permissions.resolve(source, operation, args)
    }

    pub fn with_permissions(&self, permissions: Permissions) -> Self {
        Self {
            permissions,
            source: "<api>".into(),
            ..self.clone()
        }
    }

    /// Render the file the operator owns. Shallow structure, so no TOML writer needed.
    pub fn to_toml(&self) -> String {
        let mut lines = vec![
            "# Ghost's permissions (ADR 0034). Edited through the desktop app or by hand."
                .to_string(),
            "# Everything runs unless a rule below blocks it or asks first.".to_string(),
            String::new(),
        ];
        lines.extend(self.permissions.to_toml());
        lines.push(String::new());
        if let Some(ttl) = self.pending_ttl {
            lines.extend([
                "[defaults]".to_string(),
                format!("pending_ttl = {ttl}"),
                String::new(),
            ]);
        }
        if let Some(routes) = &self.routes {
            lines.extend(routes_lines(routes));
            lines.push(String::new());
        }
        format!("{}\n", lines.join("\n").trim_end())
    }
}

/// `[routes]` as lines, from the table the file held.
pub fn routes_lines(routes: &toml::Table) -> Vec<String> {
    let mut wrapper = toml::Table::new();
    wrapper.insert("routes".into(), toml::Value::Table(routes.clone()));
    toml::to_string(&wrapper)
        .unwrap_or_default()
        .trim_end()
        .lines()
        .map(String::from)
        .collect()
}

static HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[\s*([^\[\]]+?)\s*\]\s*(#.*)?$").unwrap());
static AUTONOMY_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*autonomy\s*=").unwrap());

fn top_of(section: &str) -> String {
    section
        .trim()
        .trim_matches('"')
        .split('.')
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .to_string()
}

/// `text` with its `[permissions]` table replaced by `permissions`, everything else kept
/// as the operator wrote it (comments, `[routes]`, `[defaults]`). The older per-source
/// tables are folded in and dropped, since their rules are in the lists now, and so is
/// `[defaults] autonomy`, which the mode replaces.
pub fn write_permissions(text: &str, permissions: &Permissions) -> Result<String, PolicyError> {
    let keep_top: Vec<&str> = ["defaults", "routes", "permissions"]
        .into_iter()
        .chain(RETIRED_SECTIONS.iter().map(|(n, _)| *n))
        .collect();
    let rendered = permissions.to_toml();
    let mut out: Vec<String> = Vec::new();
    let mut section: Option<String> = None;
    let mut placed = false;
    for line in text.lines() {
        if let Some(m) = HEADER.captures(line) {
            let name = m[1].trim().trim_matches('"').to_string();
            let top = top_of(&name);
            section = Some(name);
            if top == "permissions" {
                if !placed {
                    out.extend(rendered.iter().cloned());
                    placed = true;
                }
                continue;
            }
            if !keep_top.contains(&top.as_str()) {
                continue;
            }
            out.push(line.to_string());
            continue;
        }
        let Some(sec) = &section else {
            out.push(line.to_string());
            continue;
        };
        let top = top_of(sec);
        if top == "permissions" || !keep_top.contains(&top.as_str()) {
            continue;
        }
        if top == "defaults" && AUTONOMY_LINE.is_match(line) {
            continue;
        }
        out.push(line.to_string());
    }
    if !placed {
        // After the opening comments, before the first table.
        let at = out
            .iter()
            .position(|l| HEADER.is_match(l))
            .unwrap_or(out.len());
        let mut block: Vec<String> = rendered.clone();
        block.push(String::new());
        if at > 0 && !out[at - 1].trim().is_empty() {
            block.insert(0, String::new());
        }
        out.splice(at..at, block);
    }
    let result = format!("{}\n", out.join("\n").trim_end());
    AutonomyPolicy::parse(&result, "rendered")?; // never write a file that won't read
    Ok(result)
}

/// `text` with its `[routes]` table (and `[routes.urgency]`) replaced by `rendered`
/// lines (empty takes it out), where it was, or at the end. Everything else stays as the
/// operator wrote it, comments included: the comments just above the next table are that
/// table's, so they stay too.
pub fn write_routes(text: &str, rendered: &[String]) -> Result<String, PolicyError> {
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    let mut placed = false;
    let mut dropped: Vec<String> = Vec::new();

    fn leave(out: &mut Vec<String>, dropped: &mut Vec<String>) {
        let mut tail: Vec<String> = Vec::new();
        while let Some(last) = dropped.last() {
            if last.trim().is_empty() || last.trim_start().starts_with('#') {
                tail.insert(0, dropped.pop().unwrap());
            } else {
                break;
            }
        }
        dropped.clear();
        if out.last().is_some_and(|l| !l.trim().is_empty())
            && tail.first().is_none_or(|t| !t.is_empty())
        {
            tail.insert(0, String::new());
        }
        out.extend(tail);
    }

    for line in text.lines() {
        if let Some(m) = HEADER.captures(line) {
            let top = top_of(&m[1]);
            if top == "routes" {
                if !placed {
                    out.extend(rendered.iter().cloned());
                    placed = true;
                }
                inside = true;
                continue;
            }
            if inside {
                leave(&mut out, &mut dropped);
                inside = false;
            }
        }
        if inside {
            dropped.push(line.to_string());
        } else {
            out.push(line.to_string());
        }
    }
    if inside {
        leave(&mut out, &mut dropped);
    }
    if !placed && !rendered.is_empty() {
        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        out.extend(rendered.iter().cloned());
    }
    let result = format!("{}\n", out.join("\n").trim_end());
    AutonomyPolicy::parse(&result, "rendered")?;
    Ok(result)
}

fn push_legacy(legacy: &mut [(&str, Vec<Rule>)], list: &str, rule: Rule) {
    if let Some((_, rules)) = legacy.iter_mut().find(|(k, _)| *k == list) {
        rules.push(rule);
    }
}

/// `[source] op = "level"` and `[source.targets."t"] op = "level"` as rules.
fn legacy_rules(
    section: &str,
    body: &toml::Table,
    out: &mut [(&str, Vec<Rule>)],
    source: &str,
) -> Result<(), PolicyError> {
    for (key, value) in body {
        if key == "targets" {
            let targets = value.as_table().ok_or_else(|| {
                PolicyError::Invalid(format!("{source}: [{section}.targets] must be a table"))
            })?;
            for (tname, tops) in targets {
                let tops = tops.as_table().ok_or_else(|| {
                    PolicyError::Invalid(format!(
                        "{source}: [{section}.targets.{tname}] must be a table"
                    ))
                })?;
                for (op, lv) in tops {
                    let level =
                        parse_level(lv, &format!("{source}: {section}.targets.{tname}.{op}"))?;
                    let rule = Rule {
                        app: section.to_string(),
                        tool: op.clone(),
                        conditions: vec![Condition {
                            arg: "target".into(),
                            value: tname.clone(),
                            negated: false,
                        }],
                    };
                    push_legacy(out, legacy_list(level), rule);
                }
            }
            continue;
        }
        let level = parse_level(value, &format!("{source}: {section}.{key}"))?;
        push_legacy(out, legacy_list(level), Rule::tool(section, key.clone()));
    }
    Ok(())
}

fn seconds(value: &toml::Value, where_: &str) -> Result<i64, PolicyError> {
    match value.as_integer() {
        Some(n) if (60..=7 * 86400).contains(&n) => Ok(n),
        _ => Err(PolicyError::Invalid(format!(
            "{where_} must be a whole number of seconds from 60 to 604800"
        ))),
    }
}

fn parse_level(value: &toml::Value, where_: &str) -> Result<Autonomy, PolicyError> {
    let text = value
        .as_str()
        .map_or_else(|| value.to_string(), String::from);
    Autonomy::parse(&text).ok_or_else(|| {
        let shown = if value.is_str() {
            format!("'{text}'")
        } else {
            text.clone()
        };
        let all: Vec<&str> = Autonomy::ALL.iter().map(|a| a.as_str()).collect();
        PolicyError::Invalid(format!(
            "{where_}: {shown} is not one of {}",
            all.join(", ")
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    const FILE: &str = r#"
[defaults]
autonomy = "confirm"

[github_issues]
set_labels = "announce"
comment = "quiet"

[github_issues.targets."Acme/Public"]
comment = "confirm"
close = "deny"

[notes]
"*" = "announce"
"#;

    #[test]
    fn the_older_layout_reads_as_rules_and_the_most_careful_wins() {
        let p = AutonomyPolicy::parse(FILE, "<string>").unwrap();
        let public = args(json!({"target": "acme/public"}));
        let other = args(json!({"target": "acme/other"}));
        // an ask for one target beats a quiet for every target
        assert_eq!(
            p.resolve("github_issues", "comment", Some(&public)),
            Autonomy::Confirm
        );
        assert_eq!(
            p.resolve("github_issues", "comment", Some(&other)),
            Autonomy::Quiet
        );
        assert_eq!(
            p.resolve("github_issues", "close", Some(&public)),
            Autonomy::Deny
        );
        assert_eq!(
            p.resolve("github_issues", "set_labels", Some(&other)),
            Autonomy::Announce
        );
        // what no rule names takes the mode, here "ask" from defaults.autonomy
        assert_eq!(
            p.resolve("github_issues", "close", Some(&other)),
            Autonomy::Confirm
        );
        assert_eq!(p.resolve("notes", "append", None), Autonomy::Announce); // the wildcard
    }

    #[test]
    fn a_missing_file_runs_everything_and_a_present_one_reads() {
        let dir = tempfile::tempdir().unwrap();
        let p = AutonomyPolicy::load(dir.path().join("nope.toml")).unwrap();
        assert!(p.source.contains("missing"));
        assert_eq!(p.resolve("x", "y", None), Autonomy::Announce);
        let f = dir.path().join("policy.toml");
        std::fs::write(&f, "[github_issues]\nclose = \"announce\"\n").unwrap();
        assert_eq!(
            AutonomyPolicy::load(&f)
                .unwrap()
                .resolve("github_issues", "close", None),
            Autonomy::Announce
        );
    }

    #[test]
    fn bad_files_name_the_problem() {
        for (text, needle) in [
            ("[github_issues]\nclose = \"yolo\"\n", "yolo"),
            ("[defaults]\nautonomy = 3\n", "defaults.autonomy"),
            ("github_issues = 1\n", "must be a table"),
            ("[a\n", "<string>"),
            ("[defaults]\npending_ttl = 5\n", "60 to 604800"),
        ] {
            let err = AutonomyPolicy::parse(text, "<string>")
                .unwrap_err()
                .to_string();
            assert!(err.contains(needle), "{text:?}: {err}");
        }
    }

    #[test]
    fn the_example_file_parses_open_with_no_rules() {
        let text = include_str!("../../../infra/policy.example.toml");
        let p = AutonomyPolicy::parse(text, "example").unwrap();
        assert_eq!(p.permissions.mode, Mode::Open);
        assert!(p.permissions.rules().iter().all(|(_, r)| r.is_empty()));
        assert_eq!(p.resolve("github", "issue_write", None), Autonomy::Announce);
        assert!(p.routes.is_none() && p.pending_ttl.is_none());
    }

    #[test]
    fn abilities_section_is_ignored() {
        let p = AutonomyPolicy::parse(
            "[abilities]\nenabled = [\"notes\"]\n[notes]\nadd = \"quiet\"\n",
            "<string>",
        )
        .unwrap();
        assert_eq!(p.resolve("notes", "add", None), Autonomy::Quiet);
        assert!(!p.to_toml().contains("[abilities]"));
    }

    #[test]
    fn the_policy_file_reads_permissions_and_round_trips() {
        let text = r#"
[permissions]
mode = "ask"
block = ["gmail.delete_*"]
allow = ["github.add_issue_comment"]

[defaults]
pending_ttl = 600

[routes]
enabled = ["digest", "phone"]
[routes.urgency]
ci = "call"
"#;
        let p = AutonomyPolicy::parse(text, "<string>").unwrap();
        assert_eq!(p.permissions.mode, Mode::Ask);
        assert_eq!(p.resolve("gmail", "delete_draft", None), Autonomy::Deny);
        assert_eq!(p.pending_ttl, Some(600));
        let again = AutonomyPolicy::parse(&p.to_toml(), "again").unwrap();
        assert_eq!(again.permissions, p.permissions);
        assert_eq!(again.pending_ttl, Some(600));
        assert_eq!(again.routes, p.routes);
        assert_eq!(
            again.routes.as_ref().unwrap()["urgency"]["ci"].as_str(),
            Some("call")
        );
    }

    #[test]
    fn the_older_per_source_tables_still_read_as_rules() {
        let p = AutonomyPolicy::parse(
            "[defaults]\nautonomy = \"confirm\"\n\n[conversations]\ndelete = \"deny\"\nrename = \"quiet\"\n\n[github.targets.\"acme/public\"]\nclose = \"deny\"\n",
            "<string>",
        )
        .unwrap();
        assert_eq!(p.permissions.mode, Mode::Ask);
        assert_eq!(p.resolve("conversations", "delete", None), Autonomy::Deny);
        assert_eq!(p.resolve("conversations", "rename", None), Autonomy::Quiet);
        assert_eq!(
            p.resolve("conversations", "create", None),
            Autonomy::Confirm
        );
        assert_eq!(
            p.resolve(
                "github",
                "close",
                Some(&args(json!({"target": "acme/public"})))
            ),
            Autonomy::Deny
        );
        assert_eq!(
            p.resolve(
                "github",
                "close",
                Some(&args(json!({"target": "acme/app"})))
            ),
            Autonomy::Confirm
        );
    }

    #[test]
    fn writing_permissions_keeps_the_rest_of_the_file() {
        let text = "# my notes\n# keep these\n\n[defaults]\nautonomy = \"confirm\"\npending_ttl = 600\n\n[conversations]\ndelete = \"deny\"\n\n[routes]\nenabled = [\"digest\", \"phone\"]  # mine\n";
        let before = AutonomyPolicy::parse(text, "<string>").unwrap();
        let perms_now = before
            .permissions
            .with_rule(&Rule::parse("gmail").unwrap(), Some("block"))
            .unwrap();
        let out = write_permissions(text, &perms_now).unwrap();
        assert!(out.starts_with("# my notes\n# keep these\n"), "{out}");
        assert!(out.contains("enabled = [\"digest\", \"phone\"]  # mine"));
        assert!(!out.contains("[conversations]") && !out.contains("autonomy"));
        let after = AutonomyPolicy::parse(&out, "<string>").unwrap();
        assert_eq!(after.permissions, perms_now);
        assert_eq!(after.pending_ttl, Some(600));
        assert_eq!(
            after.resolve("conversations", "delete", None),
            Autonomy::Deny
        );
        // a second write replaces the table in place
        let again = write_permissions(
            &out,
            &after
                .permissions
                .with_rule(&Rule::parse("gmail").unwrap(), None)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(again.matches("[permissions]").count(), 1);
        assert_eq!(
            AutonomyPolicy::parse(&again, "<string>")
                .unwrap()
                .resolve("gmail", "send", None),
            Autonomy::Confirm
        );
    }

    #[test]
    fn writing_routes_replaces_the_table_in_place_or_appends_it() {
        let text = "# notes\n\n[permissions]\nmode = \"open\"\n\n# how Ghost reaches me\n[routes]\nenabled = [\"digest\"]\n[routes.urgency]\nci = \"call\"\n\n# trailing\n[defaults]\npending_ttl = 600\n";
        let rendered = vec![
            "[routes]".to_string(),
            "enabled = [\"digest\", \"phone\"]".to_string(),
        ];
        let out = write_routes(text, &rendered).unwrap();
        assert!(out.contains("# how Ghost reaches me\n[routes]\nenabled = [\"digest\", \"phone\"]\n\n# trailing\n[defaults]"), "{out}");
        assert!(!out.contains("urgency"));
        let gone = write_routes(&out, &[]).unwrap();
        assert!(
            !gone.contains("[routes]") && gone.contains("[defaults]\npending_ttl = 600"),
            "{gone}"
        );
        let appended = write_routes("[permissions]\nmode = \"open\"\n", &rendered).unwrap();
        assert!(
            appended.ends_with(
                "[permissions]\nmode = \"open\"\n\n[routes]\nenabled = [\"digest\", \"phone\"]\n"
            ),
            "{appended}"
        );
    }
}
