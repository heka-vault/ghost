//! Every write of the operator's `policy.toml` goes through here (ADR 0070): one lock, the
//! file read again under it, and an atomic replace.
//!
//! Three writers edit the file: `PUT /permissions` (Settings › Permissions and an app's
//! page), `PUT /policy` (the TOML view and the routes) and a decision made to stick
//! (`permission_choices`). Each read the file, changed it and wrote it back on its own, so
//! two at once could lose one's change. Now each holds the lock from its read to its write:
//! a mutex in this process and an advisory `flock` on `<file>.lock` beside it, so the
//! daemon and a `ghostd` command never interleave either. The edit is handed the file as it
//! is under the lock, so a second writer builds on the first's result (both changes land) or
//! refuses cleanly; never a lost rule.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rustix::fs::FlockOperation;

/// One writer at a time in this process.
static WRITING: Mutex<()> = Mutex::new(());

/// The file beside `policy.toml` that the advisory lock is taken on: never the file itself,
/// which is replaced by a rename (a lock on the old inode would stop nothing).
pub fn lock_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "policy.toml".into());
    path.with_file_name(format!("{name}.lock"))
}

/// Held from the read to the write; both locks go when it drops.
pub struct Locked {
    _file: fs::File,
    _guard: MutexGuard<'static, ()>,
}

/// Take the lock for writing `path`, waiting for whoever holds it.
pub fn lock(path: &Path) -> std::io::Result<Locked> {
    let guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(path))?;
    rustix::fs::flock(&file, FlockOperation::LockExclusive)?;
    Ok(Locked {
        _file: file,
        _guard: guard,
    })
}

/// The file's text and whether it was there ("" when not yet).
pub fn read(path: &Path) -> std::io::Result<(String, bool)> {
    match fs::read_to_string(path) {
        Ok(t) => Ok((t, true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((String::new(), false)),
        Err(e) => Err(e),
    }
}

/// Write `text` over `path` all at once: a temporary file beside it, synced, renamed over
/// it with the original's mode. Call it holding the lock.
pub fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mode = fs::metadata(path).ok().map(|m| m.permissions());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        if let Some(mode) = &mode {
            fs::set_permissions(&tmp, mode.clone())?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Read, change and write the file under the lock. `change` gets the file as it is now and
/// whether it exists, and returns the new text (None: nothing to write) or its own error.
pub fn edit<E: From<std::io::Error>>(
    path: &Path,
    change: impl FnOnce(&str, bool) -> Result<Option<String>, E>,
) -> Result<Option<String>, E> {
    let _held = lock(path)?;
    let (text, existed) = read(path)?;
    let Some(out) = change(&text, existed)? else {
        return Ok(None);
    };
    replace(path, &out)?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::Rule;
    use crate::policy::{AutonomyPolicy, write_permissions};
    use std::sync::{Arc, Barrier};

    /// Add `rule` to `list` the way `PUT /permissions` does: the whole table rendered again
    /// from the file as read under the lock.
    fn add_like_put_permissions(path: &Path, rule: &str, list: &str) {
        edit::<std::io::Error>(path, |text, _| {
            let p = AutonomyPolicy::parse(text, "t").unwrap();
            let perms = p
                .permissions
                .with_rule(&Rule::parse(rule).unwrap(), Some(list))
                .unwrap();
            Ok(Some(write_permissions(text, &perms).unwrap()))
        })
        .unwrap();
    }

    #[test]
    fn interleaved_writers_keep_both_rules() {
        // Many writers of both kinds at once, from threads: each change lands, none is lost.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.toml");
        fs::write(&path, "# mine\n[permissions]\nmode = \"ask\"\n").unwrap();
        let n = 12;
        let start = Arc::new(Barrier::new(n));
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let path = path.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    let rule = format!("app{i}.tool");
                    if i % 2 == 0 {
                        add_like_put_permissions(&path, &rule, "block");
                    } else {
                        crate::permission_choices::add_rule(
                            &path,
                            crate::permission_choices::Choice::Allow,
                            &Rule::parse(&rule).unwrap(),
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let p = AutonomyPolicy::load(&path).unwrap();
        for i in 0..n {
            let rule = Rule::parse(&format!("app{i}.tool")).unwrap();
            let list = if i % 2 == 0 { "block" } else { "allow" };
            assert!(
                p.permissions.list(list).contains(&rule),
                "app{i}.tool lost: {}",
                fs::read_to_string(&path).unwrap()
            );
        }
        assert_eq!(p.permissions.mode, crate::permissions::Mode::Ask);
    }

    #[test]
    fn a_writer_waits_for_the_one_holding_the_lock_and_builds_on_its_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.toml");
        fs::write(&path, "[permissions]\n").unwrap();
        let held = lock(&path).unwrap();
        let p2 = path.clone();
        let other = std::thread::spawn(move || add_like_put_permissions(&p2, "b.two", "block"));
        std::thread::sleep(std::time::Duration::from_millis(100));
        // The other writer is waiting: the first writes under its lock, then lets go.
        assert!(!other.is_finished());
        replace(&path, "[permissions]\nallow = [\"a.one\"]\n").unwrap();
        drop(held);
        other.join().unwrap();
        let p = AutonomyPolicy::load(&path).unwrap();
        assert_eq!(p.permissions.allow, vec![Rule::parse("a.one").unwrap()]);
        assert_eq!(p.permissions.block, vec![Rule::parse("b.two").unwrap()]);
    }

    #[test]
    fn the_lock_is_beside_the_file_and_a_refused_edit_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.toml");
        fs::write(&path, "x = 1\n").unwrap();
        let r = edit::<std::io::Error>(&path, |_, _| {
            Err(std::io::Error::other(
                "the file changed since you opened it",
            ))
        });
        assert!(r.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "x = 1\n");
        assert!(lock_path(&path).ends_with("policy.toml.lock"));
    }
}
