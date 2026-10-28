//! Moving passwords into Ghost's vault, once (ADR 0036).
//!
//! Before the vault, Ghost's passwords were plaintext: `~/.config/ghost/env`, which
//! `ghost-render-env.sh` rewrote from heka-vault at every start, and `apps.env`, the keys
//! the desktop kept for apps. The import takes the values from heka-vault (when its config,
//! `~/.config/heka-vault/ghost.env`, is on this computer), then from those two files, puts
//! them in the vault, checks the vault reads each value back, and removes the plaintext
//! files, unless a value couldn't be imported (too long, or not one line) or a file holds
//! another value than the vault for a name already in it: then the files stay. The
//! non-secret settings in `env` (`ANTHROPIC_BASE_URL`, and anything the renderer copied)
//! move to `settings.env`, which is plain text. A password already in the vault is kept.
//!
//! Nothing here prints, logs or returns a value: the report names the passwords.
//!
//! The KEY=value files are read and written through `EnvFiles` (`ghost_core.envfile` in
//! Python, not ported yet): the daemon passes its implementation, the tests a small one.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::passwords::{
    MAX_VALUE_CHARS, Passwords, PasswordsError, check_name, check_value, is_password_name,
};

pub const LEGACY_ENV: &str = "env"; // ~/.config/ghost/env
pub const LEGACY_APPS_ENV: &str = "apps.env"; // ~/.config/ghost/apps.env
pub const SETTINGS_ENV: &str = "settings.env";
/// What ghost-render-env.sh fetched from heka-vault.
pub const HEKA_KEYS: [&str; 10] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY", // the model connection's own keys (ADR 0037)
    "OPENAI_API_KEY",
    "CLIPROXY_BASE_URL",
    "GITHUB_TOKEN",
    "HF_TOKEN",
    "TYPESAFE_API_KEY",
    "OPENROUTER_API_KEY",
    "ELEVENLABS_API_KEY",
];
/// Settings heka-vault held that are not passwords; CLIPROXY_BASE_URL was only ever the
/// fallback for ANTHROPIC_BASE_URL.
pub const HEKA_SETTINGS: [&str; 2] = ["ANTHROPIC_BASE_URL", "CLIPROXY_BASE_URL"];
pub const FETCH_TIMEOUT_SECS: f64 = 10.0;

/// The KEY=value files (`ghost_core.envfile`): `settings.env`, and the plaintext files
/// the import reads before it removes them. Never echoes a value.
pub trait EnvFiles: Send + Sync {
    /// The file's pairs, in file order; empty when there is no file. Quotes around a
    /// value and a leading `export` are dropped; comments and blank lines skipped.
    fn read_env(&self, path: &Path) -> io::Result<BTreeMap<String, String>>;

    /// Set (`Some`) or remove (`None`) each key, then write the file atomically with
    /// `mode`, one `KEY=value` per line, the old file's comments kept at the top.
    fn update_env(
        &self,
        path: &Path,
        changes: &BTreeMap<String, Option<String>>,
        mode: u32,
    ) -> io::Result<()>;
}

/// Why heka-vault didn't answer: the error's kind (`ConnectError`, `HTTPStatusError`),
/// never its body, which is what the log line shows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FetchError(pub String);

impl FetchError {
    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }
}

/// The value heka-vault holds under a name, or None when it has none.
pub type FetchResult = Result<Option<String>, FetchError>;

/// `(url, project, token, name)` -> the value, or None when heka-vault has none.
pub trait Fetch: Fn(&str, &str, &str, &str) -> FetchResult + Send + Sync {}
impl<F: Fn(&str, &str, &str, &str) -> FetchResult + Send + Sync> Fetch for F {}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// New in the vault.
    pub imported: Vec<String>,
    /// Already in the vault; left as it was.
    pub kept: Vec<String>,
    /// Moved to settings.env.
    pub settings: Vec<String>,
    /// Plaintext files deleted.
    pub removed: Vec<PathBuf>,
    /// Not imported; their files were kept.
    pub skipped: Vec<String>,
    /// Kept, but a plaintext file holds another value: the files were kept.
    pub differ: Vec<String>,
    /// What else to know; never a value.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    HekaVault,
    File,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HekaVault => "heka_vault",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSource {
    pub kind: SourceKind,
    pub path: PathBuf,
}

/// What there is to import on this computer.
pub fn sources(
    config_dir: &Path,
    heka_env: Option<&Path>,
    files: &dyn EnvFiles,
) -> Result<Vec<ImportSource>, PasswordsError> {
    let mut out = Vec::new();
    if let Some(heka) = heka_env
        && heka.is_file()
    {
        out.push(ImportSource {
            kind: SourceKind::HekaVault,
            path: heka.to_path_buf(),
        });
    }
    for name in [LEGACY_ENV, LEGACY_APPS_ENV] {
        let p = config_dir.join(name);
        if p.is_file() {
            out.push(ImportSource {
                kind: SourceKind::File,
                path: p,
            });
        }
    }
    let settings = config_dir.join(SETTINGS_ENV);
    if settings.is_file()
        && files
            .read_env(&settings)?
            .keys()
            .any(|k| is_password_name(k, &[]))
    {
        out.push(ImportSource {
            kind: SourceKind::File,
            path: settings,
        });
    }
    Ok(out)
}

/// What the plaintext files hold, for a daemon that runs before the import: `env`, then
/// `apps.env`. The caller keeps them out of logs and child processes.
pub fn legacy_values(
    config_dir: &Path,
    files: &dyn EnvFiles,
) -> Result<BTreeMap<String, String>, PasswordsError> {
    let mut out = BTreeMap::new();
    for name in [LEGACY_ENV, LEGACY_APPS_ENV] {
        out.extend(files.read_env(&config_dir.join(name))?);
    }
    Ok(out)
}

/// One secret from heka-vault's API, as `ghost-render-env.sh` read it. Blocking: the
/// import runs off the async threads.
pub fn fetch_heka(url: &str, project: &str, token: &str, name: &str) -> FetchResult {
    fn kind(e: reqwest::Error) -> FetchError {
        FetchError::new(if e.is_timeout() {
            "TimeoutException"
        } else if e.is_connect() {
            "ConnectError"
        } else if e.is_status() {
            "HTTPStatusError"
        } else if e.is_request() {
            "RequestError"
        } else {
            "HTTPError"
        })
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs_f64(FETCH_TIMEOUT_SECS))
        .build()
        .map_err(kind)?;
    let r = client
        .get(format!(
            "{}/api/v1/projects/{project}/secrets/{name}",
            url.trim_end_matches('/')
        ))
        .query(&[("env", "dev"), ("format", "raw")])
        .bearer_auth(token)
        .send()
        .map_err(kind)?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let r = r.error_for_status().map_err(kind)?;
    r.text().map(Some).map_err(kind)
}

/// What an import takes besides the vault and the folder.
pub struct ImportOptions<'a> {
    /// heka-vault's config on this computer (`~/.config/heka-vault/ghost.env`), if any.
    pub heka_env: Option<&'a Path>,
    /// The names the connected apps and the webhooks use, so they count as passwords
    /// whatever they're called.
    pub referenced: BTreeSet<String>,
    /// How heka-vault is read; `fetch_heka` unless a test says otherwise.
    pub fetch: &'a dyn Fetch,
}

impl Default for ImportOptions<'_> {
    fn default() -> Self {
        Self {
            heka_env: None,
            referenced: BTreeSet::new(),
            fetch: &fetch_heka,
        }
    }
}

/// Move every password on this computer into `vault` and remove the plaintext files.
/// Errs `VaultClosed` before touching anything when the vault can't be written.
pub fn import_passwords(
    vault: &Passwords,
    config_dir: &Path,
    files: &dyn EnvFiles,
    options: ImportOptions<'_>,
) -> Result<ImportReport, PasswordsError> {
    if !vault.is_open() {
        return Err(PasswordsError::VaultClosed(
            vault
                .problem()
                .unwrap_or_else(|| "the vault isn't open".into()),
        ));
    }
    let mut report = ImportReport::default();
    let mut named = options.referenced.clone();
    let settings_path = config_dir.join(SETTINGS_ENV);
    let settings = files.read_env(&settings_path)?;
    for listed in ["GHOST_APP_SECRETS", "GHOST_MCP_SECRETS"] {
        if let Some(list) = settings.get(listed) {
            named.extend(
                list.split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(String::from),
            );
        }
    }
    let named_refs: Vec<&str> = named.iter().map(String::as_str).collect();

    let mut found: BTreeMap<String, String> = BTreeMap::new(); // password -> value, later sources win
    let mut moved: BTreeMap<String, String> = BTreeMap::new(); // non-secret setting -> value, for settings.env

    if let Some(heka) = options.heka_env
        && heka.is_file()
    {
        from_heka(
            heka,
            &named,
            options.fetch,
            files,
            &mut found,
            &mut moved,
            &mut report,
        )?;
    }

    let env_path = config_dir.join(LEGACY_ENV);
    // name -> every value a plaintext file holds for it
    let mut in_files: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (k, v) in files.read_env(&env_path)? {
        if is_password_name(&k, &named_refs) {
            in_files
                .entry(k.clone())
                .or_default()
                .insert(v.trim().to_string());
            found.insert(k, v);
        } else if !settings.contains_key(&k) {
            moved.insert(k, v);
        }
    }
    for (k, v) in files.read_env(&config_dir.join(LEGACY_APPS_ENV))? {
        in_files
            .entry(k.clone())
            .or_default()
            .insert(v.trim().to_string());
        found.insert(k, v);
    }
    let in_settings: Vec<String> = settings
        .keys()
        .filter(|k| is_password_name(k, &named_refs))
        .cloned()
        .collect();
    for k in &in_settings {
        let v = &settings[k];
        found.insert(k.clone(), v.clone());
        in_files
            .entry(k.clone())
            .or_default()
            .insert(v.trim().to_string());
    }

    let mut new: BTreeMap<String, String> = BTreeMap::new();
    for (name, raw) in &found {
        let value = raw.trim();
        if vault.contains(name) {
            // The vault's copy wins, even over one that can't be imported.
            report.kept.push(name.clone());
            // A file holding another value may hold the only copy of it: compared, never
            // shown.
            if in_files
                .get(name)
                .is_some_and(|vs| vs.iter().any(|v| !v.is_empty() && !vault.holds(name, v)))
            {
                report.differ.push(name.clone());
            }
            continue;
        }
        if value.is_empty() {
            continue; // nothing to lose
        }
        if let Err(e) = check_name(name).and_then(|()| check_value(value)) {
            report.skipped.push(name.clone());
            report.notes.push(format!("{name} wasn't imported: {e}"));
            continue;
        }
        new.insert(name.clone(), value.to_string());
    }
    if !new.is_empty() {
        vault.set_many(new.iter().map(|(n, v)| (n.as_str(), v.as_str())))?;
    }
    report.imported = new.keys().cloned().collect();

    // The vault must read back every value before a plaintext copy goes: the same value,
    // not only the name.
    let check = Passwords::new(vault.path(), vault.keyring());
    if !check.open()
        || !new.iter().all(|(n, v)| check.holds(n, v))
        || !report.kept.iter().all(|n| check.contains(n))
    {
        report
            .notes
            .push("the vault didn't read back; the plaintext files were left as they were".into());
        return Ok(report);
    }
    drop(new);

    if !settings.contains_key("ANTHROPIC_BASE_URL")
        && !moved.contains_key("ANTHROPIC_BASE_URL")
        && let Some(base) = moved.get("CLIPROXY_BASE_URL").cloned()
    {
        moved.insert("ANTHROPIC_BASE_URL".into(), base);
    }
    moved.remove("CLIPROXY_BASE_URL");
    let mut changes: BTreeMap<String, Option<String>> = moved
        .iter()
        .filter(|(k, _)| !settings.contains_key(*k))
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect();
    // A password written into settings.env leaves it, once it is in the vault.
    let stay: BTreeSet<&String> = report.skipped.iter().chain(&report.differ).collect();
    for k in &in_settings {
        if !stay.contains(k) {
            changes.insert(k.clone(), None);
        }
    }
    if !changes.is_empty() {
        files.update_env(&settings_path, &changes, 0o600)?;
    }
    report.settings = changes
        .iter()
        .filter(|(_, v)| v.is_some())
        .map(|(k, _)| k.clone())
        .collect();

    let mut plain: Vec<PathBuf> = [env_path, config_dir.join(LEGACY_APPS_ENV)]
        .into_iter()
        .filter(|p| p.exists())
        .collect();
    let files_said = if plain.is_empty() {
        tilde(&settings_path)
    } else {
        plain
            .iter()
            .map(|p| tilde(p))
            .collect::<Vec<_>>()
            .join(" and ")
    };
    let were = if plain.len() > 1 { "were" } else { "was" };
    if !report.skipped.is_empty() {
        // A skipped value may be the only copy: no file that may hold it is deleted. The
        // vault takes no more than Settings › Passwords does, so that is no way in either.
        let many = report.skipped.len() > 1;
        report.notes.push(format!(
            "{files_said} {were} kept, since {} {} still only there, and Ghost can't keep {}: \
             a password is one line of at most {} characters, named in capital letters, \
             digits and _. If what uses it takes a shorter form, save that on Settings › \
             Passwords; otherwise copy it somewhere safe. Then take it out of the file and \
             import again to remove the plain files.",
            report.skipped.join(", "),
            if many { "are" } else { "is" },
            if many { "them" } else { "it" },
            with_commas(MAX_VALUE_CHARS),
        ));
    }
    if !report.differ.is_empty() {
        // The vault's copy is kept and used; the file's may be the only copy of another.
        let many = report.differ.len() > 1;
        report.notes.push(format!(
            "{files_said} {were} kept, since {} {} another value there than in Ghost's \
             passwords. Ghost uses the one in its passwords. If the file's is the right one, \
             save it on Settings › Passwords. Then take it out of the file and import again \
             to remove the plain files.",
            report.differ.join(", "),
            if many { "have" } else { "has" },
        ));
    }
    if !report.skipped.is_empty() || !report.differ.is_empty() {
        plain.clear();
    }
    for path in plain {
        fs::remove_file(&path)?;
        report.removed.push(path);
    }
    if let Some(heka) = options.heka_env
        && heka.is_file()
    {
        report.notes.push(format!(
            "{} still holds the password Ghost read heka-vault with. Ghost doesn't read it \
             any more; delete it when nothing else needs it.",
            tilde(heka)
        ));
    }
    if report.imported.is_empty()
        && report.kept.is_empty()
        && report.removed.is_empty()
        && report.settings.is_empty()
    {
        report.notes.push("There was nothing to import.".into());
    }
    info!(
        "passwords: imported {}, kept {}, removed {} plaintext file(s)",
        report.imported.len(),
        report.kept.len(),
        report.removed.len()
    );
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn from_heka(
    heka_env: &Path,
    named: &BTreeSet<String>,
    fetch: &dyn Fetch,
    files: &dyn EnvFiles,
    found: &mut BTreeMap<String, String>,
    moved: &mut BTreeMap<String, String>,
    report: &mut ImportReport,
) -> Result<(), PasswordsError> {
    let conf = files.read_env(heka_env)?;
    let get = |k: &str| conf.get(k).cloned().unwrap_or_default();
    let (url, project) = (get("HEKA_VAULT_URL"), get("HEKA_VAULT_PROJECT"));
    let token = Zeroizing::new(get("HEKA_VAULT_TOKEN"));
    if url.is_empty() || project.is_empty() || token.is_empty() {
        report.notes.push(format!(
            "{} doesn't name heka-vault's address, project and token",
            tilde(heka_env)
        ));
        return Ok(());
    }
    let mut failed = 0;
    let mut names: Vec<&str> = HEKA_KEYS.to_vec();
    for n in named {
        if !names.contains(&n.as_str()) {
            names.push(n);
        }
    }
    for name in names {
        match fetch(&url, &project, &token, name) {
            Err(e) => {
                // The store's own trouble; never its body.
                failed += 1;
                warn!("passwords: heka-vault didn't give {name} ({e})");
            }
            Ok(None) => {}
            Ok(Some(value)) => {
                if HEKA_SETTINGS.contains(&name) {
                    moved.insert(name.to_string(), value.trim().to_string());
                } else {
                    found.insert(name.to_string(), value);
                }
            }
        }
    }
    if failed > 0 {
        report.notes.push(format!(
            "heka-vault didn't answer for {failed} name(s); see the log"
        ));
    }
    Ok(())
}

/// Python's `f"{n:,}"`.
fn with_commas(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    //! The one-time import (ADR 0036): heka-vault and the plaintext env files into the
    //! vault, read back, then the plaintext copies removed. Temp folders and a fake keyring
    //! and a fake heka-vault only.

    use super::*;
    use crate::passwords::{Keyring, MemoryKeyring, VaultKey};
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    /// `ghost_core.envfile` as the Python has it, for these fixtures.
    struct TestEnvFiles;

    impl EnvFiles for TestEnvFiles {
        fn read_env(&self, path: &Path) -> io::Result<BTreeMap<String, String>> {
            if !path.exists() {
                return Ok(BTreeMap::new());
            }
            let line_re =
                regex::Regex::new(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$")
                    .unwrap();
            let mut out = BTreeMap::new();
            for line in fs::read_to_string(path)?.lines() {
                if line.trim().is_empty() || line.trim_start().starts_with('#') {
                    continue;
                }
                if let Some(m) = line_re.captures(line) {
                    let mut value = m[2].to_string();
                    let b = value.as_bytes();
                    if b.len() >= 2 && b[0] == b[b.len() - 1] && (b[0] == b'"' || b[0] == b'\'') {
                        value = value[1..value.len() - 1].to_string();
                    }
                    out.insert(m[1].to_string(), value);
                }
            }
            Ok(out)
        }

        fn update_env(
            &self,
            path: &Path,
            changes: &BTreeMap<String, Option<String>>,
            mode: u32,
        ) -> io::Result<()> {
            let mut values = self.read_env(path)?;
            for (k, v) in changes {
                match v {
                    None => {
                        values.remove(k);
                    }
                    Some(v) => {
                        values.insert(k.clone(), v.clone());
                    }
                }
            }
            let comments: Vec<String> = if path.exists() {
                fs::read_to_string(path)?
                    .lines()
                    .filter(|l| l.trim_start().starts_with('#'))
                    .map(String::from)
                    .collect()
            } else {
                vec![]
            };
            let quote = |v: &str| {
                if v.is_empty() || v.chars().any(|c| " #\"'\\$\n\t".contains(c)) {
                    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
                } else {
                    v.to_string()
                }
            };
            let mut lines = comments;
            lines.extend(values.iter().map(|(k, v)| format!("{k}={}", quote(v))));
            let body = lines.join("\n") + "\n";
            let tmp = path.with_extension("tmp");
            {
                let mut f = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(mode)
                    .open(&tmp)?;
                use std::io::Write;
                f.write_all(body.as_bytes())?;
            }
            fs::rename(&tmp, path)
        }
    }

    const FILES: &TestEnvFiles = &TestEnvFiles;

    fn read_env(path: &Path) -> BTreeMap<String, String> {
        FILES.read_env(path).unwrap()
    }

    fn write(path: &Path, text: &str) {
        fs::write(path, text).unwrap();
    }

    fn world(tmp: &TempDir) -> (PathBuf, Passwords) {
        let config = tmp.path().join("config");
        fs::create_dir(&config).unwrap();
        write(
            &config.join("env"),
            "ANTHROPIC_AUTH_TOKEN=sk-ant-from-env-file-123\n\
             ANTHROPIC_BASE_URL=http://proxy.lan:8317\n\
             GHOST_INPUT_DEVICE=ghost.mic\n\
             MCP_PROXY_KEY=proxy-key-0123456789\n",
        );
        write(
            &config.join("apps.env"),
            "GHOST_APP_MOBBIN_KEY=\"Bearer mob-0123456789\"\n",
        );
        write(
            &config.join("settings.env"),
            "GHOST_INPUT_DEVICE=ghost.mic\nGHOST_JEV=auto\n",
        );
        let ring = Arc::new(MemoryKeyring::new());
        let vault = Passwords::new(tmp.path().join("data").join("passwords.vault"), ring);
        assert!(vault.open());
        (config, vault)
    }

    fn import(vault: &Passwords, config: &Path) -> ImportReport {
        import_passwords(vault, config, FILES, ImportOptions::default()).unwrap()
    }

    fn names(v: &[ImportSource]) -> BTreeSet<String> {
        v.iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn file_names(v: &[PathBuf]) -> BTreeSet<String> {
        v.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn test_env_files_move_into_the_vault_and_are_removed() {
        let tmp = TempDir::new().unwrap();
        let (config, vault) = world(&tmp);
        assert_eq!(
            names(&sources(&config, None, FILES).unwrap()),
            BTreeSet::from(["env".to_string(), "apps.env".to_string()])
        );
        let report = import(&vault, &config);
        assert_eq!(
            report.imported,
            vec![
                "ANTHROPIC_AUTH_TOKEN",
                "GHOST_APP_MOBBIN_KEY",
                "MCP_PROXY_KEY"
            ]
        );
        assert_eq!(report.settings, vec!["ANTHROPIC_BASE_URL"]);
        assert_eq!(
            file_names(&report.removed),
            BTreeSet::from(["env".to_string(), "apps.env".to_string()])
        );
        assert!(!config.join("env").exists() && !config.join("apps.env").exists());
        assert_eq!(
            read_env(&config.join("settings.env")),
            BTreeMap::from([
                ("GHOST_INPUT_DEVICE".to_string(), "ghost.mic".to_string()),
                ("GHOST_JEV".to_string(), "auto".to_string()),
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "http://proxy.lan:8317".to_string()
                ),
            ])
        );
        let fresh = Passwords::new(vault.path(), vault.keyring());
        assert!(fresh.open());
        assert_eq!(
            fresh.value("GHOST_APP_MOBBIN_KEY").map(|v| v.to_string()),
            Some("Bearer mob-0123456789".into())
        );
        assert!(sources(&config, None, FILES).unwrap().is_empty());
        // Nothing in the report is a value.
        assert!(!format!("{report:?}").contains("sk-ant-from-env-file-123"));
    }

    #[test]
    fn test_heka_vault_is_read_once_and_the_files_win() {
        let tmp = TempDir::new().unwrap();
        let (config, vault) = world(&tmp);
        let heka = tmp.path().join("heka").join("ghost.env");
        fs::create_dir(heka.parent().unwrap()).unwrap();
        write(
            &heka,
            "HEKA_VAULT_URL=https://vault.lan\nHEKA_VAULT_PROJECT=ghost\nHEKA_VAULT_TOKEN=t\n",
        );
        let held = BTreeMap::from([
            ("ANTHROPIC_AUTH_TOKEN", "sk-ant-from-heka-old"),
            ("HF_TOKEN", "hf_from_heka_0123456"),
            ("CLIPROXY_BASE_URL", "http://cliproxy.lan:8317"),
            ("LINEAR_TOKEN", "lin_from_heka_01234"),
        ]);
        let asked: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(vec![]);
        let fetch = |url: &str, project: &str, token: &str, name: &str| -> FetchResult {
            assert_eq!((url, project, token), ("https://vault.lan", "ghost", "t"));
            asked.lock().unwrap().push(name.to_string());
            if name == "ELEVENLABS_API_KEY" {
                return Err(FetchError::new("ConnectionError"));
            }
            Ok(held.get(name).map(|v| v.to_string()))
        };
        let report = import_passwords(
            &vault,
            &config,
            FILES,
            ImportOptions {
                heka_env: Some(&heka),
                referenced: BTreeSet::from(["LINEAR_TOKEN".to_string()]),
                fetch: &fetch,
            },
        )
        .unwrap();
        assert!(asked.lock().unwrap().iter().any(|n| n == "LINEAR_TOKEN"));
        assert_eq!(
            report.imported.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "HF_TOKEN".to_string(),
                "LINEAR_TOKEN".to_string(),
                "GHOST_APP_MOBBIN_KEY".to_string(),
                "MCP_PROXY_KEY".to_string(),
            ])
        );
        // The newer copy.
        assert_eq!(
            vault.value("ANTHROPIC_AUTH_TOKEN").map(|v| v.to_string()),
            Some("sk-ant-from-env-file-123".into())
        );
        // The env file's base URL is explicit, so CLIPROXY's isn't used.
        assert_eq!(
            read_env(&config.join("settings.env"))["ANTHROPIC_BASE_URL"],
            "http://proxy.lan:8317"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("heka-vault didn't answer for 1"))
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("still holds the password Ghost read heka-vault with"))
        );
        assert!(heka.exists()); // heka-vault's own file is the operator's to delete
    }

    #[test]
    fn test_a_password_already_in_the_vault_with_the_same_value_lets_the_files_go() {
        let tmp = TempDir::new().unwrap();
        let (config, vault) = world(&tmp);
        vault
            .set("ANTHROPIC_AUTH_TOKEN", "sk-ant-from-env-file-123")
            .unwrap();
        let report = import(&vault, &config);
        assert_eq!(report.kept, vec!["ANTHROPIC_AUTH_TOKEN"]);
        assert!(report.differ.is_empty());
        assert!(!config.join("env").exists() && !config.join("apps.env").exists());
    }

    /// The vault's copy wins, but the file's other value may be the only copy of it: the
    /// files stay, and the note names the password, never either value.
    #[test]
    fn test_a_password_already_in_the_vault_with_another_value_keeps_the_files() {
        let tmp = TempDir::new().unwrap();
        let (config, vault) = world(&tmp);
        vault
            .set("ANTHROPIC_AUTH_TOKEN", "sk-ant-set-in-ghost-999")
            .unwrap();
        write(
            &config.join("settings.env"),
            "GHOST_JEV=auto\nANTHROPIC_AUTH_TOKEN=sk-ant-in-set\n",
        );
        let report = import(&vault, &config);
        assert_eq!(report.kept, vec!["ANTHROPIC_AUTH_TOKEN"]);
        assert_eq!(report.differ, vec!["ANTHROPIC_AUTH_TOKEN"]);
        assert_eq!(
            vault.value("ANTHROPIC_AUTH_TOKEN").map(|v| v.to_string()),
            Some("sk-ant-set-in-ghost-999".into())
        );
        assert!(report.removed.is_empty());
        assert!(config.join("env").exists() && config.join("apps.env").exists());
        assert_eq!(
            read_env(&config.join("settings.env"))["ANTHROPIC_AUTH_TOKEN"],
            "sk-ant-in-set"
        );
        // The rest still goes into the vault.
        assert!(report.imported.iter().any(|n| n == "MCP_PROXY_KEY"));
        let note = report
            .notes
            .iter()
            .find(|n| n.contains("another value"))
            .unwrap();
        assert!(note.contains("ANTHROPIC_AUTH_TOKEN") && note.contains("Settings › Passwords"));
        let shown = format!("{report:?}");
        for value in [
            "sk-ant-set-in-ghost-999",
            "sk-ant-from-env-file-123",
            "sk-ant-in-set",
        ] {
            assert!(!shown.contains(value));
        }

        // Taken out of the files, the next import removes them.
        write(
            &config.join("env"),
            "ANTHROPIC_BASE_URL=http://proxy.lan:8317\n",
        );
        write(&config.join("settings.env"), "GHOST_JEV=auto\n");
        let again = import(&vault, &config);
        assert!(again.differ.is_empty() && !config.join("env").exists());
    }

    #[test]
    fn test_a_password_written_into_settings_env_leaves_it() {
        let tmp = TempDir::new().unwrap();
        let (config, vault) = world(&tmp);
        write(
            &config.join("settings.env"),
            "GHOST_JEV=auto\nTYPESAFE_API_KEY=ts_0123456789\n",
        );
        let report = import(&vault, &config);
        assert!(report.imported.iter().any(|n| n == "TYPESAFE_API_KEY"));
        assert!(!read_env(&config.join("settings.env")).contains_key("TYPESAFE_API_KEY"));
    }

    #[test]
    fn test_a_locked_vault_touches_nothing() {
        let tmp = TempDir::new().unwrap();
        let (config, _) = world(&tmp);
        let ring = Arc::new(MemoryKeyring::locked());
        let vault = Passwords::new(tmp.path().join("data").join("passwords.vault"), ring);
        assert!(vault.open()); // no file yet: open, but saving needs the keyring
        assert!(matches!(
            import_passwords(&vault, &config, FILES, ImportOptions::default()),
            Err(PasswordsError::VaultClosed(_))
        ));
        assert!(config.join("env").exists() && config.join("apps.env").exists());
    }

    #[test]
    fn test_nothing_to_import() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config");
        fs::create_dir(&config).unwrap();
        let vault = Passwords::new(
            tmp.path().join("passwords.vault"),
            Arc::new(MemoryKeyring::new()),
        );
        assert!(vault.open());
        let report = import(&vault, &config);
        assert_eq!(report.notes, vec!["There was nothing to import."]);
    }

    #[test]
    fn test_legacy_values_read_both_files() {
        let tmp = TempDir::new().unwrap();
        let (config, _) = world(&tmp);
        let values = legacy_values(&config, FILES).unwrap();
        assert_eq!(values["MCP_PROXY_KEY"], "proxy-key-0123456789");
        assert_eq!(values["GHOST_APP_MOBBIN_KEY"], "Bearer mob-0123456789");
    }

    /// A value the vault refuses may be the only copy: the files holding it stay.
    #[test]
    fn test_a_value_that_cant_be_imported_keeps_the_files() {
        for bad in ["x".repeat(16_001), "nul\0inside-0123456".to_string()] {
            let tmp = TempDir::new().unwrap();
            let (config, vault) = world(&tmp);
            write(
                &config.join("apps.env"),
                &format!(
                    "GHOST_APP_MOBBIN_KEY=\"Bearer mob-0123456789\"\nGHOST_APP_HUGE_KEY={bad}\n"
                ),
            );
            let report = import(&vault, &config);
            assert_eq!(report.skipped, vec!["GHOST_APP_HUGE_KEY"]);
            assert!(report.imported.iter().any(|n| n == "GHOST_APP_MOBBIN_KEY"));
            assert!(report.removed.is_empty());
            assert!(config.join("env").exists() && config.join("apps.env").exists());
            let note = report
                .notes
                .iter()
                .find(|n| n.contains("were kept"))
                .unwrap();
            assert!(note.contains("GHOST_APP_HUGE_KEY") && note.contains("16,000 characters"));
            // Not "add it on Settings", which refuses it too.
            assert!(note.contains("take it out of the file"));
            assert!(!format!("{report:?}").contains(&bad));

            // A shorter form saved by hand: the file's value is another one, so the files
            // stay until it is taken out of them.
            vault
                .set("GHOST_APP_HUGE_KEY", "a-good-value-by-hand")
                .unwrap();
            let again = import(&vault, &config);
            assert!(again.skipped.is_empty());
            assert_eq!(again.differ, vec!["GHOST_APP_HUGE_KEY"]);
            assert!(config.join("apps.env").exists());
            write(
                &config.join("apps.env"),
                "GHOST_APP_MOBBIN_KEY=\"Bearer mob-0123456789\"\n",
            );
            let third = import(&vault, &config);
            assert!(third.differ.is_empty());
            assert!(!third.kept.iter().any(|n| n == "GHOST_APP_HUGE_KEY"));
            assert!(!config.join("env").exists() && !config.join("apps.env").exists());
        }
    }

    /// A lowercase name shaped like a password counts as one, but the vault names are
    /// capitals: it is skipped, and the files stay.
    #[test]
    fn test_a_name_the_vault_cant_take_keeps_the_files() {
        let tmp = TempDir::new().unwrap();
        let (config, vault) = world(&tmp);
        write(
            &config.join("apps.env"),
            "openai_api_key=sk-lower-0123456789\n",
        );
        let report = import(&vault, &config);
        assert_eq!(report.skipped, vec!["openai_api_key"]);
        assert!(config.join("apps.env").exists() && report.removed.is_empty());
    }

    /// A keyring that, the first time the read-back vault asks it for the key, lets
    /// another writer change MCP_PROXY_KEY under it: the vault "comes back with another
    /// value" (Python monkeypatched `holds`; here the file really differs).
    struct RacingKeyring {
        inner: Arc<MemoryKeyring>,
        path: PathBuf,
        fired: AtomicBool,
    }

    impl Keyring for RacingKeyring {
        fn load_key(&self) -> Result<Option<VaultKey>, PasswordsError> {
            if self.path.exists() && !self.fired.swap(true, Ordering::SeqCst) {
                let other = Passwords::new(&self.path, self.inner.clone());
                assert!(other.open());
                other
                    .set("MCP_PROXY_KEY", "another-value-0123456789")
                    .unwrap();
            }
            self.inner.load_key()
        }

        fn store_key(&self, key: &[u8]) -> Result<(), PasswordsError> {
            self.inner.store_key(key)
        }

        fn unlock(&self) -> Result<bool, PasswordsError> {
            self.inner.unlock()
        }
    }

    #[test]
    fn test_the_read_back_compares_values_not_names() {
        let tmp = TempDir::new().unwrap();
        let (config, _) = world(&tmp);
        let path = tmp.path().join("data").join("passwords.vault");
        let ring = Arc::new(RacingKeyring {
            inner: Arc::new(MemoryKeyring::new()),
            path: path.clone(),
            fired: AtomicBool::new(false),
        });
        let vault = Passwords::new(&path, ring);
        assert!(vault.open());
        let report = import(&vault, &config);
        assert!(report.notes.iter().any(|n| n.contains("didn't read back")));
        assert!(report.removed.is_empty() && config.join("env").exists());
    }
}
