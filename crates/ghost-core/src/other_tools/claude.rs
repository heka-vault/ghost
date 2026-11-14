//! `~/.claude.json`, read for its app list only (ADR 0068).
//!
//! Claude Code keeps its own account (an email, an organisation, an id, perhaps a key) and
//! every project's history in this file beside the apps. Only the two `mcpServers` maps are
//! deserialized, into types of this module that have no other field: serde skips everything
//! else as it reads, the text is dropped before this function returns, and what leaves it is
//! [`ClaudeApps`], which holds the app entries and the project folders they belong to and
//! nothing more. `nothing_but_the_apps_leaves_claude_json` holds that.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use super::read::{FileState, read_bounded, shown_path};
use super::{CLAUDE_JSON_CAP, PROJECTS_MAX};

/// The file as Ghost reads it: the user scope's apps and each project's. No other field.
#[derive(Deserialize)]
struct File {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: Option<IndexMap<String, Value>>,
    #[serde(default)]
    projects: Option<IndexMap<String, Project>>,
}

/// One project as Ghost reads it: its apps. Its history and settings are never read.
#[derive(Deserialize)]
struct Project {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: Option<IndexMap<String, Value>>,
}

/// What leaves the reader: app entries by name, and the project folders inside the home
/// folder with theirs (empty when the project has none here; its `.mcp.json` may).
#[derive(Debug, Default)]
pub struct ClaudeApps {
    pub user: Vec<(String, Value)>,
    pub projects: Vec<(PathBuf, Vec<(String, Value)>)>,
}

/// Read `~/.claude.json` for its apps. A file that is not this shape is a line, never a crash.
pub fn read_claude_json(
    home: &Path,
    path: &Path,
    opened: &mut Vec<PathBuf>,
) -> Result<ClaudeApps, FileState> {
    let text = read_bounded(home, path, CLAUDE_JSON_CAP, opened)?;
    let parsed: Result<File, _> = serde_json::from_str(&text);
    drop(text);
    let file = parsed.map_err(|_| {
        FileState::Unreadable("could not be read as JSON, so Ghost left it as it is".into())
    })?;
    let home_real = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let projects = file
        .projects
        .unwrap_or_default()
        .into_iter()
        .map(|(k, p)| (PathBuf::from(k), p))
        .filter(|(p, _)| p.is_absolute())
        .filter(|(p, _)| {
            p.canonicalize()
                .map(|r| r.starts_with(&home_real))
                .unwrap_or(false)
        })
        .take(PROJECTS_MAX)
        .map(|(p, proj)| {
            (
                p,
                proj.mcp_servers.unwrap_or_default().into_iter().collect(),
            )
        })
        .collect();
    Ok(ClaudeApps {
        user: file.mcp_servers.unwrap_or_default().into_iter().collect(),
        projects,
    })
}

/// A project's folder as the operator sees it: `~/code/site`, its last 60 characters at the
/// most, and redacted like any untrusted text. Shown only; never written to `apps.json`.
pub fn project_label(home: &Path, project: &Path) -> String {
    let shown = shown_path(home, project);
    let n = shown.chars().count();
    let short = if n > 60 {
        format!("…{}", shown.chars().skip(n - 59).collect::<String>())
    } else {
        shown
    };
    crate::redact::redact_text(&short)
}
