//! A yes or a no that sticks (ADR 0070): the operator decides a change waiting for them and,
//! with the same click, says "and don't ask again for this" or "and never do this". The
//! click becomes one rule in `[permissions]` of the operator's `policy.toml`, written here
//! and nowhere else, and is kept in `permission_choices` so Settings › Permissions can show
//! it in a sentence and take it back out.
//!
//! What is offered is fixed and narrow: one tool of one app, by its exact name, never a
//! glob, a whole app or an argument pattern. "Don't ask again" is `allow` and "never" is
//! `block`; no level is added and `mode` is never touched. The words come from the tool's
//! name and the app's key (`scope_words`), never from anything the app says about itself.
//!
//! Who can set one: only the operator, by a click in their own desktop session
//! (`Surface::Desktop`, the evidence `classifier: ui` a Confirm already is). Not the phone
//! yet, not the voice, not a tool of Ghost's own app, an outside client, a run with nobody
//! there, an app's text or a model: none of them has a path here, and the closed-door tests
//! (`ghost_app::wide_tests`, `permission_choices::tests`) hold that line.
//!
//! The writer (`add_rule`, `remove_rule`) edits the file as text through `toml_edit`, so
//! every byte outside the one array it touches stays as the operator left it: comments,
//! order, spacing, other tables. It refuses a file that does not read, refuses an edit
//! whose result reads as anything but the same permissions with that one rule more (or
//! less), backs the file up before its first write, and writes atomically (a temporary
//! file beside it, synced, renamed over it, with the original's mode).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::levels::Autonomy;
use crate::models::Proposal;
use crate::permissions::{Permissions, Rule};
use crate::policy::AutonomyPolicy;
use crate::store::{Store, StoreError};

/// The file beside `policy.toml` that keeps it as it was before the first choice wrote it.
pub const BACKUP_SUFFIX: &str = ".before-choices";

/// What a click makes stick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Choice {
    /// "Yes, and don't ask again": `allow`, runs and Ghost says so.
    Allow,
    /// "No, and never": `block`, not offered and refused.
    Block,
}

impl Choice {
    /// The list in `[permissions]` it writes to.
    pub fn list(self) -> &'static str {
        match self {
            Choice::Allow => "allow",
            Choice::Block => "block",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Choice::Allow),
            "block" => Some(Choice::Block),
            _ => None,
        }
    }
}

/// Where the operator's click came from. One variant: a rule is a standing grant, bigger
/// than one yes, and only the desktop sets it for now (ADR 0070). Adding the phone means
/// adding a variant here, which is the decision, in review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A click in the operator's own desktop session (`classifier: ui`, ADR 0013).
    Desktop,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Desktop => "ui",
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ChoiceError {
    /// Safe to show: what to do instead.
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Io(String),
}

impl From<StoreError> for ChoiceError {
    fn from(e: StoreError) -> Self {
        ChoiceError::Io(e.to_string())
    }
}

/// What the card may offer for one waiting change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The rule a click would write (`github.issue_write`).
    pub rule: String,
    /// The tool and app in plain words: `“issue write” in github`.
    pub scope: String,
    /// Whether "don't ask again" would take effect: false when a more careful rule of the
    /// operator's would still ask.
    pub allow: bool,
    /// Said instead of the allow choice when it would not take effect.
    pub held: Option<String>,
}

/// What `add_rule` did to the file, so `remove_rule` can put it back byte for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Written {
    /// The rule was there already: nothing was written.
    pub already: bool,
    /// The list's key was not in the file and the writer added it.
    pub created_key: bool,
    /// `[permissions]` was not in the file and the writer added it.
    pub created_table: bool,
}

/// One rule the operator set from a decision, as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceRow {
    pub id: i64,
    pub rule: String,
    pub choice: Choice,
    pub scope: String,
    pub set_at: DateTime<Utc>,
    pub proposal_id: String,
    /// `ui`: the surface it was set from.
    pub by: String,
    pub created_key: bool,
    pub created_table: bool,
}

impl ChoiceRow {
    /// The rule in a sentence, in the register the desktop shows levels in.
    pub fn sentence(&self) -> String {
        sentence(self.choice, &self.scope)
    }
}

pub fn sentence(choice: Choice, scope: &str) -> String {
    match choice {
        Choice::Allow => format!("{scope} runs without asking"),
        Choice::Block => format!("{scope} never runs"),
    }
}

/// Said on the card when "don't ask again" would not take effect.
pub const HELD: &str = "Your rules ask for this every time: change that in Permissions.";

static APP_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_\-/]+$").unwrap());
static TOOL_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_\-.]+$").unwrap());
static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());
static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9]+").unwrap());

/// Longest the tool's words get on the card.
const MAX_TOOL_WORDS: usize = 6;
const MAX_TOOL_CHARS: usize = 40;

/// The one rule a click on this change writes: exactly this tool of this app. None when the
/// name holds a character a rule would read as a pattern, or no rule can name it.
pub fn rule_for(source: &str, operation: &str) -> Option<Rule> {
    if !APP_NAME.is_match(source) || !TOOL_NAME.is_match(operation) {
        return None;
    }
    let rule = Rule::tool(source, operation);
    // The text must read back as the same rule, or the file would say something else.
    (Rule::parse(&rule.to_string()).ok()? == rule).then_some(rule)
}

/// The tool and app in a few plain words: `“issue write” in github`. From the tool's name
/// and the app's key only (ADR 0017's words), never the app's description or title.
pub fn scope_words(source: &str, operation: &str) -> String {
    let snake = CAMEL.replace_all(operation, "${1}_${2}").to_lowercase();
    let mut tool = String::new();
    for w in WORDS.find_iter(&snake).take(MAX_TOOL_WORDS) {
        if tool.chars().count() + w.as_str().len() + 1 > MAX_TOOL_CHARS {
            break;
        }
        if !tool.is_empty() {
            tool.push(' ');
        }
        tool.push_str(w.as_str());
    }
    if tool.is_empty() {
        tool = "this tool".into();
    }
    let app = if source == crate::apps::GHOST_APP {
        "Ghost".to_string()
    } else {
        source.chars().take(MAX_TOOL_CHARS).collect()
    };
    format!("“{tool}” in {app}")
}

/// The permissions with `rule` added to `choice`'s list, nothing moved or removed: what
/// the writer's file will read as.
fn plus(perms: &Permissions, choice: Choice, rule: &Rule) -> Permissions {
    let mut out = perms.clone();
    match choice {
        Choice::Allow => out.allow.push(rule.clone()),
        Choice::Block => out.block.push(rule.clone()),
    }
    out
}

/// What the card may offer for this change under these permissions. None when no rule can
/// name its tool. `args` are the call's (the gate's `args_of`), so a careful rule with
/// argument patterns that catches this call is seen.
pub fn offer(
    perms: &Permissions,
    source: &str,
    operation: &str,
    args: &Map<String, Value>,
) -> Option<Offer> {
    let rule = rule_for(source, operation)?;
    let after = plus(perms, Choice::Allow, &rule).resolve(source, operation, Some(args));
    let allow = after != Autonomy::Confirm && after != Autonomy::Deny;
    Some(Offer {
        rule: rule.to_string(),
        scope: scope_words(source, operation),
        allow,
        held: (!allow).then(|| HELD.to_string()),
    })
}

// --- the writer ------------------------------------------------------------------------------

/// One writer at a time in this process: read, edit and rename are one step.
static WRITING: Mutex<()> = Mutex::new(());

fn refused(s: impl Into<String>) -> ChoiceError {
    ChoiceError::Refused(s.into())
}

fn io(path: &Path, e: impl std::fmt::Display) -> ChoiceError {
    ChoiceError::Io(format!("{}: {e}", path.display()))
}

/// The file's text, "" when it is not there yet.
fn read(path: &Path) -> Result<(String, bool), ChoiceError> {
    match fs::read_to_string(path) {
        Ok(t) => Ok((t, true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((String::new(), false)),
        Err(e) => Err(io(path, e)),
    }
}

fn parse(text: &str, path: &Path) -> Result<(AutonomyPolicy, toml_edit::Document), ChoiceError> {
    let policy = AutonomyPolicy::parse(text, &path.display().to_string()).map_err(|e| {
        refused(format!(
            "Your permissions file doesn’t read ({e}), so Ghost didn’t change it. Fix it in Settings › Permissions."
        ))
    })?;
    let doc = text.parse::<toml_edit::Document>().map_err(|e| {
        refused(format!(
            "Your permissions file doesn’t read ({e}), so Ghost didn’t change it."
        ))
    })?;
    Ok((policy, doc))
}

/// The edit must read as exactly the permissions expected, and nothing else of the file may
/// read differently.
fn check(
    before: &AutonomyPolicy,
    text: &str,
    expected: &Permissions,
    path: &Path,
) -> Result<(), ChoiceError> {
    let after = AutonomyPolicy::parse(text, &path.display().to_string()).map_err(|e| {
        refused(format!(
            "Ghost’s edit would not read ({e}); nothing changed."
        ))
    })?;
    if &after.permissions != expected
        || after.pending_ttl != before.pending_ttl
        || after.routes != before.routes
    {
        return Err(refused(
            "Your permissions file uses a layout Ghost can’t add to without changing other rules, so nothing changed. Open Settings › Permissions to update it.",
        ));
    }
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "policy.toml".into());
    path.with_file_name(format!("{name}{BACKUP_SUFFIX}"))
}

/// Keep the file as it was before the first write, once.
fn back_up(path: &Path, existed: bool) -> Result<(), ChoiceError> {
    let backup = backup_path(path);
    if !existed || backup.exists() {
        return Ok(());
    }
    fs::copy(path, &backup).map_err(|e| io(&backup, e))?;
    Ok(())
}

/// Write `text` over `path` all at once: a temporary file beside it, synced, renamed.
fn replace(path: &Path, text: &str) -> Result<(), ChoiceError> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    }
    let mode = fs::metadata(path).ok().map(|m| m.permissions());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp).map_err(|e| io(&tmp, e))?;
        f.write_all(text.as_bytes()).map_err(|e| io(&tmp, e))?;
        f.sync_all().map_err(|e| io(&tmp, e))?;
        if let Some(mode) = &mode {
            fs::set_permissions(&tmp, mode.clone()).map_err(|e| io(&tmp, e))?;
        }
        fs::rename(&tmp, path).map_err(|e| io(path, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn permissions_table(
    doc: &mut toml_edit::Document,
    create: bool,
) -> Result<Option<(&mut toml_edit::Table, bool)>, ChoiceError> {
    let created = !doc.contains_key("permissions");
    if created {
        if !create {
            return Ok(None);
        }
        doc.insert(
            "permissions",
            toml_edit::Item::Table(toml_edit::Table::new()),
        );
    }
    match doc.get_mut("permissions").and_then(|i| i.as_table_mut()) {
        Some(t) => Ok(Some((t, created))),
        None => Err(refused(
            "[permissions] in your file isn’t a plain table, so Ghost didn’t change it. Edit it in Settings › Permissions.",
        )),
    }
}

fn index_of(array: &toml_edit::Array, rule: &Rule) -> Option<usize> {
    let mut found = None;
    for (i, v) in array.iter().enumerate() {
        if let Some(s) = v.as_str()
            && Rule::parse(s).ok().as_ref() == Some(rule)
        {
            found = Some(i);
        }
    }
    found
}

/// Add `rule` to `choice`'s list in the file at `path`. A rule already in that list is not
/// added twice (`already`). Everything outside the one array stays byte for byte.
pub fn add_rule(path: &Path, choice: Choice, rule: &Rule) -> Result<Written, ChoiceError> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let (text, existed) = read(path)?;
    let (before, mut doc) = parse(&text, path)?;
    if before.permissions.list(choice.list()).contains(rule) {
        return Ok(Written {
            already: true,
            ..Default::default()
        });
    }
    let (table, created_table) = permissions_table(&mut doc, true)?.expect("created");
    let created_key = !table.contains_key(choice.list());
    if created_key {
        table.insert(
            choice.list(),
            toml_edit::value(toml_edit::Array::from_iter([rule.to_string()])),
        );
    } else {
        let array = table
            .get_mut(choice.list())
            .and_then(|i| i.as_array_mut())
            .ok_or_else(|| {
                refused(format!(
                    "{} in your [permissions] isn’t a list, so Ghost didn’t change it.",
                    choice.list()
                ))
            })?;
        let mut value = toml_edit::Value::from(rule.to_string());
        // The same spacing as the item before it: one per line stays one per line.
        if let Some(last) = array.iter().last() {
            let prefix = last
                .decor()
                .prefix()
                .and_then(|p| p.as_str())
                .filter(|p| p.contains('\n'))
                .unwrap_or(" ")
                .to_string();
            let suffix = last
                .decor()
                .suffix()
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            value.decor_mut().set_prefix(prefix);
            value.decor_mut().set_suffix(suffix);
            array.push_formatted(value);
        } else {
            array.push(rule.to_string());
        }
    }
    let mut out = doc.to_string();
    if created_table && !out.ends_with('\n') {
        out.push('\n');
    }
    check(
        &before,
        &out,
        &plus(&before.permissions, choice, rule),
        path,
    )?;
    back_up(path, existed)?;
    replace(path, &out)?;
    Ok(Written {
        already: false,
        created_key,
        created_table,
    })
}

/// Take `rule` out of `choice`'s list (its last copy). The key and the table go too when the
/// writer added them and nothing else is in them, so a rule that was the last thing added
/// leaves the file byte for byte as it was. False when it was not there (taken out by hand).
pub fn remove_rule(
    path: &Path,
    choice: Choice,
    rule: &Rule,
    written: Written,
) -> Result<bool, ChoiceError> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let (text, existed) = read(path)?;
    if !existed {
        return Ok(false);
    }
    let (before, mut doc) = parse(&text, path)?;
    let Some((table, _)) = permissions_table(&mut doc, false)? else {
        return Ok(false);
    };
    let Some(array) = table.get_mut(choice.list()).and_then(|i| i.as_array_mut()) else {
        return Ok(false);
    };
    let Some(at) = index_of(array, rule) else {
        return Ok(false);
    };
    array.remove(at);
    let emptied = array.is_empty();
    if emptied && written.created_key {
        table.remove(choice.list());
    }
    let table_empty = table.is_empty();
    if table_empty && written.created_table {
        doc.remove("permissions");
    }
    let mut out = doc.to_string();
    if written.created_table && table_empty && out.trim().is_empty() {
        out.clear();
    }
    let mut expected = before.permissions.clone();
    let list = match choice {
        Choice::Allow => &mut expected.allow,
        Choice::Block => &mut expected.block,
    };
    if let Some(i) = list.iter().rposition(|r| r == rule) {
        list.remove(i);
    }
    check(&before, &out, &expected, path)?;
    back_up(path, existed)?;
    replace(path, &out)?;
    Ok(true)
}

// --- from a decision -------------------------------------------------------------------------

/// The operator decided a waiting change on `surface` and made it stick: write the rule
/// (refusing an allow that a more careful rule would still overrule), keep it, and put it in
/// the audit beside the decision. The caller then confirms or cancels the change as a plain
/// click would, and reloads the gate's permissions (`WriteGate::set_autonomy`).
pub fn set_from_decision(
    store: &Store,
    path: &Path,
    proposal: &Proposal,
    args: &Map<String, Value>,
    choice: Choice,
    surface: Surface,
) -> Result<ChoiceRow, ChoiceError> {
    let source = proposal.ability.as_str();
    let operation = proposal.operation.as_str();
    let rule = rule_for(source, operation).ok_or_else(|| {
        refused("Ghost can’t name this tool in a rule, so it can’t remember this choice. Decide it with Yes or No.")
    })?;
    let (text, _) = read(path)?;
    let (policy, _) = parse(&text, path)?;
    if choice == Choice::Allow
        && offer(&policy.permissions, source, operation, args).is_some_and(|o| !o.allow)
    {
        return Err(refused(HELD));
    }
    let written = add_rule(path, choice, &rule)?;
    let scope = scope_words(source, operation);
    let row = store.add_permission_choice(&NewChoice {
        rule: rule.to_string(),
        choice,
        scope,
        proposal_id: proposal.id.clone(),
        by: surface.as_str().to_string(),
        created_key: written.created_key,
        created_table: written.created_table,
    })?;
    store.event(
        &proposal.id,
        "permission_set",
        Some(&json!({
            "rule": row.rule,
            "list": choice.list(),
            "by": row.by,
            "already": written.already,
        })),
    )?;
    Ok(row)
}

/// What the store keeps for a new choice.
#[derive(Debug, Clone)]
pub struct NewChoice {
    pub rule: String,
    pub choice: Choice,
    pub scope: String,
    pub proposal_id: String,
    pub by: String,
    pub created_key: bool,
    pub created_table: bool,
}

/// The operator took a choice back in Settings › Permissions: the rule leaves the file
/// through the same writer, the row is marked, and the audit says so beside the decision.
/// False when there was no such choice.
pub fn remove_choice(store: &Store, path: &Path, id: i64) -> Result<bool, ChoiceError> {
    let Some(row) = store.permission_choice(id)? else {
        return Ok(false);
    };
    let rule = Rule::parse(&row.rule).map_err(|e| ChoiceError::Io(e.0))?;
    // Another choice may have written the same rule since: the key and table stay then.
    let others = store
        .permission_choices()?
        .into_iter()
        .any(|r| r.id != row.id && r.rule == row.rule && r.choice == row.choice);
    let in_file = if others {
        false
    } else {
        remove_rule(
            path,
            row.choice,
            &rule,
            Written {
                already: false,
                created_key: row.created_key,
                created_table: row.created_table,
            },
        )?
    };
    store.remove_permission_choice(id)?;
    store.event(
        &row.proposal_id,
        "permission_removed",
        Some(&json!({"rule": row.rule, "list": row.choice.list(), "by": "ui", "was_in_file": in_file})),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests;
