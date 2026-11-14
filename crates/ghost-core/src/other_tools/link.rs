//! Letting another tool use Ghost (ADR 0068, ADR 0048): the line Ghost adds to that tool,
//! shown before it is added, and written by the tool's own command when it has one, or into
//! its file by a careful merge: every other entry and key kept, in order, the file's mode
//! kept, a backup beside it first, and the new file put in place by one rename.
//!
//! With a `ghostd` that stays put (not an AppImage), the tool runs `ghostd mcp serve
//! --client <name>` and its file holds no token; otherwise it connects to the listener's
//! address with the client's token in a header, as the operator would have pasted it.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::read::{FileState, resolve};
use super::{CLAUDE_JSON_CAP, FILE_CAP, Tool, ToolFound, Where, shown_path};
use crate::apps::edit_json_at;

/// The entry's name in the other tool.
pub const ENTRY: &str = "ghost";
/// Stands for the token in what is shown before the click; the token is made at the
/// click and never shown.
pub const TOKEN_SHOWN: &str = "<token>";

/// How the other tool reaches Ghost.
#[derive(Debug, Clone)]
pub enum Bridge {
    /// It runs this `ghostd` with `mcp serve`.
    Command(String),
    /// It connects to this `host:port` with a token.
    Address(String),
}

/// What letting a tool use Ghost does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// The tool's own command, as argv: run directly, never through a shell.
    Command(Vec<String>),
    /// Ghost adds `entry` to the file at `path`.
    File {
        path: PathBuf,
        shown: String,
        entry: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPlan {
    pub tool: Tool,
    /// The client made for it, as `ghostd mcp add` would.
    pub client: String,
    pub how: How,
    /// It connects by address, with a token written into its config.
    pub by_address: bool,
}

impl LinkPlan {
    /// What the operator is shown before the click, with [`TOKEN_SHOWN`] for the token.
    pub fn shown(&self) -> String {
        match &self.how {
            How::Command(argv) => shell_words(argv),
            How::File { shown, entry, .. } => format!("{shown}:\n{entry}"),
        }
    }

    /// The argv to run, with the token in it.
    pub fn argv(&self, token: &str) -> Option<Vec<String>> {
        match &self.how {
            How::Command(argv) => {
                Some(argv.iter().map(|a| a.replace(TOKEN_SHOWN, token)).collect())
            }
            How::File { .. } => None,
        }
    }
}

/// Words joined for showing, each quoted when a shell would split or read it. Shown only:
/// a command is always run as argv.
pub fn shell_words(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            let plain = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c));
            if plain {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A client's name for the tool: its id, or `<id>-2` and on when that is taken.
pub fn client_name(tool: Tool, clients: &[String]) -> String {
    let base = tool.id();
    if !clients.iter().any(|c| c == base) {
        return base.to_string();
    }
    let mut n = 2;
    while clients.iter().any(|c| *c == format!("{base}-{n}")) {
        n += 1;
    }
    format!("{base}-{n}")
}

fn json_entry(bridge: &Bridge, client: &str) -> Value {
    match bridge {
        Bridge::Command(ghostd) => json!({
            "type": "stdio",
            "command": ghostd,
            "args": ["mcp", "serve", "--client", client],
        }),
        Bridge::Address(addr) => json!({
            "type": "http",
            "url": format!("http://{addr}/mcp"),
            "headers": {"Authorization": format!("Bearer {TOKEN_SHOWN}")},
        }),
    }
}

fn toml_entry(bridge: &Bridge, client: &str) -> toml_edit::Table {
    let mut t = toml_edit::Table::new();
    match bridge {
        Bridge::Command(ghostd) => {
            t["command"] = toml_edit::value(ghostd.as_str());
            let mut args = toml_edit::Array::new();
            for a in ["mcp", "serve", "--client", client] {
                args.push(a);
            }
            t["args"] = toml_edit::value(args);
        }
        Bridge::Address(addr) => {
            t["url"] = toml_edit::value(format!("http://{addr}/mcp"));
            let mut h = toml_edit::InlineTable::new();
            h.insert("Authorization", format!("Bearer {TOKEN_SHOWN}").into());
            t["http_headers"] = toml_edit::value(h);
        }
    }
    t
}

fn toml_shown(bridge: &Bridge, client: &str) -> String {
    let mut doc = toml_edit::Document::new();
    let mut servers = toml_edit::Table::new();
    servers.set_implicit(true);
    servers.insert(ENTRY, toml_edit::Item::Table(toml_entry(bridge, client)));
    doc.insert("mcp_servers", toml_edit::Item::Table(servers));
    doc.to_string().trim_end().to_string()
}

/// What letting `found.tool` use Ghost would do, or why it can't be done.
pub fn plan_link(
    w: &Where,
    found: &ToolFound,
    clients: &[String],
    bridge: &Bridge,
) -> Result<LinkPlan, String> {
    let tool = found.tool;
    if let Some(l) = &found.linked {
        return Err(format!(
            "{} already uses Ghost ({} in {}).",
            tool.title(),
            l.entry,
            l.file
        ));
    }
    let file = w.user_file(tool);
    let shown_file = shown_path(&w.home, &file);
    if let Some(seen) = found.files.iter().find(|f| f.path == file) {
        match &seen.state {
            FileState::Read | FileState::Missing => {}
            FileState::TooLarge(_) => {
                return Err(format!(
                    "{shown_file} is larger than Ghost reads, so Ghost leaves it alone."
                ));
            }
            FileState::Outside => {
                return Err(format!(
                    "{shown_file} links outside your home folder, so Ghost leaves it alone."
                ));
            }
            FileState::Unreadable(_) => {
                return Err(format!(
                    "{shown_file} could not be read, so Ghost leaves it alone."
                ));
            }
        }
    }
    if found.apps.iter().any(|a| {
        a.file == shown_file && a.name.eq_ignore_ascii_case(ENTRY) && a.scope == "every project"
    }) {
        return Err(format!(
            "{} already has an app called ghost that is not Ghost; rename it there first.",
            tool.title()
        ));
    }
    let client = client_name(tool, clients);
    let by_address = matches!(bridge, Bridge::Address(_));
    let binary = found.binary.as_ref().map(|p| p.display().to_string());
    let how = match (tool, bridge, binary) {
        (Tool::ClaudeCode, Bridge::Command(ghostd), Some(bin)) => How::Command(
            [
                bin.as_str(),
                "mcp",
                "add",
                "--scope",
                "user",
                ENTRY,
                "--",
                ghostd,
                "mcp",
                "serve",
                "--client",
                &client,
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        (Tool::ClaudeCode, Bridge::Address(addr), Some(bin)) => How::Command(
            [
                bin.as_str(),
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "http",
                ENTRY,
                &format!("http://{addr}/mcp"),
                "--header",
                &format!("Authorization: Bearer {TOKEN_SHOWN}"),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        (Tool::Codex, Bridge::Command(ghostd), Some(bin)) => How::Command(
            [
                bin.as_str(),
                "mcp",
                "add",
                ENTRY,
                "--",
                ghostd,
                "mcp",
                "serve",
                "--client",
                &client,
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        (Tool::Codex, _, _) => {
            if found
                .files
                .iter()
                .any(|f| f.path == file && f.state == FileState::Read)
                && has_toml_ghost(&file, &w.home)
            {
                return Err(format!(
                    "{shown_file} already has an app called ghost; rename it there first."
                ));
            }
            How::File {
                path: file,
                shown: format!("Adds to {shown_file}"),
                entry: toml_shown(bridge, &client),
            }
        }
        (_, _, _) => {
            if found.files.iter().any(|f| f.path == file && f.commented) {
                return Err(format!(
                    "{shown_file} has comments, which Ghost would lose by writing it; add this \
                     to it by hand:\n{}",
                    serde_json::to_string_pretty(
                        &json!({"mcpServers": {ENTRY: json_entry(bridge, &client)}})
                    )
                    .unwrap_or_default()
                ));
            }
            How::File {
                path: file,
                shown: format!("Adds to {shown_file}"),
                entry: serde_json::to_string_pretty(
                    &json!({"mcpServers": {ENTRY: json_entry(bridge, &client)}}),
                )
                .unwrap_or_default(),
            }
        }
    };
    Ok(LinkPlan {
        tool,
        client,
        how,
        by_address,
    })
}

fn has_toml_ghost(file: &Path, home: &Path) -> bool {
    let mut opened = Vec::new();
    super::read::read_bounded(home, file, FILE_CAP, &mut opened)
        .ok()
        .and_then(|t| t.parse::<toml_edit::Document>().ok())
        .is_some_and(|d| d.get("mcp_servers").and_then(|s| s.get(ENTRY)).is_some())
}

/// Write the plan's entry, with `token` in it, into the tool's file. Returns the backup
/// made beside it, when the file was there.
pub fn write_link(
    w: &Where,
    plan: &LinkPlan,
    bridge: &Bridge,
    token: &str,
    stamp: &str,
) -> Result<Option<PathBuf>, String> {
    let How::File { path, .. } = &plan.how else {
        return Err("this tool is set up by its own command".into());
    };
    edit_file(w, plan.tool, path, stamp, |tool, text| match tool {
        Tool::Codex => {
            let mut doc = parse_toml(text)?;
            let mut entry = toml_entry(bridge, &plan.client);
            if let Some(h) = entry
                .get_mut("http_headers")
                .and_then(|h| h.as_inline_table_mut())
            {
                h.insert("Authorization", format!("Bearer {token}").into());
            }
            let servers = doc
                .entry("mcp_servers")
                .or_insert_with(|| {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    toml_edit::Item::Table(t)
                })
                .as_table_mut()
                .ok_or("its mcp_servers is not a table")?;
            servers.insert(ENTRY, toml_edit::Item::Table(entry));
            Ok(doc.to_string())
        }
        _ => {
            let mut entry = json_entry(bridge, &plan.client);
            if let Some(h) = entry.get_mut("headers") {
                h["Authorization"] = Value::String(format!("Bearer {token}"));
            }
            edit_json_at(text, &["mcpServers", ENTRY], Some(entry))
        }
    })
}

/// Take Ghost's entry back out of the tool's user file, when it is there and names
/// `client` (or names no client). Returns the backup, or None when there was nothing to
/// take out.
pub fn remove_link(
    w: &Where,
    tool: Tool,
    entry: &str,
    stamp: &str,
) -> Result<Option<PathBuf>, String> {
    let path = w.user_file(tool);
    if matches!(resolve(&w.home, &path), Err(FileState::Missing)) {
        return Ok(None);
    }
    edit_file(w, tool, &path, stamp, |tool, text| match tool {
        Tool::Codex => {
            let mut doc = parse_toml(text)?;
            if let Some(s) = doc.get_mut("mcp_servers").and_then(|s| s.as_table_mut()) {
                s.remove(entry);
            }
            Ok(doc.to_string())
        }
        _ => edit_json_at(text, &["mcpServers", entry], None),
    })
}

/// The tool's own command that takes Ghost out, when it has one.
pub fn unlink_argv(tool: Tool, binary: Option<&Path>, entry: &str) -> Option<Vec<String>> {
    let bin = binary?.display().to_string();
    match tool {
        Tool::ClaudeCode => Some(vec![
            bin,
            "mcp".into(),
            "remove".into(),
            "--scope".into(),
            "user".into(),
            entry.into(),
        ]),
        Tool::Codex => Some(vec![bin, "mcp".into(), "remove".into(), entry.into()]),
        Tool::Cursor => None,
    }
}

fn parse_toml(text: &str) -> Result<toml_edit::Document, String> {
    text.parse::<toml_edit::Document>()
        .map_err(|_| "it could not be read as TOML".to_string())
}

/// Read the file, change it, back it up beside itself, and put the new one in place by
/// one rename with the old file's mode. A file that will not read is left as it is.
fn edit_file(
    w: &Where,
    tool: Tool,
    path: &Path,
    stamp: &str,
    change: impl Fn(Tool, &str) -> Result<String, String>,
) -> Result<Option<PathBuf>, String> {
    let shown = shown_path(&w.home, path);
    let cap = if tool == Tool::ClaudeCode {
        CLAUDE_JSON_CAP
    } else {
        FILE_CAP
    };
    let (real, text, mode) = match resolve(&w.home, path) {
        Ok(real) => {
            let mut opened = Vec::new();
            let text = super::read::read_bounded(&w.home, path, cap, &mut opened)
                .map_err(|_| format!("{shown} could not be read, so Ghost left it as it is."))?;
            let mode = std::fs::metadata(&real)
                .map_err(|e| format!("{shown}: {}", e.kind()))?
                .permissions()
                .mode()
                & 0o7777;
            (real, Some(text), mode)
        }
        Err(FileState::Missing) => (path.to_path_buf(), None, 0o600),
        Err(FileState::Outside) => {
            return Err(format!(
                "{shown} links outside your home folder, so Ghost left it as it is."
            ));
        }
        Err(_) => {
            return Err(format!(
                "{shown} could not be read, so Ghost left it as it is."
            ));
        }
    };
    let new = change(tool, text.as_deref().unwrap_or(""))
        .map_err(|e| format!("{shown}: {e}, so Ghost left it as it is."))?;
    let dir = real
        .parent()
        .ok_or_else(|| format!("{shown} has no folder"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("{shown}: {}", e.kind()))?;
    let name = real
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config")
        .to_string();
    let backup = match &text {
        Some(old) => {
            // Never over an earlier backup: one made the same second gets a number.
            let mut b = dir.join(format!("{name}.ghost-backup-{stamp}"));
            let mut n = 2;
            while std::fs::symlink_metadata(&b).is_ok() {
                b = dir.join(format!("{name}.ghost-backup-{stamp}-{n}"));
                n += 1;
            }
            write_new(&b, old.as_bytes(), mode).map_err(|e| format!("{shown}: the backup: {e}"))?;
            Some(b)
        }
        None => None,
    };
    let tmp = dir.join(format!(".{name}.ghost-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    write_new(&tmp, new.as_bytes(), mode).map_err(|e| format!("{shown}: {e}"))?;
    std::fs::rename(&tmp, &real).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{shown}: {}", e.kind())
    })?;
    Ok(backup)
}

fn write_new(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(mode))?;
    f.write_all(bytes)?;
    f.sync_all()
}
