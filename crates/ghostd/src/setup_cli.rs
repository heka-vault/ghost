//! `ghostd setup` (ADR 0068): the AI tools already on this computer, from a terminal, for
//! the operator or a coding agent following the setup guide. The same routes the first
//! run uses, through the running daemon, which holds the vault and the apps:
//!
//! - `detect [--json]`: what Claude Code, Cursor and Codex have set up, and a model
//!   connection already here. Nothing changes.
//! - `import <tool> <app>… | --all [--yes]`: connect those apps, each secret into Ghost's
//!   vault.
//! - `link <tool> [--yes]`: let that tool use Ghost.
//! - `unlink <tool> [--yes]`: take Ghost back out of it.
//!
//! Each change prints what it will do and asks y/N unless `--yes`.

use std::io::{BufRead, Write};

use async_trait::async_trait;
use serde_json::{Value, json};

pub const USAGE: &str = "usage: ghostd setup detect [--json] | import <tool> <app>... | import <tool> --all | link <tool> | unlink <tool>   (import, link and unlink ask first unless --yes)";

/// One request to the daemon: its status and JSON body, or why it could not be made.
#[async_trait]
pub trait Api: Send + Sync {
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String>;
}

#[async_trait]
impl Api for ghost_api::client::GhostClient {
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        ghost_api::client::GhostClient::call(self, method, path, body.as_ref())
            .await
            .map(|r| (r.status, r.body))
            .map_err(|e| e.to_string())
    }
}

fn detail(v: &Value) -> String {
    match &v["detail"] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// `detect`'s report, as lines for a terminal.
pub fn report(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match v["line"].as_str() {
        Some(l) => out.push(l.to_string()),
        None => out.push(
            "No AI tool Ghost knows (Claude Code, Cursor, Codex) is set up on this computer."
                .into(),
        ),
    }
    for t in v["tools"].as_array().into_iter().flatten() {
        out.push(String::new());
        out.push(format!("{} ({})", s(&t["title"]), s(&t["id"])));
        for f in t["files"].as_array().into_iter().flatten() {
            if let Some(p) = f["problem"].as_str() {
                out.push(format!("  {p}"));
            }
        }
        match (&t["uses_ghost"], &t["link"]) {
            (u, _) if u.is_object() => out.push(format!(
                "  uses Ghost already{}",
                u["client"]
                    .as_str()
                    .map(|c| format!(" (as {c})"))
                    .unwrap_or_default()
            )),
            (_, l) if l.is_object() => out.push(format!(
                "  can use Ghost: ghostd setup link {}",
                s(&t["id"])
            )),
            _ => {
                if let Some(p) = t["link_problem"].as_str() {
                    out.push(format!("  {}", p.lines().next().unwrap_or(p)));
                }
            }
        }
        let apps = t["apps"].as_array().cloned().unwrap_or_default();
        if apps.is_empty() {
            continue;
        }
        out.push("  apps:".into());
        for a in &apps {
            let what = match s(&a["kind"]) {
                "command" => format!("runs: {}", s(&a["command_line"])),
                _ => s(&a["address"]).to_string(),
            };
            out.push(format!("    {}  {what}", s(&a["name"])));
            out.push(format!("      {}", s(&a["scope"])));
            match a["skip"].as_str() {
                Some(why) => out.push(format!("      not offered: {why}")),
                None => {
                    if a["sign_in"] == true {
                        out.push("      signs in through Ghost".into());
                    }
                    let moved: Vec<&str> = a["moved"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    if !moved.is_empty() {
                        out.push(format!("      into Ghost's vault: {}", moved.join(", ")));
                    }
                    let needs: Vec<&str> = a["needs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    if !needs.is_empty() {
                        out.push(format!("      needs the password {}", needs.join(", ")));
                    }
                    out.push(format!(
                        "      ghostd setup import {} {}",
                        s(&t["id"]),
                        s(&a["name"])
                    ));
                }
            }
        }
    }
    let models = v["models"].as_array().cloned().unwrap_or_default();
    if !models.is_empty() {
        out.push(String::new());
        out.push("A model connection already here:".into());
        for m in &models {
            out.push(format!("  {}: {}", s(&m["title"]), s(&m["how"])));
            let names: Vec<&str> = m["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(8)
                .collect();
            if !names.is_empty() {
                out.push(format!("    {}", names.join(", ")));
            }
        }
    }
    let own: Vec<&str> = v["own_sign_in"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !own.is_empty() {
        out.push(String::new());
        out.push(format!(
            "{}'s sign-in belongs to {}, so Ghost doesn't use it: give Ghost a key, sign in to OpenRouter, or use a model on this computer (Settings › Model).",
            own.join(" and "),
            if own.len() == 1 { "that tool" } else { "those tools" }
        ));
    }
    out
}

fn ask(input: &mut dyn BufRead, out: &mut dyn Write, question: &str) -> bool {
    let _ = write!(out, "{question} [y/N] ");
    let _ = out.flush();
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// `ghostd setup …`. Returns the exit code.
pub async fn setup_cli(
    args: &[String],
    api: &dyn Api,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let yes = args.iter().any(|a| a == "--yes" || a == "-y");
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let fail = |err: &mut dyn Write, m: String| {
        let _ = writeln!(err, "{m}");
        1
    };
    let detected = match api.call("GET", "/other-tools", None).await {
        Ok((200, v)) => v,
        Ok((_, v)) => return fail(err, detail(&v)),
        Err(e) => return fail(err, format!("Ghost is not running ({e}); start it first.")),
    };
    match words.as_slice() {
        ["detect"] => {
            if flag("--json") {
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string_pretty(&detected).unwrap_or_default()
                );
            } else {
                for l in report(&detected) {
                    let _ = writeln!(out, "{l}");
                }
            }
            0
        }
        ["import", tool, names @ ..] => {
            let all = flag("--all");
            if names.is_empty() == !all {
                let _ = writeln!(err, "{USAGE}");
                return 2;
            }
            let Some(t) = detected["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|t| t["id"] == *tool)
            else {
                return fail(
                    err,
                    format!("Nothing found for {tool}; ghostd setup detect lists what is."),
                );
            };
            let apps = t["apps"].as_array().cloned().unwrap_or_default();
            let mut chosen = Vec::new();
            for n in names {
                let hits: Vec<&Value> = apps
                    .iter()
                    .filter(|a| a["name"] == *n || a["key"] == *n)
                    .collect();
                match hits.as_slice() {
                    [one] => chosen.push((*one).clone()),
                    [] => return fail(err, format!("{} has no app called {n}.", s(&t["title"]))),
                    _ => {
                        return fail(
                            err,
                            format!(
                                "{} has more than one {n}; name it by its key (ghostd setup detect --json).",
                                s(&t["title"])
                            ),
                        );
                    }
                }
            }
            if all {
                chosen = apps
                    .iter()
                    .filter(|a| a["skip"].is_null())
                    .cloned()
                    .collect();
            }
            let offered: Vec<&Value> = chosen.iter().filter(|a| a["skip"].is_null()).collect();
            for a in chosen.iter().filter(|a| !a["skip"].is_null()) {
                let _ = writeln!(out, "Skipping {}: {}", s(&a["name"]), s(&a["skip"]));
            }
            if offered.is_empty() {
                let _ = writeln!(out, "Nothing to connect.");
                return 0;
            }
            for a in &offered {
                match s(&a["kind"]) {
                    "command" => {
                        let _ = writeln!(
                            out,
                            "Connect {} as {}. Ghost will run, as written in {}:\n  {}",
                            s(&a["name"]),
                            s(&a["suggested"]),
                            s(&a["file"]),
                            s(&a["command_line"])
                        );
                    }
                    _ => {
                        let _ = writeln!(
                            out,
                            "Connect {} as {}: {}{}",
                            s(&a["name"]),
                            s(&a["suggested"]),
                            s(&a["address"]),
                            if a["sign_in"] == true {
                                " (you sign in through Ghost)"
                            } else {
                                ""
                            }
                        );
                    }
                }
                let moved: Vec<&str> = a["moved"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if !moved.is_empty() {
                    let _ = writeln!(out, "  into Ghost's vault: {}", moved.join(", "));
                }
            }
            if !yes
                && !ask(
                    input,
                    out,
                    &format!(
                        "Connect {}?",
                        if offered.len() == 1 {
                            "it".to_string()
                        } else {
                            format!("these {}", offered.len())
                        }
                    ),
                )
            {
                let _ = writeln!(out, "Nothing changed.");
                return 1;
            }
            let keys: Vec<&Value> = offered.iter().map(|a| &a["key"]).collect();
            match api
                .call(
                    "POST",
                    &format!("/other-tools/{tool}/import"),
                    Some(json!({"apps": keys})),
                )
                .await
            {
                Ok((200, v)) => {
                    for a in v["apps"].as_array().into_iter().flatten() {
                        let _ = writeln!(out, "Connected {}.", s(&a["name"]));
                    }
                    if let Some(u) = v["sign_in_url"].as_str() {
                        let _ = writeln!(out, "Sign in by opening: {u}");
                    }
                    0
                }
                Ok((_, v)) => fail(err, detail(&v)),
                Err(e) => fail(err, e),
            }
        }
        [verb @ ("link" | "unlink"), tool] => {
            let Some(t) = detected["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|t| t["id"] == *tool)
            else {
                return fail(err, format!("{tool} is not set up on this computer."));
            };
            let title = s(&t["title"]).to_string();
            let (question, method) = if *verb == "link" {
                let Some(l) = t["link"].as_object() else {
                    if t["uses_ghost"].is_object() {
                        let _ = writeln!(out, "{title} uses Ghost already.");
                        return 0;
                    }
                    return fail(
                        err,
                        t["link_problem"]
                            .as_str()
                            .unwrap_or("Ghost can't add itself there.")
                            .to_string(),
                    );
                };
                let _ = writeln!(
                    out,
                    "Ghost makes the outside tool {} and {}:\n{}",
                    s(&l["client"]),
                    if l["by"] == "command" {
                        "runs"
                    } else {
                        "writes, after a backup beside it"
                    },
                    s(&l["shown"])
                );
                if l["by_address"] == true {
                    let _ = writeln!(
                        out,
                        "(<token> is the tool's own token, made when you say yes and never shown.)"
                    );
                }
                (format!("Let {title} use Ghost?"), "POST")
            } else {
                if !t["uses_ghost"].is_object() {
                    let _ = writeln!(out, "{title} does not use Ghost.");
                    return 0;
                }
                (format!("Take Ghost out of {title}'s settings?"), "DELETE")
            };
            if !yes && !ask(input, out, &question) {
                let _ = writeln!(out, "Nothing changed.");
                return 1;
            }
            match api
                .call(method, &format!("/other-tools/{tool}/link"), None)
                .await
            {
                Ok((200, v)) => {
                    let _ = writeln!(out, "{}", s(&v["done"]));
                    if let Some(b) = v["backup"].as_str() {
                        let _ = writeln!(out, "The file as it was is in {b}.");
                    }
                    0
                }
                Ok((_, v)) => fail(err, detail(&v)),
                Err(e) => fail(err, e),
            }
        }
        _ => {
            let _ = writeln!(err, "{USAGE}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Fake {
        detected: Value,
        calls: Mutex<Vec<(String, String, Option<Value>)>>,
    }

    #[async_trait]
    impl Api for Fake {
        async fn call(
            &self,
            method: &str,
            path: &str,
            body: Option<Value>,
        ) -> Result<(u16, Value), String> {
            self.calls
                .lock()
                .unwrap()
                .push((method.into(), path.into(), body));
            Ok(match (method, path) {
                ("GET", _) => (200, self.detected.clone()),
                ("POST", p) if p.ends_with("/import") => (
                    200,
                    json!({"apps": [{"name": "github"}], "sign_in_url": null}),
                ),
                _ => (
                    200,
                    json!({"done": "Cursor can use Ghost now; restart it if it is open.", "backup": "~/.cursor/mcp.json.ghost-backup-1"}),
                ),
            })
        }
    }

    fn fake() -> Fake {
        Fake {
            detected: json!({
                "line": "Cursor is set up on this computer.",
                "tools": [{
                    "id": "cursor", "title": "Cursor", "installed": true, "files": [],
                    "uses_ghost": null, "link_problem": null,
                    "link": {"client": "cursor", "by": "file", "by_address": false,
                             "shown": "Adds to ~/.cursor/mcp.json:\n{…}"},
                    "apps": [
                        {"key": "k1", "name": "github", "kind": "address", "address": "https://api.githubcopilot.com/mcp/",
                         "suggested": "github", "scope": "every project", "file": "~/.cursor/mcp.json",
                         "sign_in": false, "moved": ["header Authorization"], "needs": [], "notes": [], "skip": null,
                         "command": [], "command_line": null},
                        {"key": "k2", "name": "tricks", "kind": "command", "address": null, "command": ["sh", "-c", "x; y"],
                         "command_line": "sh -c 'x; y'", "suggested": "tricks", "scope": "every project", "file": "~/.cursor/mcp.json",
                         "sign_in": false, "moved": [], "needs": [], "notes": [], "skip": null},
                        {"key": "k3", "name": "old", "kind": "address", "address": "https://x/sse", "skip": "Connects over SSE.",
                         "suggested": "old", "scope": "every project", "file": "~/.cursor/mcp.json", "sign_in": false,
                         "moved": [], "needs": [], "notes": [], "command": [], "command_line": null}
                    ]
                }],
                "models": [{"provider": "ollama", "title": "Ollama", "how": "Ollama is running on this computer, with 1 model.", "models": ["llama3.2"]}],
                "own_sign_in": []
            }),
            calls: Mutex::default(),
        }
    }

    async fn run(api: &Fake, args: &[&str], typed: &str) -> (i32, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut input = std::io::Cursor::new(typed.as_bytes().to_vec());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = setup_cli(&args, api, &mut input, &mut out, &mut err).await;
        (
            code,
            String::from_utf8(out).unwrap() + &String::from_utf8(err).unwrap(),
        )
    }

    #[tokio::test]
    async fn detect_says_what_is_there_and_changes_nothing() {
        let api = fake();
        let (code, said) = run(&api, &["detect"], "").await;
        assert_eq!(code, 0);
        assert!(said.starts_with("Cursor is set up on this computer."));
        assert!(said.contains("ghostd setup import cursor github"));
        assert!(said.contains("runs: sh -c 'x; y'"));
        assert!(said.contains("not offered: Connects over SSE."));
        assert!(said.contains("can use Ghost: ghostd setup link cursor"));
        assert!(said.contains("Ollama is running"));
        assert_eq!(api.calls.lock().unwrap().len(), 1);
        let (_, json) = run(&api, &["detect", "--json"], "").await;
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), api.detected);
    }

    #[tokio::test]
    async fn import_shows_the_command_and_asks_first() {
        let api = fake();
        let (code, said) = run(&api, &["import", "cursor", "tricks"], "n\n").await;
        assert_eq!(code, 1);
        assert!(said.contains("Ghost will run, as written in ~/.cursor/mcp.json:\n  sh -c 'x; y'"));
        assert!(said.contains("Nothing changed."));
        assert_eq!(api.calls.lock().unwrap().len(), 1, "only the look");
        let (code, said) = run(&api, &["import", "cursor", "--all"], "y\n").await;
        assert_eq!(code, 0, "{said}");
        let calls = api.calls.lock().unwrap();
        let (m, p, body) = calls.last().unwrap();
        assert_eq!(
            (m.as_str(), p.as_str()),
            ("POST", "/other-tools/cursor/import")
        );
        assert_eq!(body.as_ref().unwrap()["apps"], json!(["k1", "k2"]));
    }

    #[tokio::test]
    async fn link_with_yes_does_not_ask() {
        let api = fake();
        let (code, said) = run(&api, &["link", "cursor", "--yes"], "").await;
        assert_eq!(code, 0, "{said}");
        assert!(said.contains("Adds to ~/.cursor/mcp.json"));
        assert!(said.contains("The file as it was is in ~/.cursor/mcp.json.ghost-backup-1."));
        assert!(!said.contains("[y/N]"));
        let (code, _) = run(&api, &["import", "cursor"], "").await;
        assert_eq!(code, 2);
    }
}
