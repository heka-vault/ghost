//! The AI tools already on this computer (ADR 0068): what Claude Code, Cursor and Codex
//! have connected, Ghost offered to them, and the model connections that need no
//! credential lifting.
//!
//! Detection is quiet, local and read-only: a fixed list of files ([`files_for`]), read by
//! one function that refuses a sign-in file and a link out of the home folder
//! ([`read::read_bounded`]), and nothing sent anywhere. Everything read is someone else's
//! text: bounded, redacted, shown as data, and never handed to a model. What is found is
//! names and kinds; a secret in an entry is moved into the plan for Ghost's vault and
//! never shown ([`FoundApp::planned`]).
//!
//! Nothing here writes Ghost's own files. Connecting a found app goes through the same
//! path as the desktop's "Add your own" (`apps::edit_apps` and the vault, in the control
//! API), and letting a tool use Ghost makes a client through `ghost_app::clients` and
//! writes that tool's own file through [`link`], after a backup beside it. Every one of
//! those is the operator's own click, or `ghostd setup … --yes`.

pub mod entries;
pub mod link;
pub mod models;
pub mod read;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use zeroize::Zeroizing;

use crate::apps::{AppSpec, json_entries_at};
pub use entries::{Shown, Skip};
pub use read::{FileState, is_credential, shown_path};

/// Ghost reads at most this much of a tool's `mcp.json` or `config.toml`.
pub const FILE_CAP: u64 = 1024 * 1024;
/// `~/.claude.json` keeps Claude Code's history beside its apps: a larger bound.
pub const CLAUDE_JSON_CAP: u64 = 16 * 1024 * 1024;
/// Project folders whose `.mcp.json` is read, from the ones `~/.claude.json` names.
pub const PROJECTS_MAX: usize = 32;
/// Entries read from one file.
pub const ENTRIES_MAX: usize = 100;
/// Characters of another tool's text shown for one value.
pub const SHOWN_MAX: usize = 300;

/// A tool Ghost knows the files of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    ClaudeCode,
    Cursor,
    Codex,
}

pub const TOOLS: [Tool; 3] = [Tool::ClaudeCode, Tool::Cursor, Tool::Codex];

impl Tool {
    pub fn id(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "claude-code",
            Tool::Cursor => "cursor",
            Tool::Codex => "codex",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "Claude Code",
            Tool::Cursor => "Cursor",
            Tool::Codex => "Codex",
        }
    }

    /// The command the tool installs, when it has one that manages its own apps.
    pub fn binary(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "claude",
            Tool::Cursor => "cursor",
            Tool::Codex => "codex",
        }
    }

    pub fn parse(id: &str) -> Option<Tool> {
        TOOLS.into_iter().find(|t| t.id() == id)
    }

    /// Whether the tool's sign-in is a subscription of its own that Ghost may not use.
    pub fn has_own_sign_in(self) -> bool {
        matches!(self, Tool::ClaudeCode | Tool::Codex)
    }
}

/// Where this computer's tools keep their files.
#[derive(Debug, Clone)]
pub struct Where {
    pub home: PathBuf,
    /// `CODEX_HOME`, when the environment Ghost was started in sets it.
    pub codex_home: Option<PathBuf>,
    /// The commands found on the PATH (`claude`, `cursor`, `codex`).
    pub binaries: IndexMap<Tool, PathBuf>,
}

impl Where {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            codex_home: None,
            binaries: IndexMap::new(),
        }
    }

    pub fn codex_dir(&self) -> PathBuf {
        self.codex_home
            .clone()
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| self.home.join(".codex"))
    }

    /// The user-level file each tool keeps its apps in: the one Ghost writes when the
    /// operator lets that tool use Ghost and its own command is not there.
    pub fn user_file(&self, tool: Tool) -> PathBuf {
        match tool {
            Tool::ClaudeCode => self.home.join(".claude.json"),
            Tool::Cursor => self.home.join(".cursor").join("mcp.json"),
            Tool::Codex => self.codex_dir().join("config.toml"),
        }
    }

    /// Whether the tool is set up here: its folder or its command (a stat, never a read).
    pub fn installed(&self, tool: Tool) -> bool {
        self.binaries.contains_key(&tool)
            || match tool {
                Tool::ClaudeCode => {
                    self.home.join(".claude.json").is_file() || self.home.join(".claude").is_dir()
                }
                Tool::Cursor => self.home.join(".cursor").is_dir(),
                Tool::Codex => self.codex_dir().is_dir(),
            }
    }
}

/// The fixed list of files read for `tool`, before the project files `~/.claude.json`
/// names. Never a sign-in file: `files_never_include_a_sign_in` holds that.
pub fn files_for(w: &Where, tool: Tool) -> Vec<PathBuf> {
    vec![w.user_file(tool)]
}

/// A file read, as shown: `~/.cursor/mcp.json`, and what came of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSeen {
    pub path: PathBuf,
    pub shown: String,
    pub state: FileState,
    /// Lines about entries that were left out, or about the file.
    pub problems: Vec<String>,
    /// The file has comments or trailing commas, which its tool allows: Ghost reads it,
    /// and does not write to it.
    pub commented: bool,
}

impl FileSeen {
    /// The line said when it could not be read, or None.
    pub fn problem_line(&self, tool: Tool) -> Option<String> {
        let name = &self.shown;
        match &self.state {
            FileState::Read | FileState::Missing => None,
            FileState::TooLarge(n) => Some(format!(
                "{name} is {:.1} MB, larger than Ghost reads, so the apps {} lists there are \
                 not shown.",
                *n as f64 / 1e6,
                tool.title()
            )),
            FileState::Outside => Some(format!(
                "{name} links to a place outside your home folder, so Ghost did not follow it."
            )),
            FileState::Unreadable(why) => Some(format!("{name} {why}.")),
        }
    }
}

/// Ghost already in a tool's file: the entry, and the client it names when it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    pub entry: String,
    pub client: Option<String>,
    pub file: String,
}

/// An app another tool uses, as Ghost would connect it.
pub struct FoundApp {
    pub tool: Tool,
    /// Unique within the tool: the file and the entry's name.
    pub key: String,
    /// The entry's name in the other tool (bounded).
    pub name: String,
    /// Where in the tool: "every project", or "~/code/site only".
    pub scope: String,
    pub file: String,
    /// The name it would have in Ghost.
    pub suggested: String,
    /// What it is: an address with no query or user-info, or the exact command.
    pub shown: Shown,
    /// It signs in through Ghost's own sign-in.
    pub sign_in: bool,
    /// What would go into Ghost's vault, by where it was: "header Authorization". Never a
    /// value.
    pub moved: Vec<String>,
    /// Passwords it refers to (`${GITHUB_TOKEN}`) that Ghost does not have.
    pub needs: Vec<String>,
    pub notes: Vec<String>,
    pub skip: Option<Skip>,
    plan: Option<Planned>,
}

impl FoundApp {
    /// What connecting it writes: the entry for `apps.json` with `${NAME}` in place of each
    /// secret, and the secrets for the vault. None when it is skipped.
    pub fn planned(&self) -> Option<&Planned> {
        self.plan.as_ref().filter(|_| self.skip.is_none())
    }
}

impl std::fmt::Debug for FoundApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoundApp")
            .field("tool", &self.tool)
            .field("key", &self.key)
            .field("suggested", &self.suggested)
            .field("shown", &self.shown)
            .field("skip", &self.skip)
            .finish_non_exhaustive()
    }
}

/// An `apps.json` entry and the passwords it needs kept.
pub struct Planned {
    pub spec: AppSpec,
    pub secrets: IndexMap<String, Zeroizing<String>>,
}

/// One tool, as found.
#[derive(Debug)]
pub struct ToolFound {
    pub tool: Tool,
    pub installed: bool,
    pub binary: Option<PathBuf>,
    pub files: Vec<FileSeen>,
    pub apps: Vec<FoundApp>,
    pub linked: Option<Linked>,
}

/// What detection needs to know of Ghost: its connected apps, the passwords it has (by
/// name), and the ports it listens on for outside tools.
#[derive(Debug, Clone, Default)]
pub struct Ghost {
    pub connected: Vec<AppSpec>,
    pub passwords: BTreeSet<String>,
    pub ports: Vec<u16>,
}

/// Every tool, read now. Every path opened is in `opened`, for the test that no sign-in
/// file ever is.
pub fn detect(w: &Where, ghost: &Ghost, opened: &mut Vec<PathBuf>) -> Vec<ToolFound> {
    let mut taken: Vec<String> = ghost.connected.iter().map(|s| s.name.clone()).collect();
    let mut seen: IndexMap<String, String> = ghost
        .connected
        .iter()
        .map(|s| (entries::identity(s), s.name.clone()))
        .collect();
    let mut found = Vec::new();
    for tool in TOOLS {
        let mut t = ToolFound {
            tool,
            installed: w.installed(tool),
            binary: w.binaries.get(&tool).cloned(),
            files: Vec::new(),
            apps: Vec::new(),
            linked: None,
        };
        let mut raws: Vec<entries::Raw> = Vec::new();
        for path in files_for(w, tool) {
            let cap = if tool == Tool::ClaudeCode {
                CLAUDE_JSON_CAP
            } else {
                FILE_CAP
            };
            let mut seen_file = FileSeen {
                shown: shown_path(&w.home, &path),
                path: path.clone(),
                state: FileState::Read,
                problems: Vec::new(),
                commented: false,
            };
            match read::read_bounded(&w.home, &path, cap, opened) {
                Err(state) => seen_file.state = state,
                Ok(text) => {
                    let mut projects = Vec::new();
                    match tool {
                        Tool::Codex => entries::codex_entries(&text, &mut seen_file, &mut raws),
                        Tool::Cursor => entries::json_entries(
                            tool,
                            &text,
                            &["mcpServers"],
                            "every project",
                            &mut seen_file,
                            &mut raws,
                        ),
                        Tool::ClaudeCode => {
                            entries::json_entries(
                                tool,
                                &text,
                                &["mcpServers"],
                                "every project",
                                &mut seen_file,
                                &mut raws,
                            );
                            projects = claude_projects(&w.home, &text);
                            for p in &projects {
                                let scope = format!("{} only", shown_path(&w.home, p));
                                let key = p.to_string_lossy().to_string();
                                entries::json_entries(
                                    tool,
                                    &text,
                                    &["projects", key.as_str(), "mcpServers"],
                                    &scope,
                                    &mut seen_file,
                                    &mut raws,
                                );
                            }
                        }
                    }
                    t.files.push(seen_file);
                    // Claude Code's project scope: `.mcp.json` in each project it names.
                    for p in projects.into_iter().take(PROJECTS_MAX) {
                        let file = p.join(".mcp.json");
                        let mut pf = FileSeen {
                            shown: shown_path(&w.home, &file),
                            path: file.clone(),
                            state: FileState::Read,
                            problems: Vec::new(),
                            commented: false,
                        };
                        match read::read_bounded(&w.home, &file, FILE_CAP, opened) {
                            Err(FileState::Missing) => continue,
                            Err(state) => pf.state = state,
                            Ok(text) => {
                                let scope = format!("{} only", shown_path(&w.home, &p));
                                entries::json_entries(
                                    tool,
                                    &text,
                                    &["mcpServers"],
                                    &scope,
                                    &mut pf,
                                    &mut raws,
                                );
                            }
                        }
                        t.files.push(pf);
                    }
                    continue;
                }
            }
            t.files.push(seen_file);
        }
        for raw in raws {
            let app = entries::plan(w, ghost, raw, &mut taken, &mut seen);
            match app {
                entries::Planning::Ghost(linked) => {
                    if t.linked.is_none() {
                        t.linked = Some(linked);
                    }
                }
                entries::Planning::App(a) => t.apps.push(*a),
            }
        }
        found.push(t);
    }
    found
}

/// The project folders `~/.claude.json` names, inside the home folder only.
fn claude_projects(home: &Path, text: &str) -> Vec<PathBuf> {
    let Ok(Some(projects)) = json_entries_at(text, &["projects"]) else {
        return Vec::new();
    };
    let home_real = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    projects
        .into_iter()
        .map(|(k, _)| PathBuf::from(k))
        .filter(|p| p.is_absolute())
        .filter(|p| {
            p.canonicalize()
                .map(|r| r.starts_with(&home_real))
                .unwrap_or(false)
        })
        .take(PROJECTS_MAX)
        .collect()
}

/// `s` cut to [`SHOWN_MAX`] characters, and redacted of every password Ghost holds.
pub fn bounded(s: &str) -> String {
    let cut: String = s.chars().take(SHOWN_MAX).collect();
    let cut = if s.chars().count() > SHOWN_MAX {
        format!("{cut}…")
    } else {
        cut
    };
    crate::redact::redact_text(&cut)
}

/// "Cursor and Claude Code are set up on this computer.", or None when none is.
pub fn noticed_line(found: &[ToolFound]) -> Option<String> {
    let names: Vec<&str> = found
        .iter()
        .filter(|t| t.installed)
        .map(|t| t.tool.title())
        .collect();
    let list = match names.as_slice() {
        [] => return None,
        [one] => return Some(format!("{one} is set up on this computer.")),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    };
    Some(format!("{list} are set up on this computer."))
}
