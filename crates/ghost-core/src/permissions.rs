//! What the operator blocks, or wants asked first (ADR 0034). Everything else runs.
//!
//! A rule names an app, one of its tools, or a tool with argument patterns:
//!
//! ```text
//! gmail                                   every tool of the app
//! github.delete_*                         a glob over the tool's name
//! github.issue_write(state=closed)        only calls whose arguments match
//! github.*(repo!=site)                    every call that is not about that repository
//! conversations.delete                    one of Ghost's own operations
//! ```
//!
//! Each rule sits in one list, and the most careful list that matches wins: `block`, then
//! `ask`, then `quiet`, then `allow`, then `mode`, the default for a change no rule names
//! (`open` announces it, `ask` asks first, `read-only` blocks it). A read is only ever
//! offered or blocked: `ask` and `quiet` say nothing about it. There is one set of these,
//! the operator's; nothing narrows or widens it per agent (ADR 0035).
//!
//! A pattern is a shell glob, case-insensitive. `arg=pattern` holds when the argument is
//! there and matches (any item, for a list); `arg!=pattern` holds when it is missing or
//! does not match, so a `block` with `!=` also stops a call that leaves the argument out.
//! A rule with argument patterns can only be decided on a call: it never takes a tool
//! away from the model, it refuses the call.
//!
//! The model never reads or writes any of this; nothing an app sends reaches it.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::levels::Autonomy;
use crate::models::py_str;

pub const LISTS: [&str; 4] = ["block", "ask", "quiet", "allow"];

static RULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?P<app>[A-Za-z0-9_*?\-/]+)(?:\.(?P<tool>[A-Za-z0-9_*?\-.]+))?\s*(?:\((?P<args>[^()]*)\))?\s*$")
        .unwrap()
});
static COND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?P<arg>[A-Za-z0-9_\-.]+)\s*(?P<op>!=|=)\s*(?P<value>.*?)\s*$").unwrap()
});

#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
#[error("{0}")]
pub struct PermissionsError(pub String);

/// What a change no rule names does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// It runs, and Ghost says so (announce).
    #[default]
    Open,
    /// Ghost asks first (confirm).
    Ask,
    /// It is blocked.
    ReadOnly,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Open, Mode::Ask, Mode::ReadOnly];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Open => "open",
            Mode::Ask => "ask",
            Mode::ReadOnly => "read-only",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }

    pub fn level(self) -> Autonomy {
        match self {
            Mode::Open => Autonomy::Announce,
            Mode::Ask => Autonomy::Confirm,
            Mode::ReadOnly => Autonomy::Deny,
        }
    }
}

/// `fnmatch.fnmatchcase`: `*`, `?` and `[...]` over the whole string.
pub fn fnmatch(value: &str, pattern: &str) -> bool {
    let s: Vec<char> = value.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut si, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (pattern index after '*', string index it covers from)
    while si < s.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi + 1, si));
                    pi += 1;
                    continue;
                }
                '?' => {
                    si += 1;
                    pi += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next)) = class(&p, pi, s[si]) {
                        if matched {
                            si += 1;
                            pi = next;
                            continue;
                        }
                    } else if s[si] == '[' {
                        si += 1;
                        pi += 1;
                        continue;
                    }
                }
                c => {
                    if c == s[si] {
                        si += 1;
                        pi += 1;
                        continue;
                    }
                }
            }
        }
        match star {
            Some((p_after, s_from)) => {
                pi = p_after;
                si = s_from + 1;
                star = Some((p_after, si));
            }
            None => return false,
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A bracket class at `p[start] == '['`: whether `c` is in it and the index after `]`.
/// None when the bracket never closes (then it is a literal `[`).
fn class(p: &[char], start: usize, c: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = i < p.len() && p[i] == '!';
    if negate {
        i += 1;
    }
    let mut j = i;
    if j < p.len() && p[j] == ']' {
        j += 1; // a leading ']' is literal
    }
    while j < p.len() && p[j] != ']' {
        j += 1;
    }
    if j >= p.len() {
        return None;
    }
    let body = &p[i..j];
    let mut hit = false;
    let mut k = 0;
    while k < body.len() {
        if k + 2 < body.len() && body[k + 1] == '-' {
            if body[k] <= c && c <= body[k + 2] {
                hit = true;
            }
            k += 3;
        } else {
            if body[k] == c {
                hit = true;
            }
            k += 1;
        }
    }
    Some((hit != negate, j + 1))
}

fn glob(value: &str, pattern: &str) -> bool {
    fnmatch(&value.to_lowercase(), &pattern.to_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Condition {
    pub arg: String,
    pub value: String,
    pub negated: bool,
}

impl Condition {
    pub fn holds(&self, args: &Map<String, Value>) -> bool {
        let values: Vec<String> = match args.get(&self.arg) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.iter().map(py_str).collect(),
            Some(other) => vec![py_str(other)],
        };
        let hit = values.iter().any(|v| glob(v, &self.value));
        if self.negated { !hit } else { hit }
    }
}

impl std::fmt::Display for Condition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}{}",
            self.arg,
            if self.negated { "!=" } else { "=" },
            self.value
        )
    }
}

/// One line of a list. `tool` "*" is every tool of the app.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Rule {
    pub app: String,
    pub tool: String,
    pub conditions: Vec<Condition>,
}

impl Rule {
    pub fn app(app: impl Into<String>) -> Self {
        Self {
            app: app.into(),
            tool: "*".into(),
            conditions: Vec::new(),
        }
    }

    pub fn tool(app: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            app: app.into(),
            tool: tool.into(),
            conditions: Vec::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Rule, PermissionsError> {
        let m = RULE.captures(text).ok_or_else(|| {
            PermissionsError(format!(
                "{text:?} is not a rule: write app, app.tool or app.tool(arg=value)"
            ))
        })?;
        let mut conditions = Vec::new();
        for part in m.name("args").map_or("", |a| a.as_str()).split(',') {
            if part.trim().is_empty() {
                continue;
            }
            let c = COND
                .captures(part)
                .filter(|c| !c["value"].is_empty())
                .ok_or_else(|| {
                    PermissionsError(format!(
                        "{text:?}: {:?} is not arg=value or arg!=value",
                        part.trim()
                    ))
                })?;
            conditions.push(Condition {
                arg: c["arg"].to_string(),
                value: c["value"].to_string(),
                negated: &c["op"] == "!=",
            });
        }
        Ok(Rule {
            app: m["app"].to_string(),
            tool: m
                .name("tool")
                .map_or("*".to_string(), |t| t.as_str().to_string()),
            conditions,
        })
    }

    pub fn conditional(&self) -> bool {
        !self.conditions.is_empty()
    }

    /// Whether the rule is about this tool at all, its argument patterns aside.
    pub fn names(&self, source: &str, operation: &str) -> bool {
        glob(source, &self.app) && glob(operation, &self.tool)
    }

    /// `args` None is "no call yet": a rule with argument patterns cannot match.
    pub fn matches(
        &self,
        source: &str,
        operation: &str,
        args: Option<&Map<String, Value>>,
    ) -> bool {
        if !self.names(source, operation) {
            return false;
        }
        if self.conditions.is_empty() {
            return true;
        }
        args.is_some_and(|a| self.conditions.iter().all(|c| c.holds(a)))
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.tool == "*" {
            write!(f, "{}", self.app)?;
        } else {
            write!(f, "{}.{}", self.app, self.tool)?;
        }
        if !self.conditions.is_empty() {
            let inner: Vec<String> = self.conditions.iter().map(ToString::to_string).collect();
            write!(f, "({})", inner.join(", "))?;
        }
        Ok(())
    }
}

pub fn parse_rules(values: &toml::Value, where_: &str) -> Result<Vec<Rule>, PermissionsError> {
    let items = values
        .as_array()
        .filter(|a| a.iter().all(toml::Value::is_str))
        .ok_or_else(|| PermissionsError(format!("{where_} must be a list of rules (strings)")))?;
    items
        .iter()
        .map(|v| {
            Rule::parse(v.as_str().unwrap_or_default())
                .map_err(|e| PermissionsError(format!("{where_}: {}", e.0)))
        })
        .collect()
}

/// Most careful first, the order of the lists (ADR 0034): block, ask, quiet, allow.
const CARE: [Autonomy; 4] = [
    Autonomy::Deny,
    Autonomy::Confirm,
    Autonomy::Quiet,
    Autonomy::Announce,
];

/// The operator's lists, and the mode for what they leave out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Permissions {
    pub mode: Mode,
    pub block: Vec<Rule>,
    pub ask: Vec<Rule>,
    pub quiet: Vec<Rule>,
    pub allow: Vec<Rule>,
}

impl Permissions {
    /// The `[permissions]` table.
    pub fn parse(body: &toml::Table, where_: &str) -> Result<Permissions, PermissionsError> {
        let mut unknown: Vec<&str> = body
            .keys()
            .map(String::as_str)
            .filter(|k| !LISTS.contains(k) && *k != "mode")
            .collect();
        unknown.sort_unstable();
        if !unknown.is_empty() {
            return Err(PermissionsError(format!(
                "{where_} has unknown key(s): {}",
                unknown.join(", ")
            )));
        }
        let mut mode = Mode::Open;
        if let Some(raw) = body.get("mode") {
            let text = raw.as_str().map_or_else(|| raw.to_string(), String::from);
            mode = Mode::parse(&text).ok_or_else(|| {
                let all: Vec<&str> = Mode::ALL.iter().map(|m| m.as_str()).collect();
                PermissionsError(format!(
                    "{where_}.mode: {text:?} is not one of {}",
                    all.join(", ")
                ))
            })?;
        }
        let empty = toml::Value::Array(Vec::new());
        let list = |k: &str| parse_rules(body.get(k).unwrap_or(&empty), &format!("{where_}.{k}"));
        Ok(Permissions {
            mode,
            block: list("block")?,
            ask: list("ask")?,
            quiet: list("quiet")?,
            allow: list("allow")?,
        })
    }

    pub fn list(&self, name: &str) -> &[Rule] {
        match name {
            "block" => &self.block,
            "ask" => &self.ask,
            "quiet" => &self.quiet,
            "allow" => &self.allow,
            _ => &[],
        }
    }

    fn list_mut(&mut self, name: &str) -> &mut Vec<Rule> {
        match name {
            "block" => &mut self.block,
            "ask" => &mut self.ask,
            "quiet" => &mut self.quiet,
            _ => &mut self.allow,
        }
    }

    /// The same lists with `rule` (by its text) moved to `level`, or removed when `level`
    /// is None: what one toggle on the Apps screen does.
    pub fn with_rule(
        &self,
        rule: &Rule,
        level: Option<&str>,
    ) -> Result<Permissions, PermissionsError> {
        let text = rule.to_string();
        let mut out = self.clone();
        for k in LISTS {
            out.list_mut(k).retain(|r| r.to_string() != text);
        }
        if let Some(level) = level {
            if !LISTS.contains(&level) {
                return Err(PermissionsError(format!(
                    "{level:?} is not one of {}",
                    LISTS.join(", ")
                )));
            }
            out.list_mut(level).push(rule.clone());
        }
        Ok(out)
    }

    /// The level for a change: the most careful list that matches, else the mode.
    pub fn resolve(
        &self,
        source: &str,
        operation: &str,
        args: Option<&Map<String, Value>>,
    ) -> Autonomy {
        self.matched(source, operation, args)
            .unwrap_or_else(|| self.mode.level())
    }

    fn matched(
        &self,
        source: &str,
        operation: &str,
        args: Option<&Map<String, Value>>,
    ) -> Option<Autonomy> {
        [&self.block, &self.ask, &self.quiet, &self.allow]
            .into_iter()
            .zip(CARE)
            .find(|(rules, _)| rules.iter().any(|r| r.matches(source, operation, args)))
            .map(|(_, level)| level)
    }

    /// Whether a read (or any call) is refused: only `block` can say so.
    pub fn blocks(&self, source: &str, operation: &str, args: Option<&Map<String, Value>>) -> bool {
        self.block
            .iter()
            .any(|r| r.matches(source, operation, args))
    }

    /// Whether the model is offered the change at all: not when an unconditional `block`
    /// names it, nor in `read-only` when no rule could let a call through.
    pub fn offers_change(&self, source: &str, operation: &str) -> bool {
        if self.blocks(source, operation, None) {
            return false;
        }
        if self.mode != Mode::ReadOnly {
            return true;
        }
        self.ask
            .iter()
            .chain(&self.quiet)
            .chain(&self.allow)
            .any(|r| r.names(source, operation))
    }

    pub fn offers_read(&self, source: &str, operation: &str) -> bool {
        !self.blocks(source, operation, None)
    }

    /// Each list's rules as text, in the lists' order.
    pub fn rules(&self) -> Vec<(&'static str, Vec<String>)> {
        LISTS
            .iter()
            .map(|k| (*k, self.list(k).iter().map(ToString::to_string).collect()))
            .collect()
    }

    pub fn to_toml(&self) -> Vec<String> {
        let mut lines = vec![
            "[permissions]".to_string(),
            format!("mode = \"{}\"", self.mode.as_str()),
        ];
        for k in LISTS {
            let rules = self.list(k);
            if !rules.is_empty() {
                let inner: Vec<String> = rules.iter().map(|r| toml_str(&r.to_string())).collect();
                lines.push(format!("{k} = [{}]", inner.join(", ")));
            }
        }
        lines
    }
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn perms(text: &str) -> Permissions {
        Permissions::parse(&toml::from_str(text).unwrap(), "test").unwrap()
    }

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn fnmatch_is_pythons() {
        assert!(fnmatch("delete_branch", "delete_*"));
        assert!(fnmatch("x", "?"));
        assert!(!fnmatch("xy", "?"));
        assert!(
            fnmatch("abc", "a[b-d]c") && !fnmatch("aec", "a[b-d]c") && fnmatch("aec", "a[!b-d]c")
        );
        assert!(fnmatch("a[c", "a[c"));
        assert!(fnmatch("anything/at/all", "*"));
        assert!(fnmatch("", "*") && !fnmatch("", "?"));
        assert!(fnmatch("github", "github") && !fnmatch("github", "githu"));
    }

    #[test]
    fn an_empty_policy_runs_every_change_and_announces_it() {
        let p = Permissions::default();
        assert_eq!(p.resolve("github", "issue_write", None), Autonomy::Announce);
        assert!(p.offers_change("github", "delete_repository"));
        assert!(p.offers_read("github", "list_issues"));
    }

    #[test]
    fn block_beats_ask_beats_quiet_beats_allow() {
        let p = perms(
            r#"block = ["github.delete_*"]
ask = ["github.*"]
quiet = ["github.add_*", "github.delete_*"]
allow = ["github.*"]"#,
        );
        assert_eq!(p.resolve("github", "delete_branch", None), Autonomy::Deny);
        assert_eq!(
            p.resolve("github", "add_issue_comment", None),
            Autonomy::Confirm
        );
        let q = perms(
            r#"quiet = ["github.add_*"]
allow = ["github.*"]"#,
        );
        assert_eq!(
            q.resolve("github", "add_issue_comment", None),
            Autonomy::Quiet
        );
    }

    #[test]
    fn a_whole_app_is_one_word_and_names_are_case_insensitive() {
        let p = perms(r#"block = ["Gmail"]"#);
        assert_eq!(p.resolve("gmail", "send_email", None), Autonomy::Deny);
        assert!(!p.offers_read("gmail", "search"));
        assert!(p.offers_read("github", "list_issues"));
    }

    #[test]
    fn argument_patterns_decide_on_the_call_and_never_take_the_tool_away() {
        let p = perms(
            r#"ask = ["github.issue_write(state=closed)"]
block = ["github.*(repo!=site)"]"#,
        );
        assert!(p.offers_change("github", "issue_write"));
        assert_eq!(
            p.resolve(
                "github",
                "issue_write",
                Some(&args(json!({"repo": "site", "state": "closed"})))
            ),
            Autonomy::Confirm
        );
        assert_eq!(
            p.resolve(
                "github",
                "issue_write",
                Some(&args(json!({"repo": "site", "state": "open"})))
            ),
            Autonomy::Announce
        );
        // != also stops a call that leaves the argument out
        assert_eq!(
            p.resolve(
                "github",
                "search_issues",
                Some(&args(json!({"query": "bug"})))
            ),
            Autonomy::Deny
        );
        assert!(p.blocks(
            "github",
            "list_issues",
            Some(&args(json!({"repo": "other"})))
        ));
        assert!(!p.blocks(
            "github",
            "list_issues",
            Some(&args(json!({"repo": "site"})))
        ));
    }

    #[test]
    fn a_list_argument_matches_when_any_item_does() {
        let p = perms(r#"ask = ["github.issue_write(labels=wont*)"]"#);
        assert_eq!(
            p.resolve(
                "github",
                "issue_write",
                Some(&args(json!({"labels": ["bug", "wontfix"]})))
            ),
            Autonomy::Confirm
        );
        assert_eq!(
            p.resolve(
                "github",
                "issue_write",
                Some(&args(json!({"labels": ["bug"]})))
            ),
            Autonomy::Announce
        );
    }

    #[test]
    fn lockdown_modes() {
        let ask = perms(
            r#"mode = "ask"
allow = ["github.add_issue_comment"]"#,
        );
        assert_eq!(
            ask.resolve("github", "issue_write", None),
            Autonomy::Confirm
        );
        assert_eq!(
            ask.resolve("github", "add_issue_comment", None),
            Autonomy::Announce
        );
        let ro = perms(
            r#"mode = "read-only"
allow = ["github.issue_write(repo=site)"]"#,
        );
        assert_eq!(
            ro.resolve("github", "add_issue_comment", None),
            Autonomy::Deny
        );
        assert!(!ro.offers_change("github", "add_issue_comment"));
        // a conditional allow keeps the tool offered; the call decides
        assert!(ro.offers_change("github", "issue_write"));
        assert_eq!(
            ro.resolve(
                "github",
                "issue_write",
                Some(&args(json!({"repo": "site"})))
            ),
            Autonomy::Announce
        );
        assert_eq!(
            ro.resolve("github", "issue_write", Some(&args(json!({"repo": "x"})))),
            Autonomy::Deny
        );
        assert!(ro.offers_read("github", "list_issues"));
    }

    #[test]
    fn a_rule_that_does_not_parse_is_an_error() {
        for bad in ["github.(x)", "github.issue_write(state)", "", "a b"] {
            assert!(Rule::parse(bad).is_err(), "{bad:?}");
        }
        let err =
            Permissions::parse(&toml::from_str(r#"mode = "loud""#).unwrap(), "t").unwrap_err();
        assert!(err.0.contains("t.mode") && err.0.contains("open, ask, read-only"));
        let err = Permissions::parse(&toml::from_str(r#"nope = []"#).unwrap(), "t").unwrap_err();
        assert!(err.0.contains("unknown key(s): nope"));
        let err =
            Permissions::parse(&toml::from_str(r#"block = "gmail""#).unwrap(), "t").unwrap_err();
        assert!(err.0.contains("t.block must be a list"));
    }

    #[test]
    fn rules_round_trip_through_their_text() {
        for text in [
            "gmail",
            "github.delete_*",
            "github.issue_write(state=closed, repo!=site)",
        ] {
            assert_eq!(Rule::parse(text).unwrap().to_string(), text);
        }
    }

    #[test]
    fn with_rule_moves_a_rule_between_lists() {
        let p = perms(r#"ask = ["github.issue_write"]"#);
        let rule = Rule::parse("github.issue_write").unwrap();
        let moved = p.with_rule(&rule, Some("block")).unwrap();
        assert!(moved.ask.is_empty());
        assert_eq!(
            moved.rules()[0],
            ("block", vec!["github.issue_write".to_string()])
        );
        assert!(moved.with_rule(&rule, None).unwrap().block.is_empty());
        assert!(p.with_rule(&rule, Some("loud")).is_err());
        assert_eq!(
            perms(
                r#"mode = "ask"
block = ["a", "b.c(d=e)"]"#
            )
            .to_toml(),
            vec![
                "[permissions]",
                "mode = \"ask\"",
                "block = [\"a\", \"b.c(d=e)\"]"
            ]
        );
    }
}
