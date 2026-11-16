//! `ghostd uninstall`: remove Ghost from this computer (#147, ADR 0071).
//!
//! The list is `ghost_core::leftovers`; this is its command, the real hands it removes with
//! (systemd, the tools' own commands, the keyring), stopping Ghost first, and the one line the
//! person runs to finish (the package's removal, or which AppImage to delete). It never needs
//! root and never runs `apt`. The desktop's "Remove Ghost from this computer" runs it too,
//! after the app has quit (`--only … --yes --after <pid>`).
//!
//! The operator's only: no tool of Ghost's own app, no outside client, the phone or a run
//! reaches it (the control API's `GET /uninstall` lists, and nothing on it removes).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ghost_core::leftovers::{self, Category, Hands, Item, Look, Outcome, Places};
use ghost_core::passwords::{Keyring, SecretServiceKeyring};
use ghost_voice::config::VoiceSettings;

use crate::config::GhostdSettings;

pub const USAGE: &str = "usage: ghostd uninstall [--dry-run | --all | --keep-data] [--yes]\n  \
     --dry-run    list what Ghost left on this computer, and remove nothing\n  \
     --all        choose everything, your data and passwords too (asks once)\n  \
     --keep-data  choose everything but your data and passwords (asks once)\n  \
     --yes        do not ask\n\
     With no option, each kind is shown with its size and asked about in turn.";

/// Where Ghost's folders are, from the settings the daemon would run with.
pub fn places(
    cfg: &GhostdSettings,
    voice: &VoiceSettings,
    env: &HashMap<String, String>,
) -> Places {
    let home = PathBuf::from(env.get("HOME").cloned().unwrap_or_default());
    let env: BTreeMap<String, String> = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut p = Places::standard(&home, &env);
    p.ghost_config = cfg.config_dir.clone();
    p.ghost_data = cfg.data_dir.clone();
    p.ghost_cache = voice.cache_dir.clone();
    let inside = |q: &Path| {
        [&p.ghost_config, &p.ghost_data, &p.ghost_cache]
            .iter()
            .any(|r| q.starts_with(r))
    };
    let mut elsewhere = Vec::new();
    for q in [
        Some(cfg.db_path.clone()),
        Some(cfg.policy_path.clone()),
        Some(cfg.settings_path.clone()),
        Some(cfg.env_path.clone()),
        Some(cfg.catalog_path.clone()),
        Some(cfg.skills_dir.clone()),
        cfg.latency_log.clone(),
    ]
    .into_iter()
    .flatten()
    {
        if !inside(&q) && !elsewhere.contains(&q) {
            elsewhere.push(q);
        }
    }
    p.elsewhere = elsewhere;
    let mut ports: Vec<u16> = cfg
        .mcp_listen
        .split(',')
        .filter_map(|a| {
            a.trim()
                .rsplit_once(':')
                .and_then(|(_, port)| port.parse().ok())
        })
        .collect();
    if !ports.contains(&leftovers::DEFAULT_PORT) {
        ports.push(leftovers::DEFAULT_PORT);
    }
    p.ports = ports;
    p
}

/// The places this process's environment gives.
pub fn places_now() -> anyhow::Result<Places> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let (cfg, voice) = crate::daemon::settings_from(&env)?;
    Ok(places(&cfg, &voice, &env))
}

/// The keyring, asked without a prompt.
pub struct KeyringLook;

impl Look for KeyringLook {
    fn keyring_has_key(&self) -> Option<bool> {
        SecretServiceKeyring.load_key().ok().map(|k| k.is_some())
    }
}

/// systemd, the tools' own commands and the keyring, for real.
pub struct RealHands;

fn child(argv0: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(argv0);
    cmd.env_clear()
        .envs(ghost_core::redact::child_env(std::env::vars())) // never a password (ADR 0036)
        .stdin(std::process::Stdio::null());
    cmd
}

fn finished(out: std::io::Result<std::process::Output>) -> Result<(), String> {
    let out = out.map_err(|e| e.kind().to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    text.truncate(300);
    Err(if text.is_empty() {
        format!("exit {}", out.status)
    } else {
        text
    })
}

impl Hands for RealHands {
    fn systemctl(&self, args: &[&str]) -> Result<(), String> {
        finished(child("systemctl").arg("--user").args(args).output())
    }

    fn program(&self, name: &str) -> Option<PathBuf> {
        let path = std::env::var("PATH").ok()?;
        std::env::split_paths(&path)
            .filter(|d| d.is_absolute())
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    }

    fn run(&self, argv: &[String]) -> Result<(), String> {
        let (first, rest) = argv.split_first().ok_or("nothing to run")?;
        finished(child(first).args(rest).output())
    }

    fn delete_keyring_key(&self) -> Result<bool, String> {
        SecretServiceKeyring.delete_key().map_err(|e| e.to_string())
    }
}

// --- the control API's list ----------------------------------------------------------------

/// `GET /uninstall`: the list, from the daemon's own settings.
pub struct UninstallDesk {
    pub places: Places,
}

impl ghost_api::uninstall::UninstallPort for UninstallDesk {
    fn view(&self) -> ghost_api::uninstall::UninstallView {
        view(
            &self.places,
            &leftovers::inventory(&self.places, &KeyringLook),
            &finish_now(None),
        )
    }
}

/// The list as the control API gives it.
pub fn view(p: &Places, items: &[Item], finish: &Finish) -> ghost_api::uninstall::UninstallView {
    use ghost_api::uninstall::{LeftoverCategoryView, LeftoverItemView, UninstallView};
    UninstallView {
        categories: present(items)
            .into_iter()
            .map(|(c, size, there)| LeftoverCategoryView {
                id: c.id().into(),
                title: c.title().into(),
                sentence: c.sentence().into(),
                warning: c.warning().map(String::from),
                persons: c.is_the_persons(),
                size,
                items: there
                    .into_iter()
                    .map(|i| LeftoverItemView {
                        key: i.key.clone(),
                        what: i.what.clone(),
                        paths: i
                            .paths
                            .iter()
                            .filter(|q| std::fs::symlink_metadata(q).is_ok())
                            .map(|q| shown(p, q))
                            .collect(),
                        size: i.size,
                        note: i.note.clone(),
                    })
                    .collect(),
            })
            .collect(),
        install: finish.kind.clone(),
        finish: finish.line.clone(),
    }
}

// --- the echo canceller, kept to this version ----------------------------------------------

/// At every start (#147): the echo canceller's unit rewritten to this version's form when it is
/// Ghost's own, and its `installed` link pointed at this install, so the unit stops starting
/// once Ghost is gone and keeps starting when an AppImage moved. Quiet; what it did is said in
/// the returned lines for the log.
pub fn keep_echo_canceller_current(p: &Places, hands: &dyn Hands) -> Vec<String> {
    use ghost_api::echo_cancel::{
        UnitRefresh, installed_target_now, keep_installed_link, refresh_unit,
    };
    let units = p.config_home.join("systemd/user");
    let unit = units.join(leftovers::ECHO_UNIT);
    let mut said = Vec::new();
    let refreshed = match refresh_unit(&unit) {
        Ok(UnitRefresh::Absent) => return said,
        Ok(UnitRefresh::NotGhosts) => {
            said.push(format!(
                "{} is not Ghost's own; left as it is",
                unit.display()
            ));
            false
        }
        Ok(UnitRefresh::Current) => false,
        Ok(UnitRefresh::Rewritten) => {
            said.push(format!("rewrote {} to this version's form", unit.display()));
            let _ = hands.systemctl(&["daemon-reload"]);
            true
        }
        Err(e) => {
            said.push(format!("{}: {}", unit.display(), e.kind()));
            false
        }
    };
    let Some(target) = installed_target_now() else {
        return said;
    };
    let moved = match keep_installed_link(&p.installed_link(), &target) {
        Ok(changed) => changed,
        Err(e) => {
            said.push(format!("{}: {}", p.installed_link().display(), e.kind()));
            false
        }
    };
    if moved {
        said.push(format!(
            "{} -> {}",
            p.installed_link().display(),
            target.display()
        ));
    }
    // Enabled, and its condition may have kept it from starting until now: start it.
    let enabled = std::fs::symlink_metadata(
        units
            .join("default.target.wants")
            .join(leftovers::ECHO_UNIT),
    )
    .is_ok();
    if (moved || refreshed) && enabled {
        let _ = hands.systemctl(&["start", leftovers::ECHO_UNIT]);
    }
    said
}

// --- the line that finishes it -------------------------------------------------------------

/// How Ghost came to be on this computer, and the one line that takes the rest away.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finish {
    /// `deb`, `appimage` or `checkout`.
    pub kind: String,
    /// The command to run, or the file to delete.
    pub line: String,
}

/// From this Ghost's own AppImage (never another program's `$APPIMAGE`), the running binary,
/// and whether dpkg lists the package.
pub fn finish(appimage: Option<&Path>, exe: Option<&Path>, package_listed: bool) -> Finish {
    if let Some(a) = appimage {
        return Finish {
            kind: "appimage".into(),
            line: format!(
                "rm {}",
                ghost_core::other_tools::link::shell_words(&[a.display().to_string()])
            ),
        };
    }
    if package_listed || exe.is_some_and(|e| e.starts_with("/usr/bin") || e.starts_with("/usr/lib"))
    {
        return Finish {
            kind: "deb".into(),
            line: format!("sudo apt remove {}", leftovers::PACKAGE),
        };
    }
    Finish {
        kind: "checkout".into(),
        line: "Nothing to uninstall: this Ghost was built from a checkout.".into(),
    }
}

/// This process's [`finish`]. `told` is the AppImage the app named for the copy of `ghostd`
/// it runs from outside its mount (`--appimage`, with `--after`); otherwise it is the one
/// this binary runs from (`ghost_core::appimage::file`). A shell inside another program's
/// AppImage hands ghostd that program's `$APPIMAGE`, and this line once said to `rm` it.
pub fn finish_now(told: Option<&Path>) -> Finish {
    let appimage = told
        .map(Path::to_path_buf)
        .or_else(ghost_core::appimage::file);
    let exe = std::env::current_exe().ok();
    let listed = Path::new(&format!("/var/lib/dpkg/info/{}.list", leftovers::PACKAGE)).exists();
    finish(appimage.as_deref(), exe.as_deref(), listed)
}

// --- stopping Ghost ------------------------------------------------------------------------

/// One of Ghost's processes: the app (and its floating room), or a `ghostd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: i32,
    pub app: bool,
}

/// The data folder a process's environment gives it: `GHOST_DATA_DIR`, else its own
/// `HOME`'s `.local/share/ghost` (the shell passes `settings.env` on in the environment).
pub fn data_dir_of(environ: &[u8]) -> Option<PathBuf> {
    let mut home = None;
    let mut data = None;
    for kv in environ.split(|b| *b == 0) {
        let kv = String::from_utf8_lossy(kv);
        if let Some(v) = kv.strip_prefix("GHOST_DATA_DIR=") {
            data = (!v.trim().is_empty()).then(|| PathBuf::from(v));
        } else if let Some(v) = kv.strip_prefix("HOME=") {
            home = Some(PathBuf::from(v));
        }
    }
    data.or_else(|| home.map(|h| h.join(".local/share/ghost")))
}

/// Whose a Ghost process is, from its own environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    /// Its data folder is the one being removed.
    Ours,
    /// Another Ghost on this account, with folders of its own (a test's, a second home).
    Theirs,
    /// Its environment can't be read or names no data folder: Ghost can't tell.
    Unknown,
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || real(a) == real(b)
}

/// From a process's `/proc/<pid>/environ` (empty when it could not be read).
pub fn whose(environ: &[u8], data: &Path) -> Whose {
    match data_dir_of(environ) {
        None => Whose::Unknown,
        Some(d) if same_folder(&d, data) => Whose::Ours,
        Some(_) => Whose::Theirs,
    }
}

/// One of this user's processes, when it is a Ghost (`heka-ghost-desktop`, whose name the
/// kernel keeps as `heka-ghost-desk`, or a `ghostd`) and still alive (a zombie is gone):
/// whether it is the app, and whose it is.
fn ghost_process(pid: i32, data: &Path, me: Option<u32>) -> Option<(bool, Whose)> {
    let dir = PathBuf::from(format!("/proc/{pid}"));
    if std::fs::metadata(&dir).map(|m| m.uid()).ok() != me {
        return None;
    }
    let comm = std::fs::read_to_string(dir.join("comm")).ok()?;
    let app = match comm.trim() {
        "heka-ghost-desk" => true,
        "ghostd" => false,
        _ => return None,
    };
    let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
    if status
        .lines()
        .any(|l| l.starts_with("State:") && l.contains('Z'))
    {
        return None;
    }
    let environ = std::fs::read(dir.join("environ")).unwrap_or_default();
    Some((app, whose(&environ, data)))
}

/// This user's Ghost processes but `skip`, as (the ones whose data folder is `data`, the ones
/// Ghost can't place). Another Ghost on the same account with folders of its own is in
/// neither, and is never signalled.
pub fn ghost_processes(data: &Path, skip: &[i32]) -> (Vec<Proc>, Vec<i32>) {
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let (mut ours, mut unknown) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else {
            continue;
        };
        if skip.contains(&pid) {
            continue;
        }
        match ghost_process(pid, data, me) {
            Some((app, Whose::Ours)) => ours.push(Proc { pid, app }),
            Some((_, Whose::Unknown)) => unknown.push(pid),
            _ => {}
        }
    }
    (ours, unknown)
}

/// SIGTERM to `pid`, only if it is still a Ghost whose data folder is `data` (read again just
/// before, so a process id the kernel gave to someone else is never signalled).
fn term(pid: i32, data: &Path) {
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    if matches!(ghost_process(pid, data, me), Some((_, Whose::Ours))) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );
    }
}

fn wait_gone(data: &Path, skip: &[i32], within: Duration) -> Vec<Proc> {
    let start = Instant::now();
    loop {
        let (left, _) = ghost_processes(data, skip);
        if left.is_empty() || start.elapsed() >= within {
            return left;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Stop Ghost: its own service if it runs one, the app (which stops the daemon it carries
/// and the floating room, ADR 0038), then any `ghostd` left, only ever processes whose own
/// environment gives them `data`, the folder being removed. A Ghost process Ghost can't place
/// (its environment unreadable, or naming no folder) refuses the whole removal before
/// anything is signalled: Ghost never guesses. Err with a sentence when it refuses or when
/// something would not stop.
pub fn stop_ghost(
    data: &Path,
    hands: &dyn Hands,
    skip: &[i32],
    out: &mut dyn Write,
) -> Result<(), String> {
    let (found, unknown) = ghost_processes(data, skip);
    if !unknown.is_empty() {
        let pids: Vec<String> = unknown.iter().map(|p| p.to_string()).collect();
        return Err(format!(
            "Ghost can't tell whose Ghost process {} is (its environment doesn't say which \
             folders it uses), so nothing was stopped or removed. Quit it, then run ghostd \
             uninstall again.",
            pids.join(", ")
        ));
    }
    if ghost_api::deps::unit_active(leftovers::DAEMON_UNIT) {
        let _ = writeln!(out, "Stopping Ghost's service…");
        let _ = hands.systemctl(&["stop", leftovers::DAEMON_UNIT]);
    }
    if found.is_empty() {
        return Ok(());
    }
    if found.iter().any(|p| p.app) {
        let _ = writeln!(out, "Quitting the Ghost app…");
        for p in found.iter().filter(|p| p.app) {
            term(p.pid, data);
        }
        // The app's Quit stops its daemon within 20 s (ADR 0038).
        wait_gone(data, skip, Duration::from_secs(25));
    }
    let (left, _) = ghost_processes(data, skip);
    if !left.is_empty() {
        let _ = writeln!(out, "Stopping Ghost…");
        for p in &left {
            term(p.pid, data);
        }
    }
    let left = wait_gone(data, skip, Duration::from_secs(10));
    if left.is_empty() {
        return Ok(());
    }
    let pids: Vec<String> = left.iter().map(|p| p.pid.to_string()).collect();
    Err(format!(
        "Ghost is still running (process {}), so nothing was removed. Quit it from the tray \
         (or stop it), then run ghostd uninstall again.",
        pids.join(", ")
    ))
}

// --- the command ---------------------------------------------------------------------------

/// Bytes the way a person reads them.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64 / 1000.0;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn shown(p: &Places, path: &Path) -> String {
    ghost_core::other_tools::read::shown_path(&p.home, path)
}

/// What is there, by category: (category, size, items there).
pub fn present(items: &[Item]) -> Vec<(Category, u64, Vec<&Item>)> {
    Category::ALL
        .into_iter()
        .map(|c| {
            let there: Vec<&Item> = items
                .iter()
                .filter(|i| i.category == c && i.exists)
                .collect();
            (c, there.iter().map(|i| i.size).sum(), there)
        })
        .collect()
}

/// The list, as the command prints it.
pub fn render(p: &Places, items: &[Item]) -> Vec<String> {
    let mut lines = vec!["Ghost on this computer:".to_string()];
    for (n, (c, size, there)) in present(items).into_iter().enumerate() {
        lines.push(String::new());
        if there.is_empty() {
            lines.push(format!("{}. {}: nothing here", n + 1, c.title()));
            continue;
        }
        lines.push(format!("{}. {} ({})", n + 1, c.title(), human(size)));
        lines.push(format!("   {}", c.sentence()));
        if let Some(w) = c.warning() {
            lines.push(format!("   {w}"));
        }
        for it in there {
            let paths: Vec<String> = it
                .paths
                .iter()
                .filter(|q| std::fs::symlink_metadata(q).is_ok())
                .map(|q| shown(p, q))
                .collect();
            let size = if it.paths.is_empty() {
                String::new()
            } else {
                format!(", {}", human(it.size))
            };
            lines.push(format!("   - {}{size}", it.what));
            for q in paths {
                lines.push(format!("       {q}"));
            }
            if let Some(note) = &it.note {
                lines.push(format!("       {note}"));
            }
        }
    }
    lines
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

/// The report after removing.
pub fn report(outcomes: &[Outcome]) -> Vec<String> {
    let mut lines = Vec::new();
    for o in outcomes {
        for s in &o.said {
            lines.push(format!("  {s}"));
        }
        if let Some(cmd) = &o.run_yourself {
            lines.push(format!("  run this yourself: {cmd}"));
        }
    }
    let failed: Vec<&Outcome> = outcomes.iter().filter(|o| !o.done).collect();
    if failed.is_empty() {
        lines.push("Done.".into());
    } else {
        lines.push(format!(
            "Done, but {} could not be removed; see above.",
            failed
                .iter()
                .map(|o| o.what.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    lines
}

/// The options the command takes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    pub dry_run: bool,
    pub all: bool,
    pub keep_data: bool,
    pub yes: bool,
    /// The app's choice (`--only running,app-storage`).
    pub only: Option<Vec<Category>>,
    /// The app's process, waited for before anything is stopped (`--after <pid>`).
    pub after: Option<i32>,
    /// The AppImage the app runs from, for the line that finishes (`--appimage <file>`,
    /// only with `--after`): the app's copy of `ghostd` is outside the mount and cannot
    /// tell by itself.
    pub appimage: Option<PathBuf>,
}

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" | "-n" => o.dry_run = true,
            "--all" => o.all = true,
            "--keep-data" => o.keep_data = true,
            "--yes" | "-y" => o.yes = true,
            "--only" => {
                let v = it.next().ok_or("--only takes a list")?;
                let cats: Option<Vec<Category>> = v.split(',').map(Category::parse).collect();
                o.only = Some(cats.ok_or_else(|| format!("--only: not a kind: {v}"))?);
            }
            "--after" => {
                let v = it.next().ok_or("--after takes a process id")?;
                o.after = Some(
                    v.parse()
                        .map_err(|_| format!("--after: not a process id: {v}"))?,
                );
            }
            "--appimage" => {
                let v = it.next().ok_or("--appimage takes a file")?;
                if !Path::new(v).is_absolute() {
                    return Err(format!("--appimage: not a full path: {v}"));
                }
                o.appimage = Some(PathBuf::from(v));
            }
            "-h" | "--help" | "help" => return Err(USAGE.into()),
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
    }
    if [o.all, o.keep_data, o.only.is_some()]
        .iter()
        .filter(|b| **b)
        .count()
        > 1
    {
        return Err(format!(
            "choose one of --all, --keep-data and --only\n{USAGE}"
        ));
    }
    if o.appimage.is_some() && o.after.is_none() {
        return Err(format!(
            "--appimage is the app's own, with --after\n{USAGE}"
        ));
    }
    Ok(o)
}

/// What the command needs of the computer, so a test can give it a temp home.
pub struct World<'a> {
    pub places: Places,
    pub look: &'a dyn Look,
    pub hands: &'a dyn Hands,
    pub finish: Finish,
    /// Stops Ghost (`stop_ghost`), or a test's stand-in.
    pub stop: &'a dyn Fn(&mut dyn Write) -> Result<(), String>,
}

/// `ghostd uninstall …` over `world`. Returns the exit code.
pub fn run(
    args: &[String],
    world: &World<'_>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            let _ = writeln!(out, "{e}");
            return 2;
        }
    };
    let p = &world.places;
    if !leftovers::home_is_usable(&p.home) {
        let _ = writeln!(
            out,
            "Your home folder is not set (HOME), so Ghost removes nothing."
        );
        return 1;
    }
    if let Some(pid) = o.after {
        // The app quits first; give it a minute.
        let start = Instant::now();
        while Path::new(&format!("/proc/{pid}")).exists()
            && start.elapsed() < Duration::from_secs(60)
        {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let items = leftovers::inventory(p, world.look);
    for line in render(p, &items) {
        let _ = writeln!(out, "{line}");
    }
    let there: Vec<Category> = present(&items)
        .into_iter()
        .filter(|(_, _, t)| !t.is_empty())
        .map(|(c, _, _)| c)
        .collect();
    let _ = writeln!(out);
    if o.dry_run {
        let _ = writeln!(
            out,
            "Nothing was removed (--dry-run). To finish after removing these: {}",
            world.finish.line
        );
        return 0;
    }
    if there.is_empty() {
        let _ = writeln!(
            out,
            "Ghost left nothing in your folders. To finish: {}",
            world.finish.line
        );
        return 0;
    }
    let chosen: Vec<Category> = if let Some(only) = &o.only {
        only.clone()
    } else if o.all {
        Category::ALL.to_vec()
    } else if o.keep_data {
        leftovers::all_but_data()
    } else {
        // One question per kind that is there, the person's own last, each No by default.
        let mut chosen = Vec::new();
        for c in &there {
            if let Some(w) = c.warning() {
                let _ = writeln!(out, "{w}");
            }
            if ask(input, out, &format!("Remove {}?", c.title().to_lowercase())) {
                chosen.push(*c);
            }
        }
        chosen
    };
    let chosen: Vec<Category> = chosen.into_iter().filter(|c| there.contains(c)).collect();
    if chosen.is_empty() {
        let _ = writeln!(out, "Nothing chosen; nothing was removed.");
        return 0;
    }
    if o.only.is_some() || o.all || o.keep_data {
        let names: Vec<&str> = chosen.iter().map(|c| c.title()).collect();
        let _ = writeln!(out, "Ghost will remove: {}.", names.join("; "));
        for c in &chosen {
            if let Some(w) = c.warning() {
                let _ = writeln!(out, "{w}");
            }
        }
        if !o.yes && !ask(input, out, "Remove all of this?") {
            let _ = writeln!(out, "Nothing was removed.");
            return 0;
        }
    }
    if let Err(why) = (world.stop)(out) {
        let _ = writeln!(out, "{why}");
        return 1;
    }
    // Read again: stopping Ghost may have written its last lines.
    let items = leftovers::inventory(p, world.look);
    let outcomes = leftovers::remove(p, &items, &chosen, world.hands);
    for line in report(&outcomes) {
        let _ = writeln!(out, "{line}");
    }
    let kept: Vec<&str> = there
        .iter()
        .filter(|c| !chosen.contains(c))
        .map(|c| c.title())
        .collect();
    if !kept.is_empty() {
        let _ = writeln!(out, "Kept: {}.", kept.join("; "));
    }
    let _ = writeln!(out, "To finish removing Ghost: {}", world.finish.line);
    if outcomes.iter().all(|o| o.done) {
        0
    } else {
        1
    }
}

/// The command, for real.
pub fn cli(args: &[String]) -> i32 {
    let places = match places_now() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ghostd uninstall: {e}");
            return 1;
        }
    };
    let o = parse(args).unwrap_or_default();
    let mut skip = vec![std::process::id() as i32];
    skip.extend(o.after);
    let hands = RealHands;
    let data = places.ghost_data.clone();
    let stop = move |out: &mut dyn Write| stop_ghost(&data, &RealHands, &skip, out);
    let world = World {
        places,
        look: &KeyringLook,
        hands: &hands,
        finish: finish_now(o.appimage.as_deref()),
        stop: &stop,
    };
    let stdin = std::io::stdin();
    let code = run(args, &world, &mut stdin.lock(), &mut std::io::stdout());
    if o.after.is_some() {
        notify(code, &world.finish);
        // The app ran a copy of ghostd from a folder of its own (`ghost-uninstall`, under the
        // runtime folder): it goes too.
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            && dir.file_name().is_some_and(|n| n == "ghost-uninstall")
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    code
}

/// After the app's removal: a desktop notification, when the desktop has `notify-send`.
fn notify(code: i32, finish: &Finish) {
    let (title, body) = if code == 0 {
        ("Ghost was removed", format!("To finish: {}", finish.line))
    } else {
        (
            "Ghost could not remove everything",
            "Run ghostd uninstall --dry-run in a terminal to see what is left.".to_string(),
        )
    };
    if let Some(bin) = RealHands.program("notify-send") {
        let _ = child(&bin.display().to_string())
            .args([title, &body])
            .output();
    }
}

#[cfg(test)]
#[path = "uninstall_tests.rs"]
mod tests;
