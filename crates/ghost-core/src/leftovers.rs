//! What Ghost leaves on this computer, and taking it off again (#147, ADR 0071).
//!
//! The one list of every file, folder, unit and keyring item Ghost or the libraries it runs
//! write under a person's own folders, typed, sized and grouped into six categories the person
//! chooses among: what keeps running or starts by itself, Ghost's entries in other tools, the
//! speech models' cache, the app's own storage, the person's data, and the password vault with
//! its key. `ghostd uninstall`, Settings › General's "Remove Ghost from this computer" and the
//! README's by-hand list all read it; the tests hold it to what a daemon in a temp home writes
//! and to every well-known folder named in the source.
//!
//! The package's own scripts delete nothing in anyone's home folder (Debian's convention): the
//! person removes this with `ghostd uninstall` before or after `apt remove`. Removal is the
//! person's alone: no tool of Ghost's own app, no outside client, the phone or a run can list,
//! start or choose any of it.
//!
//! Every path removed is one the inventory names, under the person's home folder or their
//! runtime folder, never the home folder itself, never followed through a symbolic link out of
//! them; a link is removed as a link.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::other_tools::{self, Tool, Where};

/// The app's identifier (`tauri.conf.json`): the webview's and the shell's own folders.
pub const APP_ID: &str = "dev.heka.ghost";
/// The app's binary and package name.
pub const APP_NAME: &str = "heka-ghost-desktop";
/// The package to remove when Ghost came from the .deb.
pub const PACKAGE: &str = "heka-ghost-desktop";
/// The echo canceller's user unit (`infra/pipewire/ghost-echo-cancel.service`).
pub const ECHO_UNIT: &str = "ghost-echo-cancel.service";
/// The headless daemon's user unit (`infra/systemd/ghostd.service`, a checkout's).
pub const DAEMON_UNIT: &str = "ghostd.service";
/// The optional GNOME Shell helper (`apps/desktop/gnome-shell/`).
pub const GNOME_EXTENSION: &str = "ghost-desktop@heka.dev";
/// The link the echo canceller's unit is conditioned on: it points at this install's `ghostd`
/// (or the AppImage), so the unit does not start once Ghost is gone.
pub const INSTALLED_LINK: &str = "installed";
/// What a backup of another tool's file is named after (`other_tools::link`).
pub const BACKUP_MARK: &str = ".ghost-backup-";
/// The port outside tools reach Ghost on unless its settings say otherwise
/// (`ghost_app::clients::DEFAULT_LISTEN`).
pub const DEFAULT_PORT: u16 = 47824;

/// The six choices, in the order they are asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Running,
    OtherTools,
    SpeechModels,
    AppStorage,
    Data,
    Vault,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Running,
        Category::OtherTools,
        Category::SpeechModels,
        Category::AppStorage,
        Category::Data,
        Category::Vault,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Category::Running => "running",
            Category::OtherTools => "other-tools",
            Category::SpeechModels => "speech-models",
            Category::AppStorage => "app-storage",
            Category::Data => "data",
            Category::Vault => "vault",
        }
    }

    pub fn parse(id: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.id() == id.trim())
    }

    pub fn title(self) -> &'static str {
        match self {
            Category::Running => "What keeps running or starts by itself",
            Category::OtherTools => "Ghost's entries in your other tools",
            Category::SpeechModels => "The speech models Ghost downloaded",
            Category::AppStorage => "The app's own storage",
            Category::Data => "Your data",
            Category::Vault => "Your passwords",
        }
    }

    /// The plain sentence shown under the title.
    pub fn sentence(self) -> &'static str {
        match self {
            Category::Running => {
                "The echo canceller's service, Ghost's entry in Start at login, and the GNOME \
                 Shell helper if you installed it. They are stopped and removed; your \
                 microphone and speakers go back to what the system chooses."
            }
            Category::OtherTools => {
                "Ghost's entry in Cursor, Codex or Claude Code, the copies Ghost made of their \
                 files before changing them, and the keys those tools use to reach Ghost."
            }
            Category::SpeechModels => {
                "Parakeet and Kokoro, which Ghost downloaded to hear and speak. Ghost can \
                 download them again."
            }
            Category::AppStorage => {
                "The window's own storage and cache, the app's log and choices, and the files \
                 Ghost keeps while it runs."
            }
            Category::Data => {
                "Ghost's folder (settings, apps, rules, agents and their memory, skills, \
                 outputs), the database with every transcript and the audit, kept wake takes, \
                 the Hearing test's recordings and error reports kept here."
            }
            Category::Vault => "The password vault and its key in your login keyring.",
        }
    }

    /// Said before the person chooses, for what cannot be had back.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Category::Data => Some(
                "Your transcripts, agents, memory and settings are deleted and cannot be \
                 recovered. Kept wake takes are recordings of your voice; they go too.",
            ),
            Category::Vault => {
                Some("Every password saved in Ghost is deleted and cannot be recovered.")
            }
            _ => None,
        }
    }

    /// Whether the person's own words and keys are in it: asked last, No by default, and
    /// left by `--keep-data`.
    pub fn is_the_persons(self) -> bool {
        matches!(self, Category::Data | Category::Vault)
    }
}

/// How an item is taken off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "name")]
pub enum Removal {
    /// Its paths are deleted.
    Paths,
    /// A user unit: stopped and disabled, then its paths deleted and systemd reloaded.
    UserUnit(String),
    /// GNOME's own `gnome-extensions uninstall`, or the folder when that is not there.
    GnomeExtension,
    /// Ghost's entry taken out of another tool by its own command or from its file
    /// (`other_tools`), then the backups deleted.
    Unlink(String),
    /// The vault's key in the login keyring.
    KeyringItem,
}

/// One thing Ghost left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub category: Category,
    /// Stable within the inventory: `echo-cancel`, `autostart`, `database`…
    pub key: String,
    /// What it is, in the person's words.
    pub what: String,
    /// The files and folders it deletes (nothing for a keyring item).
    pub paths: Vec<PathBuf>,
    /// Paths inside `paths` that stay (the vault inside the data folder).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keep: Vec<PathBuf>,
    pub removal: Removal,
    /// Bytes on disk now (what `keep` holds not counted).
    pub size: u64,
    /// Whether any of it is there now.
    pub exists: bool,
    /// Something the person does themselves (Claude Code's own command), or a fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where Ghost's folders are on this computer: the person's home, the XDG folders, and
/// Ghost's own where its settings moved them. Built by the daemon from its settings, or by
/// `standard` from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub cache_home: PathBuf,
    pub state_home: PathBuf,
    /// `$XDG_RUNTIME_DIR`, when there is one.
    pub runtime: Option<PathBuf>,
    /// `~/.config/ghost` (`GHOST_CONFIG_DIR`).
    pub ghost_config: PathBuf,
    /// `~/.local/share/ghost` (`GHOST_DATA_DIR`).
    pub ghost_data: PathBuf,
    /// `~/.cache/ghost` (`GHOST_CACHE_DIR`).
    pub ghost_cache: PathBuf,
    /// Files Ghost's settings put outside the folders above (`GHOST_DB_PATH` and the like):
    /// each is the person's data, removed only when it is under their home.
    pub elsewhere: Vec<PathBuf>,
    /// The ports Ghost listens on for outside tools, to find its entries in their files.
    pub ports: Vec<u16>,
}

fn non_empty(v: Option<&String>) -> Option<PathBuf> {
    v.filter(|s| !s.trim().is_empty()).map(PathBuf::from)
}

impl Places {
    /// The defaults for `home`, under what `env` says (`XDG_*`, `GHOST_CONFIG_DIR`,
    /// `GHOST_DATA_DIR`, `GHOST_CACHE_DIR`).
    pub fn standard(home: &Path, env: &BTreeMap<String, String>) -> Self {
        let xdg =
            |name: &str, rel: &str| non_empty(env.get(name)).unwrap_or_else(|| home.join(rel));
        let config_home = xdg("XDG_CONFIG_HOME", ".config");
        let data_home = xdg("XDG_DATA_HOME", ".local/share");
        let cache_home = xdg("XDG_CACHE_HOME", ".cache");
        let state_home = xdg("XDG_STATE_HOME", ".local/state");
        Self {
            ghost_config: non_empty(env.get("GHOST_CONFIG_DIR"))
                .unwrap_or_else(|| home.join(".config/ghost")),
            ghost_data: non_empty(env.get("GHOST_DATA_DIR"))
                .unwrap_or_else(|| home.join(".local/share/ghost")),
            ghost_cache: non_empty(env.get("GHOST_CACHE_DIR"))
                .unwrap_or_else(|| home.join(".cache/ghost")),
            runtime: non_empty(env.get("XDG_RUNTIME_DIR")),
            home: home.to_path_buf(),
            config_home,
            data_home,
            cache_home,
            state_home,
            elsewhere: Vec::new(),
            ports: vec![DEFAULT_PORT],
        }
    }

    /// `$XDG_STATE_HOME/ghost`: the shell's log, its choices and the installed link.
    pub fn ghost_state(&self) -> PathBuf {
        self.state_home.join("ghost")
    }

    /// The link the echo canceller's unit is conditioned on (`%S/ghost/installed`).
    pub fn installed_link(&self) -> PathBuf {
        self.ghost_state().join(INSTALLED_LINK)
    }

    fn user_units(&self) -> PathBuf {
        self.config_home.join("systemd/user")
    }

    /// The echo canceller's files, as `infra/pipewire/install.sh` writes them.
    pub fn echo_cancel_paths(&self) -> Vec<PathBuf> {
        let units = self.user_units();
        vec![
            units.join(ECHO_UNIT),
            units.join("default.target.wants").join(ECHO_UNIT),
            self.ghost_config.join("echo-cancel.conf"),
            self.config_home
                .join("pipewire/pipewire.conf.d/50-ghost-allowed-rates.conf"),
            self.config_home
                .join("wireplumber/wireplumber.conf.d/51-ghost-sink-driver.conf"),
            self.config_home
                .join("pipewire/pipewire.conf.d/90-ghost-echo-cancel.conf"),
            self.installed_link(),
        ]
    }

    /// The vault and its lock.
    pub fn vault_paths(&self) -> Vec<PathBuf> {
        let vault = self.ghost_data.join(crate::passwords::VAULT_FILE);
        let lock = vault.with_file_name(format!("{}.lock", crate::passwords::VAULT_FILE));
        vec![vault, lock]
    }
}

/// What the inventory asks of the computer beyond its files.
pub trait Look {
    /// Whether the vault's key is in the login keyring: None when the keyring cannot say.
    fn keyring_has_key(&self) -> Option<bool>;
}

/// Nothing beyond the files: the keyring is not asked.
pub struct FilesOnly;

impl Look for FilesOnly {
    fn keyring_has_key(&self) -> Option<bool> {
        None
    }
}

/// Bytes on disk under `path`, never following a link.
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum())
        .unwrap_or(0)
}

fn there(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The backups Ghost made beside another tool's file (`<file>.ghost-backup-<stamp>`).
pub fn backups_beside(file: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    let prefix = format!("{name}{BACKUP_MARK}");
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with(&prefix))
                        && e.file_type().is_ok_and(|t| t.is_file())
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn item(category: Category, key: &str, what: &str, paths: Vec<PathBuf>, removal: Removal) -> Item {
    Item {
        category,
        key: key.into(),
        what: what.into(),
        paths,
        keep: Vec::new(),
        removal,
        size: 0,
        exists: false,
        note: None,
    }
}

/// Every item, for these places, whether it is there or not; sizes and presence filled in.
pub fn inventory(p: &Places, look: &dyn Look) -> Vec<Item> {
    let mut items = Vec::new();
    let units = p.user_units();

    // (a) What keeps running or starts by itself.
    items.push(item(
        Category::Running,
        "echo-cancel",
        "The echo canceller's service, its settings and the sound settings it added",
        p.echo_cancel_paths(),
        Removal::UserUnit(ECHO_UNIT.into()),
    ));
    items.push(item(
        Category::Running,
        "autostart",
        "Ghost in Start at login",
        // auto-launch (tauri-plugin-autostart) always writes ~/.config/autostart.
        vec![
            p.home
                .join(".config/autostart")
                .join(format!("{APP_NAME}.desktop")),
        ],
        Removal::Paths,
    ));
    items.push(item(
        Category::Running,
        "gnome-shell-helper",
        "The GNOME Shell helper for the floating room",
        vec![
            p.data_home
                .join("gnome-shell/extensions")
                .join(GNOME_EXTENSION),
        ],
        Removal::GnomeExtension,
    ));
    items.push(item(
        Category::Running,
        "headless-service",
        "Ghost's own service, installed from a checkout (make service-install, make deploy)",
        vec![
            units.join(DAEMON_UNIT),
            units.join("default.target.wants").join(DAEMON_UNIT),
            units.join(format!("{DAEMON_UNIT}.d")),
        ],
        Removal::UserUnit(DAEMON_UNIT.into()),
    ));

    // (b) Ghost's entries in other tools.
    let w = Where::new(&p.home);
    let found = other_tools::detect(
        &w,
        &other_tools::Ghost {
            ports: p.ports.clone(),
            ..Default::default()
        },
        &mut Vec::new(),
    );
    for tool in [Tool::ClaudeCode, Tool::Cursor, Tool::Codex] {
        let t = found.iter().find(|t| t.tool == tool);
        let linked = t.and_then(|t| t.linked.as_ref());
        let user_file = w.user_file(tool);
        let backups = if tool == Tool::ClaudeCode {
            Vec::new()
        } else {
            backups_beside(&user_file)
        };
        let mut it = item(
            Category::OtherTools,
            &format!("in-{}", tool.id()),
            &format!("Ghost in {}", tool.title()),
            backups,
            Removal::Unlink(tool.id().into()),
        );
        if let Some(l) = linked {
            it.exists = true;
            if tool == Tool::ClaudeCode && t.is_some_and(|t| t.binary.is_none()) {
                it.note = Some(format!(
                    "Ghost does not write Claude Code's settings; run this yourself: {}",
                    other_tools::link::claude_remove_command(&l.entry)
                ));
            }
            if l.file != other_tools::read::shown_path(&w.home, &user_file) {
                it.note = Some(format!(
                    "Ghost is in {}, which Ghost did not write; take it out there.",
                    l.file
                ));
            }
        }
        items.push(it);
    }
    items.push(item(
        Category::OtherTools,
        "client-keys",
        "The keys your other tools use to reach Ghost",
        vec![p.ghost_data.join("clients")],
        Removal::Paths,
    ));

    // (c) The speech models.
    items.push(item(
        Category::SpeechModels,
        "speech-models",
        "Parakeet, Kokoro and Ghost's cache",
        vec![p.ghost_cache.clone()],
        Removal::Paths,
    ));

    // (d) The app's own storage. The window's localStorage (`localstorage/` under the app's
    // data folder) is where its remembered choices live, the `ghost.room.*` keys the app clears
    // at start since #150 among them: the folder goes whole, so no key is named here.
    let mut storage = vec![
        p.data_home.join(APP_ID),
        p.cache_home.join(APP_ID),
        p.config_home.join(APP_ID),
        p.data_home.join(APP_NAME),
        p.cache_home.join(APP_NAME),
        p.ghost_state(),
    ];
    if let Some(rt) = &p.runtime {
        storage.push(rt.join("ghost"));
    }
    items.push(item(
        Category::AppStorage,
        "app-storage",
        "The window's storage and cache, the app's log and choices, Ghost's files while it runs",
        storage,
        Removal::Paths,
    ));

    // (e) The person's data.
    let d = &p.ghost_data;
    items.push(item(
        Category::Data,
        "ghost-folder",
        "Ghost's folder: settings, apps, rules, agents and their memory, skills, outputs",
        vec![p.ghost_config.clone()],
        Removal::Paths,
    ));
    items.push(item(
        Category::Data,
        "database",
        "The database: every transcript, the audit, schedules and questions",
        vec![
            d.join("ghost.db"),
            d.join("ghost.db-wal"),
            d.join("ghost.db-shm"),
        ],
        Removal::Paths,
    ));
    items.push(item(
        Category::Data,
        "wake-takes",
        "Kept wake takes: recordings of your voice saying Ghost's name",
        vec![d.join(crate::wake_takes::FOLDER)],
        Removal::Paths,
    ));
    items.push(item(
        Category::Data,
        "debug-log",
        "Debug mode's log, kept only while it is on (ADR 0073)",
        vec![d.join("debug-audio.log")],
        Removal::Paths,
    ));
    items.push(item(
        Category::Data,
        "hearing-test",
        "The Hearing test's recordings",
        vec![d.join("hear")],
        Removal::Paths,
    ));
    items.push(item(
        Category::Data,
        "error-reports",
        "Error reports kept here and this install's random id",
        vec![d.join("reports")],
        Removal::Paths,
    ));
    let mut rest = item(
        Category::Data,
        "data-folder",
        "Everything else in Ghost's data folder (the paired phone, latency records)",
        vec![d.clone()],
        Removal::Paths,
    );
    rest.keep = p.vault_paths();
    items.push(rest);
    for (n, path) in p.elsewhere.iter().enumerate() {
        items.push(item(
            Category::Data,
            &format!("elsewhere-{n}"),
            "A file Ghost's settings keep outside its folders",
            vec![path.clone()],
            Removal::Paths,
        ));
    }

    // (f) The vault.
    let mut vault = item(
        Category::Vault,
        "vault",
        "The password vault",
        p.vault_paths(),
        Removal::Paths,
    );
    vault.note = Some("Every password saved in Ghost is deleted and cannot be recovered.".into());
    items.push(vault);
    let mut key = item(
        Category::Vault,
        "keyring-key",
        "The vault's key in your login keyring (\"Ghost passwords vault key\")",
        Vec::new(),
        Removal::KeyringItem,
    );
    key.exists = look.keyring_has_key().unwrap_or(false);
    if look.keyring_has_key().is_none() {
        key.note = Some("Ghost could not ask your keyring; it will try when removing.".into());
    }
    items.push(key);

    // Sizes and presence: an item's size counts nothing another item deletes first (the
    // data folder's parts) or keeps (the vault).
    let all: Vec<(String, Vec<PathBuf>)> = items
        .iter()
        .map(|i| (i.key.clone(), i.paths.clone()))
        .collect();
    for it in &mut items {
        let others: Vec<PathBuf> = all
            .iter()
            .filter(|(k, _)| *k != it.key)
            .flat_map(|(_, ps)| ps.iter().cloned())
            .chain(it.keep.iter().cloned())
            .filter(|q| it.paths.iter().any(|mine| q.starts_with(mine) && q != mine))
            .collect();
        for path in &it.paths {
            let (size, files) = own_size(path, &others);
            it.size += size;
            it.exists |= files > 0 || (there(path) && others.is_empty());
        }
    }
    items
}

/// Bytes and files under `path` that are not under one of `others`, never following a link.
fn own_size(path: &Path, others: &[PathBuf]) -> (u64, usize) {
    if others.iter().any(|o| path.starts_with(o)) {
        return (0, 0);
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return (0, 0);
    };
    if !meta.is_dir() {
        return (meta.len(), 1);
    }
    std::fs::read_dir(path)
        .map(|rd| {
            rd.flatten()
                .map(|e| own_size(&e.path(), others))
                .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
        })
        .unwrap_or((0, 0))
}

/// The categories `--keep-data` chooses.
pub fn all_but_data() -> Vec<Category> {
    Category::ALL
        .into_iter()
        .filter(|c| !c.is_the_persons())
        .collect()
}

// --- removing ----------------------------------------------------------------------------

/// Why a path is not one Ghost removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// HOME is empty, relative or `/`.
    NoHome,
    /// Not under the person's home or runtime folder, or the folder itself.
    Outside(PathBuf),
    /// A folder on the way is a link out of the person's folders.
    ThroughALink(PathBuf),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NoHome => write!(f, "your home folder is not set, so Ghost removes nothing"),
            Refused::Outside(p) => write!(
                f,
                "{} is not inside your home folder, so Ghost leaves it",
                p.display()
            ),
            Refused::ThroughALink(p) => write!(
                f,
                "{} is reached through a link out of your folders, so Ghost leaves it",
                p.display()
            ),
        }
    }
}

/// Whether `home` is one removal can work under.
pub fn home_is_usable(home: &Path) -> bool {
    home.is_absolute() && home.components().count() > 1 && !home.as_os_str().is_empty()
}

fn lexically_inside(path: &Path, root: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        && path.starts_with(root)
        && path != root
}

/// Check `path` may be removed: under the home folder or the runtime folder (never either
/// itself), with no folder on the way a link out of them.
pub fn check_path(p: &Places, path: &Path) -> Result<(), Refused> {
    if !home_is_usable(&p.home) {
        return Err(Refused::NoHome);
    }
    let roots: Vec<&Path> = std::iter::once(p.home.as_path())
        .chain(p.runtime.as_deref().filter(|r| home_is_usable(r)))
        .collect();
    let Some(root) = roots.iter().find(|r| lexically_inside(path, r)) else {
        return Err(Refused::Outside(path.to_path_buf()));
    };
    // The folder holding it, resolved, must still be under a root resolved.
    let parent = path.parent().unwrap_or(root);
    let real_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.to_path_buf()))
        .collect();
    let mut probe = parent.to_path_buf();
    while !there(&probe) {
        match probe.parent() {
            Some(up) => probe = up.to_path_buf(),
            None => break,
        }
    }
    let real = std::fs::canonicalize(&probe).unwrap_or(probe);
    if !real_roots.iter().any(|r| real.starts_with(r)) {
        return Err(Refused::ThroughALink(path.to_path_buf()));
    }
    Ok(())
}

/// Delete one path the inventory names: a file or a link as itself, a folder with what it
/// holds (links inside removed as links), `keep` and the folders leading to it left. True
/// when something was removed.
pub fn remove_path(p: &Places, path: &Path, keep: &[PathBuf]) -> Result<bool, String> {
    check_path(p, path).map_err(|r| r.to_string())?;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(false);
    };
    let keeping: Vec<&PathBuf> = keep.iter().filter(|k| k.starts_with(path)).collect();
    if !meta.is_dir() {
        if keeping.iter().any(|k| *k == path) {
            return Ok(false);
        }
        std::fs::remove_file(path).map_err(|e| format!("{}: {}", path.display(), e.kind()))?;
        return Ok(true);
    }
    if keeping.is_empty() {
        std::fs::remove_dir_all(path).map_err(|e| format!("{}: {}", path.display(), e.kind()))?;
        return Ok(true);
    }
    let mut removed = false;
    let entries =
        std::fs::read_dir(path).map_err(|e| format!("{}: {}", path.display(), e.kind()))?;
    for e in entries.flatten() {
        let child = e.path();
        if keeping.iter().any(|k| **k == child) {
            continue;
        }
        removed |= remove_path(p, &child, keep)?;
    }
    Ok(removed)
}

/// What removal asks of the computer beyond deleting files. The daemon's is real
/// (`ghostd::uninstall`); tests give one that records.
pub trait Hands {
    /// `systemctl --user <args>`.
    fn systemctl(&self, args: &[&str]) -> Result<(), String>;
    /// A program on the PATH, when it is there.
    fn program(&self, name: &str) -> Option<PathBuf>;
    /// Run argv as given (never through a shell).
    fn run(&self, argv: &[String]) -> Result<(), String>;
    /// Delete the vault's key from the login keyring: Ok(false) when it was not there.
    fn delete_keyring_key(&self) -> Result<bool, String>;
}

/// What happened to one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub key: String,
    pub what: String,
    /// Removed, or there was nothing to remove.
    pub done: bool,
    /// What was removed (paths shown), or why not.
    pub said: Vec<String>,
    /// A command the person runs themselves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_yourself: Option<String>,
}

fn shown(p: &Places, path: &Path) -> String {
    other_tools::read::shown_path(&p.home, path)
}

fn remove_paths(p: &Places, it: &Item, said: &mut Vec<String>) -> bool {
    let mut ok = true;
    for path in &it.paths {
        match remove_path(p, path, &it.keep) {
            Ok(true) => said.push(format!("removed {}", shown(p, path))),
            Ok(false) => {}
            Err(why) => {
                ok = false;
                said.push(format!("left {}: {why}", shown(p, path)));
            }
        }
    }
    ok
}

/// Take off every item in `chosen` categories, in the inventory's order, each on its own: one
/// that fails leaves the others to go on. Idempotent: what is not there is skipped.
pub fn remove(p: &Places, items: &[Item], chosen: &[Category], hands: &dyn Hands) -> Vec<Outcome> {
    if !home_is_usable(&p.home) {
        return vec![Outcome {
            key: "home".into(),
            what: "Your home folder".into(),
            done: false,
            said: vec![Refused::NoHome.to_string()],
            run_yourself: None,
        }];
    }
    let mut out = Vec::new();
    let mut reload = false;
    for it in items.iter().filter(|i| chosen.contains(&i.category)) {
        let mut said = Vec::new();
        let mut run_yourself = None;
        let done = match &it.removal {
            Removal::Paths => remove_paths(p, it, &mut said),
            Removal::UserUnit(unit) => {
                let unit_file = p.user_units().join(unit);
                if there(&unit_file) {
                    if let Err(e) = hands.systemctl(&["disable", "--now", unit]) {
                        said.push(format!("systemctl could not stop {unit}: {e}"));
                    } else {
                        said.push(format!("stopped and disabled {unit}"));
                    }
                }
                let any = it.paths.iter().any(|q| there(q));
                let ok = remove_paths(p, it, &mut said);
                reload |= any;
                ok
            }
            Removal::GnomeExtension => {
                if it.paths.iter().any(|q| there(q))
                    && let Some(bin) = hands.program("gnome-extensions")
                {
                    let argv = vec![
                        bin.display().to_string(),
                        "uninstall".into(),
                        GNOME_EXTENSION.into(),
                    ];
                    match hands.run(&argv) {
                        Ok(()) => said.push(format!("GNOME removed {GNOME_EXTENSION}")),
                        Err(e) => said.push(format!("gnome-extensions: {e}")),
                    }
                }
                remove_paths(p, it, &mut said)
            }
            Removal::Unlink(id) => {
                let tool = Tool::parse(id).expect("a tool's id");
                let ok = unlink(p, tool, hands, &mut said, &mut run_yourself);
                // The backups, read again: taking Ghost out of a file makes one more.
                let mut again = it.clone();
                if tool != Tool::ClaudeCode {
                    again.paths = backups_beside(&Where::new(&p.home).user_file(tool));
                }
                remove_paths(p, &again, &mut said) && ok
            }
            Removal::KeyringItem => match hands.delete_keyring_key() {
                Ok(true) => {
                    said.push("removed the vault's key from your keyring".into());
                    true
                }
                Ok(false) => true,
                Err(e) => {
                    said.push(format!("the keyring: {e}"));
                    false
                }
            },
        };
        if !said.is_empty() || !done {
            out.push(Outcome {
                key: it.key.clone(),
                what: it.what.clone(),
                done,
                said,
                run_yourself,
            });
        }
    }
    if reload {
        let _ = hands.systemctl(&["daemon-reload"]);
    }
    // A folder kept for what it held (the data folder, for the vault) goes once it is empty.
    for it in items
        .iter()
        .filter(|i| chosen.contains(&i.category) && !i.keep.is_empty())
    {
        for path in &it.paths {
            if check_path(p, path).is_ok()
                && std::fs::remove_dir(path).is_ok()
                && let Some(o) = out.iter_mut().find(|o| o.key == it.key)
            {
                o.said.push(format!("removed {}", shown(p, path)));
            }
        }
    }
    out
}

fn unlink(
    p: &Places,
    tool: Tool,
    hands: &dyn Hands,
    said: &mut Vec<String>,
    run_yourself: &mut Option<String>,
) -> bool {
    let w = Where::new(&p.home);
    let found = other_tools::detect(
        &w,
        &other_tools::Ghost {
            ports: p.ports.clone(),
            ..Default::default()
        },
        &mut Vec::new(),
    );
    let Some(t) = found.iter().find(|t| t.tool == tool) else {
        return true;
    };
    let Some(l) = &t.linked else {
        return true;
    };
    if l.file != other_tools::read::shown_path(&w.home, &w.user_file(tool)) {
        said.push(format!(
            "Ghost is in {}, which Ghost did not write; take it out there",
            l.file
        ));
        return false;
    }
    if tool == Tool::ClaudeCode && t.binary.is_none() {
        let cmd = other_tools::link::claude_remove_command(&l.entry);
        said.push("Ghost does not write Claude Code's settings: run the command shown".into());
        *run_yourself = Some(cmd);
        return false;
    }
    let result = match other_tools::link::unlink_argv(tool, t.binary.as_deref(), &l.entry) {
        Some(argv) => hands.run(&argv).map(|()| None),
        None => other_tools::link::remove_link(
            &w,
            tool,
            &l.entry,
            &chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string(),
        ),
    };
    match result {
        Ok(_) => {
            said.push(format!("{} no longer uses Ghost", tool.title()));
            true
        }
        Err(e) => {
            said.push(format!(
                "Ghost could not be taken out of {}: {e}",
                tool.title()
            ));
            false
        }
    }
}

// --- the by-hand list ----------------------------------------------------------------------

/// Every path the inventory names for a standard install, home shown as `~`, with what it
/// is: the README's list for someone who already removed the package and cannot run
/// `ghostd uninstall` (`the_readme_lists_every_path` holds the two together).
pub fn by_hand() -> Vec<(Category, String, String)> {
    let home = Path::new("/home/person");
    let mut env = BTreeMap::new();
    env.insert("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string());
    let p = Places::standard(home, &env);
    let mut out = Vec::new();
    for it in inventory(&p, &FilesOnly) {
        for path in &it.paths {
            let s = path.display().to_string();
            let s = s
                .strip_prefix("/run/user/1000")
                .map(|r| format!("$XDG_RUNTIME_DIR{r}"))
                .unwrap_or_else(|| shown(&p, path));
            out.push((it.category, s, it.what.clone()));
        }
        if it.key == "in-cursor" || it.key == "in-codex" {
            let tool = if it.key == "in-cursor" {
                Tool::Cursor
            } else {
                Tool::Codex
            };
            let f = shown(&p, &Where::new(home).user_file(tool));
            out.push((it.category, format!("{f}{BACKUP_MARK}*"), it.what.clone()));
        }
    }
    out
}

#[cfg(test)]
#[path = "leftovers_tests.rs"]
mod tests;
