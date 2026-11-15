use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ghost_core::leftovers::FilesOnly;

use super::*;

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
}

impl Hands for Fake {
    fn systemctl(&self, args: &[&str]) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("systemctl {}", args.join(" ")));
        Ok(())
    }
    fn program(&self, _: &str) -> Option<PathBuf> {
        None
    }
    fn run(&self, argv: &[String]) -> Result<(), String> {
        self.calls.lock().unwrap().push(argv.join(" "));
        Ok(())
    }
    fn delete_keyring_key(&self) -> Result<bool, String> {
        Ok(false)
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let h = |rel: &str| tmp.path().join(rel);
    write(
        &h(".config/autostart/heka-ghost-desktop.desktop"),
        b"[Desktop Entry]\n",
    );
    write(&h(".config/ghost/agents/ghost/memory.md"), b"m");
    write(&h(".local/share/ghost/ghost.db"), &[0; 2048]);
    write(&h(".local/share/ghost/passwords.vault"), b"v");
    write(&h(".local/share/dev.heka.ghost/storage/salt"), b"s");
    write(&h(".cache/ghost/models/m.onnx"), &[1; 3_000_000]);
    tmp
}

fn places(home: &Path) -> Places {
    Places::standard(home, &BTreeMap::new())
}

fn go(home: &Path, args: &[&str], typed: &str, stop_ok: bool) -> (i32, String) {
    let hands = Fake::default();
    let stop = move |_: &mut dyn Write| {
        if stop_ok {
            Ok(())
        } else {
            Err("Ghost is still running (process 42), so nothing was removed.".into())
        }
    };
    let world = World {
        places: places(home),
        look: &FilesOnly,
        hands: &hands,
        finish: finish(None, Some(Path::new("/usr/bin/ghostd")), true),
        stop: &stop,
    };
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let code = run(&args, &world, &mut typed.as_bytes(), &mut out);
    (code, String::from_utf8(out).unwrap())
}

fn exists(home: &Path, rel: &str) -> bool {
    home.join(rel).exists()
}

#[test]
fn a_dry_run_lists_with_sizes_and_removes_nothing() {
    let tmp = home();
    let (code, said) = go(tmp.path(), &["--dry-run"], "", true);
    assert_eq!(code, 0, "{said}");
    for want in [
        "What keeps running or starts by itself",
        "~/.config/autostart/heka-ghost-desktop.desktop",
        "~/.local/share/dev.heka.ghost",
        "The speech models Ghost downloaded (3.0 MB)",
        "Every password saved in Ghost is deleted and cannot be recovered.",
        "Nothing was removed (--dry-run)",
        "sudo apt remove heka-ghost-desktop",
    ] {
        assert!(said.contains(want), "{want}\n{said}");
    }
    assert!(exists(
        tmp.path(),
        ".config/autostart/heka-ghost-desktop.desktop"
    ));
    assert!(exists(tmp.path(), ".cache/ghost/models/m.onnx"));
}

#[test]
fn each_kind_is_asked_and_no_is_the_default() {
    let tmp = home();
    // running: y; speech models: y; app storage: (enter); data: (enter); vault: n.
    let (code, said) = go(tmp.path(), &[], "y\ny\n\n\nn\n", true);
    assert_eq!(code, 0, "{said}");
    assert!(!exists(
        tmp.path(),
        ".config/autostart/heka-ghost-desktop.desktop"
    ));
    assert!(!exists(tmp.path(), ".cache/ghost"));
    assert!(exists(tmp.path(), ".local/share/dev.heka.ghost"));
    assert!(exists(tmp.path(), ".local/share/ghost/ghost.db"));
    assert!(exists(tmp.path(), ".local/share/ghost/passwords.vault"));
    // The person's own are asked last, each after its warning.
    let data = said.find("Remove your data?").unwrap();
    let vault = said.find("Remove your passwords?").unwrap();
    let models = said
        .find("Remove the speech models ghost downloaded?")
        .unwrap();
    assert!(models < data && data < vault, "{said}");
    assert!(said[..vault].contains("cannot be recovered"), "{said}");
    assert!(said.contains("Kept: "), "{said}");
}

#[test]
fn all_asks_once_and_no_keeps_everything() {
    let tmp = home();
    let (code, said) = go(tmp.path(), &["--all"], "n\n", true);
    assert_eq!(code, 0);
    assert_eq!(said.matches("[y/N]").count(), 1, "{said}");
    assert!(said.contains("Nothing was removed."));
    assert!(exists(tmp.path(), ".local/share/ghost/ghost.db"));
    let (code, said) = go(tmp.path(), &["--all", "--yes"], "", true);
    assert_eq!(code, 0, "{said}");
    assert!(!said.contains("[y/N]"));
    for rel in [
        ".config/ghost",
        ".local/share/ghost",
        ".cache/ghost",
        ".local/share/dev.heka.ghost",
    ] {
        assert!(!exists(tmp.path(), rel), "{rel}");
    }
    assert!(said.contains("To finish removing Ghost: sudo apt remove heka-ghost-desktop"));
}

#[test]
fn keep_data_leaves_the_data_and_the_vault() {
    let tmp = home();
    let (code, said) = go(tmp.path(), &["--keep-data", "--yes"], "", true);
    assert_eq!(code, 0, "{said}");
    assert!(exists(tmp.path(), ".config/ghost/agents/ghost/memory.md"));
    assert!(exists(tmp.path(), ".local/share/ghost/ghost.db"));
    assert!(exists(tmp.path(), ".local/share/ghost/passwords.vault"));
    assert!(!exists(tmp.path(), ".cache/ghost"));
    assert!(!exists(tmp.path(), ".local/share/dev.heka.ghost"));
}

#[test]
fn ghost_that_will_not_stop_removes_nothing() {
    let tmp = home();
    let (code, said) = go(tmp.path(), &["--all", "--yes"], "", false);
    assert_eq!(code, 1);
    assert!(said.contains("Ghost is still running"), "{said}");
    assert!(exists(tmp.path(), ".cache/ghost/models/m.onnx"));
}

#[test]
fn the_apps_choice_is_taken_as_given() {
    let tmp = home();
    let (code, said) = go(
        tmp.path(),
        &["--only", "app-storage,speech-models", "--yes"],
        "",
        true,
    );
    assert_eq!(code, 0, "{said}");
    assert!(!exists(tmp.path(), ".cache/ghost"));
    assert!(exists(
        tmp.path(),
        ".config/autostart/heka-ghost-desktop.desktop"
    ));
    assert!(parse(&["--only".into(), "everything".into()]).is_err());
    assert!(parse(&["--all".into(), "--keep-data".into()]).is_err());
}

#[test]
fn an_empty_home_refuses() {
    let (code, said) = go(Path::new(""), &["--all", "--yes"], "", true);
    assert_eq!(code, 1);
    assert!(said.contains("home folder is not set"), "{said}");
}

#[test]
fn the_finishing_line_names_the_package_or_the_appimage() {
    assert_eq!(
        finish(None, Some(Path::new("/usr/bin/ghostd")), true).line,
        "sudo apt remove heka-ghost-desktop"
    );
    let a = finish(
        Some(Path::new("/home/p/Apps/Ghost 1.AppImage")),
        None,
        false,
    );
    assert_eq!(
        (a.kind.as_str(), a.line.as_str()),
        ("appimage", "rm '/home/p/Apps/Ghost 1.AppImage'")
    );
    assert_eq!(
        finish(
            None,
            Some(Path::new("/home/p/ghost/target/debug/ghostd")),
            false
        )
        .kind,
        "checkout"
    );
}

#[test]
fn sizes_read_as_a_person_reads_them() {
    assert_eq!(human(512), "512 bytes");
    assert_eq!(human(2_800_000_000), "2.8 GB");
    assert_eq!(human(340_000_000), "340 MB");
}

#[test]
fn only_the_ghost_whose_data_is_removed_is_stopped() {
    let env = |pairs: &[&str]| pairs.join("\0").into_bytes();
    assert_eq!(
        data_dir_of(&env(&["PATH=/usr/bin", "HOME=/home/p"])),
        Some(PathBuf::from("/home/p/.local/share/ghost"))
    );
    assert_eq!(
        data_dir_of(&env(&["HOME=/home/p", "GHOST_DATA_DIR=/tmp/test-data"])),
        Some(PathBuf::from("/tmp/test-data"))
    );
    assert_eq!(data_dir_of(&env(&["PATH=/usr/bin"])), None);
    // Whose: the folder being removed, another's, or not known (never guessed).
    let data = Path::new("/home/p/.local/share/ghost");
    assert_eq!(whose(&env(&["HOME=/home/p"]), data), Whose::Ours);
    assert_eq!(whose(&env(&["HOME=/home/q"]), data), Whose::Theirs);
    assert_eq!(
        whose(&env(&["HOME=/home/p", "GHOST_DATA_DIR=/tmp/t"]), data),
        Whose::Theirs
    );
    assert_eq!(whose(&env(&["PATH=/usr/bin"]), data), Whose::Unknown);
    assert_eq!(whose(b"", data), Whose::Unknown);
    // The same folder spelled through a link is the same folder.
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
    let spelled = format!("GHOST_DATA_DIR={}", tmp.path().join("link").display());
    assert_eq!(whose(&env(&[spelled.as_str()]), &real), Whose::Ours);
    // Nothing on this machine uses a fresh temp folder.
    assert!(ghost_processes(tmp.path(), &[]).0.is_empty());
}
