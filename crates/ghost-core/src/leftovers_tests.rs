use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;

fn places(home: &Path) -> Places {
    let mut env = BTreeMap::new();
    env.insert(
        "XDG_RUNTIME_DIR".to_string(),
        home.join("rt").display().to_string(),
    );
    Places::standard(home, &env)
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[derive(Default)]
struct Recorded {
    calls: Mutex<Vec<String>>,
    key: Mutex<bool>,
}

impl Hands for Recorded {
    fn systemctl(&self, args: &[&str]) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("systemctl --user {}", args.join(" ")));
        Ok(())
    }
    fn program(&self, _name: &str) -> Option<PathBuf> {
        None
    }
    fn run(&self, argv: &[String]) -> Result<(), String> {
        self.calls.lock().unwrap().push(argv.join(" "));
        Ok(())
    }
    fn delete_keyring_key(&self) -> Result<bool, String> {
        let mut k = self.key.lock().unwrap();
        let was = *k;
        *k = false;
        Ok(was)
    }
}

/// A home as the 2026.11.1 container left it (#147's inventory), and a file the person made.
fn used_home(home: &Path) {
    let h = |rel: &str| home.join(rel);
    write(
        &h(".config/autostart/heka-ghost-desktop.desktop"),
        b"[Desktop Entry]\n",
    );
    write(
        &h(".config/systemd/user/ghost-echo-cancel.service"),
        b"[Unit]\n",
    );
    std::os::unix::fs::symlink(h(".config/systemd/user/ghost-echo-cancel.service"), {
        std::fs::create_dir_all(h(".config/systemd/user/default.target.wants")).unwrap();
        h(".config/systemd/user/default.target.wants/ghost-echo-cancel.service")
    })
    .unwrap();
    write(&h(".config/ghost/echo-cancel.conf"), b"x");
    write(&h(".config/ghost/agents/ghost/agent.json"), b"{}");
    write(
        &h(".config/ghost/agents/ghost/memory.md"),
        b"the person's words",
    );
    write(&h(".local/share/ghost/ghost.db"), &[0; 4096]);
    write(&h(".local/share/ghost/ghost.db-wal"), b"w");
    write(&h(".local/share/ghost/passwords.vault"), b"sealed");
    write(&h(".local/share/ghost/passwords.vault.lock"), b"");
    write(&h(".local/share/ghost/clients/cursor.token"), b"t");
    write(&h(".local/share/ghost/wake/takes/1.wav"), &[1; 100]);
    write(&h(".local/share/ghost/phone.json"), b"{}");
    write(
        &h(".local/share/dev.heka.ghost/localstorage/tauri_localhost_0.localstorage"),
        b"l",
    );
    write(
        &h(".cache/ghost/models/parakeet-tdt-0.6b-v3/encoder-model.onnx"),
        &[2; 1000],
    );
    write(&h(".cursor/mcp.json"), br#"{"mcpServers":{"fetch":{"command":"uvx"},"ghost":{"type":"stdio","command":"/usr/bin/ghostd","args":["mcp","serve","--client","cursor"]}}}"#);
    write(
        &h(".cursor/mcp.json.ghost-backup-20261114T225845"),
        br#"{"mcpServers":{"fetch":{"command":"uvx"}}}"#,
    );
    write(&h("rt/ghost/api.sock"), b"");
    write(&h("notes.txt"), b"a file the person made");
}

fn files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                walk(root, &p, out);
            } else {
                out.push(p.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    walk(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn the_inventory_finds_what_a_used_install_left_with_sizes() {
    let tmp = tempfile::tempdir().unwrap();
    used_home(tmp.path());
    let items = inventory(&places(tmp.path()), &FilesOnly);
    let get = |k: &str| items.iter().find(|i| i.key == k).unwrap();
    for k in [
        "echo-cancel",
        "autostart",
        "in-cursor",
        "client-keys",
        "speech-models",
        "app-storage",
        "ghost-folder",
        "database",
        "wake-takes",
        "data-folder",
        "vault",
    ] {
        assert!(get(k).exists, "{k}");
    }
    for k in [
        "gnome-shell-helper",
        "headless-service",
        "in-codex",
        "hearing-test",
    ] {
        assert!(!get(k).exists, "{k}");
    }
    assert_eq!(get("speech-models").size, 1000);
    assert_eq!(get("database").size, 4097);
    // The data folder counts only what no other item takes: the phone's file.
    assert_eq!(get("data-folder").size, 2);
    assert_eq!(get("vault").size, 6);
    assert_eq!(get("wake-takes").size, 100);
    assert!(get("wake-takes").what.contains("recordings of your voice"));
    // Every category is there, in order, the person's own asked last.
    let cats: Vec<Category> = items.iter().map(|i| i.category).collect();
    let mut sorted = cats.clone();
    sorted.sort();
    assert_eq!(cats, sorted);
    assert_eq!(Category::ALL.last(), Some(&Category::Vault));
}

#[test]
fn removing_everything_leaves_only_what_the_person_made() {
    let tmp = tempfile::tempdir().unwrap();
    used_home(tmp.path());
    let p = places(tmp.path());
    let items = inventory(&p, &FilesOnly);
    let hands = Recorded::default();
    *hands.key.lock().unwrap() = true;
    let out = remove(&p, &items, &Category::ALL, &hands);
    assert!(out.iter().all(|o| o.done), "{out:#?}");
    let left = files(tmp.path());
    assert_eq!(
        left,
        vec![".cursor/mcp.json".to_string(), "notes.txt".to_string()],
        "{out:#?}"
    );
    // Cursor's file keeps its own entry and loses Ghost's; no backup is left beside it.
    let cursor = std::fs::read_to_string(tmp.path().join(".cursor/mcp.json")).unwrap();
    assert!(
        cursor.contains("fetch") && !cursor.contains("ghost"),
        "{cursor}"
    );
    let calls = hands.calls.lock().unwrap().clone();
    assert!(calls.contains(&"systemctl --user disable --now ghost-echo-cancel.service".into()));
    assert_eq!(calls.last().unwrap(), "systemctl --user daemon-reload");
    assert!(!*hands.key.lock().unwrap());
    // Twice is the same as once.
    let again = remove(&p, &inventory(&p, &FilesOnly), &Category::ALL, &hands);
    assert!(
        again.iter().all(|o| o.done && o.said.is_empty()),
        "{again:#?}"
    );
}

#[test]
fn keep_data_leaves_exactly_the_data_and_the_vault() {
    let tmp = tempfile::tempdir().unwrap();
    used_home(tmp.path());
    let p = places(tmp.path());
    let items = inventory(&p, &FilesOnly);
    let hands = Recorded::default();
    *hands.key.lock().unwrap() = true;
    remove(&p, &items, &all_but_data(), &hands);
    assert_eq!(
        files(tmp.path()),
        vec![
            ".config/ghost/agents/ghost/agent.json",
            ".config/ghost/agents/ghost/memory.md",
            ".cursor/mcp.json",
            ".local/share/ghost/ghost.db",
            ".local/share/ghost/ghost.db-wal",
            ".local/share/ghost/passwords.vault",
            ".local/share/ghost/passwords.vault.lock",
            ".local/share/ghost/phone.json",
            ".local/share/ghost/wake/takes/1.wav",
            "notes.txt",
        ]
    );
    assert!(*hands.key.lock().unwrap(), "the keyring's key stays");
}

#[test]
fn the_data_without_the_vault_keeps_the_vault() {
    let tmp = tempfile::tempdir().unwrap();
    used_home(tmp.path());
    let p = places(tmp.path());
    remove(
        &p,
        &inventory(&p, &FilesOnly),
        &[Category::Data],
        &Recorded::default(),
    );
    let left = files(tmp.path());
    assert!(
        left.contains(&".local/share/ghost/passwords.vault".to_string()),
        "{left:?}"
    );
    assert!(
        !left
            .iter()
            .any(|f| f.contains("ghost.db") || f.contains("memory.md")),
        "{left:?}"
    );
}

#[test]
fn nothing_outside_the_home_folder_or_through_a_link_out_of_it_is_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("precious/file"), b"keep");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".local/share")).unwrap();
    // ~/.cache is a link to a folder outside home: Ghost's cache under it is left.
    std::os::unix::fs::symlink(outside.path(), home.join(".cache")).unwrap();
    write(&outside.path().join("ghost/models/m.onnx"), b"m");
    // The data folder moved outside home by the settings.
    let mut p = places(&home);
    p.ghost_data = outside.path().join("precious");
    let items = inventory(&p, &FilesOnly);
    let out = remove(&p, &items, &Category::ALL, &Recorded::default());
    assert!(outside.path().join("precious/file").exists());
    assert!(outside.path().join("ghost/models/m.onnx").exists());
    let said: Vec<&String> = out.iter().flat_map(|o| &o.said).collect();
    assert!(
        said.iter()
            .any(|s| s.contains("not inside your home folder")),
        "{said:?}"
    );
    assert!(
        said.iter().any(|s| s.contains("through a link")),
        "{said:?}"
    );
    // A link inside a folder Ghost removes is removed as a link, its target kept.
    write(&home.join(".local/share/dev.heka.ghost/x"), b"x");
    std::os::unix::fs::symlink(outside.path(), home.join(".local/share/dev.heka.ghost/out"))
        .unwrap();
    remove(
        &p,
        &inventory(&p, &FilesOnly),
        &[Category::AppStorage],
        &Recorded::default(),
    );
    assert!(!home.join(".local/share/dev.heka.ghost").exists());
    assert!(outside.path().join("precious/file").exists());
}

#[test]
fn an_empty_or_root_home_removes_nothing() {
    for home in ["", "/", "relative"] {
        let p = Places::standard(Path::new(home), &BTreeMap::new());
        let out = remove(
            &p,
            &inventory(&p, &FilesOnly),
            &Category::ALL,
            &Recorded::default(),
        );
        assert_eq!(out.len(), 1, "{home:?}");
        assert!(
            !out[0].done && out[0].said[0].contains("home folder"),
            "{home:?}"
        );
        assert!(check_path(&p, Path::new("/etc/passwd")).is_err());
    }
}

#[test]
fn claude_code_without_its_program_is_a_command_for_the_person() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write(
        &home.join(".claude.json"),
        br#"{"mcpServers":{"ghost":{"type":"stdio","command":"/usr/bin/ghostd","args":["mcp","serve","--client","claude-code"]}}}"#,
    );
    let mut p = places(home);
    p.ports = vec![47824];
    let items = inventory(&p, &FilesOnly);
    let it = items.iter().find(|i| i.key == "in-claude-code").unwrap();
    // On a machine with `claude` on the PATH the test cannot say which; only without it.
    if other_tools::program::which(std::env::var("PATH").ok().as_deref()).is_none() {
        assert!(it.exists);
    }
    let out = remove(&p, &items, &[Category::OtherTools], &Recorded::default());
    if let Some(o) = out
        .iter()
        .find(|o| o.key == "in-claude-code" && o.run_yourself.is_some())
    {
        assert_eq!(
            o.run_yourself.as_deref(),
            Some("claude mcp remove --scope user ghost")
        );
    }
    // Ghost never writes ~/.claude.json.
    assert!(
        std::fs::read_to_string(home.join(".claude.json"))
            .unwrap()
            .contains("ghost")
    );
}

/// Every folder under a person's home that Ghost's source names, by its first two parts
/// (`.config/ghost`, `.local/share/dev.heka.ghost`), is either in the inventory or named
/// here as one Ghost reads and never writes, with why.
const READ_NOT_WRITTEN: &[(&str, &str)] = &[
    (
        ".config/Cursor",
        "Cursor's own sign-in, refused by other_tools::read",
    ),
    (
        ".config/claude",
        "Claude Code's own folder, refused by other_tools::read",
    ),
    (
        ".config/heka-vault",
        "the operator's old env file, read once by `ghostd passwords import`",
    ),
    (
        ".cache/huggingface",
        "a Parakeet snapshot another program downloaded, read if there",
    ),
    (
        ".config/systemd",
        "named only by the units' own paths, in the inventory as files",
    ),
    (
        ".local/state",
        "the XDG state root; Ghost's is .local/state/ghost",
    ),
    (".local/share", "the XDG data root"),
];

#[test]
fn every_folder_ghost_names_in_its_source_is_in_the_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pattern =
        regex::Regex::new(r#"["'/ ]\.(config|cache|local/share|local/state)/([A-Za-z0-9_.@-]+)"#)
            .unwrap();
    // `home.join(".cache").join("ghost")`, the same folder written in two calls.
    let split = regex::Regex::new(
        r#"join\("\.(config|cache|local/share|local/state)"\)\s*\.join\("([A-Za-z0-9_.@-]+)"\)"#,
    )
    .unwrap();
    let mut named = std::collections::BTreeSet::new();
    let mut sources = Vec::new();
    for dir in [
        "crates/ghost-core/src",
        "crates/ghost-api/src",
        "crates/ghost-audio/src",
        "crates/ghost-infer/src",
        "crates/ghost-llm/src",
        "crates/ghost-reports/src",
        "crates/ghost-voice/src",
        "crates/ghostd/src",
        "apps/desktop/src-tauri/src",
    ] {
        collect(&root.join(dir), &mut sources);
    }
    sources.push(root.join("infra/pipewire/install.sh"));
    sources.push(root.join("apps/desktop/gnome-shell/install.sh"));
    for file in &sources {
        if file.ends_with("leftovers.rs") || file.ends_with("leftovers_tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap();
        for c in pattern
            .captures_iter(&text)
            .chain(split.captures_iter(&text))
        {
            named.insert(format!(".{}/{}", &c[1], &c[2]));
        }
        for (call, folder) in [
            ("app_config_dir", ".config/dev.heka.ghost"),
            ("app_data_dir", ".local/share/dev.heka.ghost"),
            ("app_local_data_dir", ".local/share/dev.heka.ghost"),
            ("app_cache_dir", ".cache/dev.heka.ghost"),
            ("app_log_dir", ".local/share/dev.heka.ghost"),
        ] {
            if text.contains(call) {
                named.insert(folder.to_string());
            }
        }
    }
    assert!(
        named.contains(".config/ghost") && named.contains(".cache/ghost"),
        "{named:?}"
    );
    let home = Path::new("/home/person");
    let p = places(home);
    let covered: Vec<PathBuf> = inventory(&p, &FilesOnly)
        .into_iter()
        .flat_map(|i| i.paths)
        .collect();
    for n in &named {
        let path = home.join(n);
        let inside = covered
            .iter()
            .any(|c| c.starts_with(&path) || path.starts_with(c));
        let excused = READ_NOT_WRITTEN.iter().any(|(r, _)| n == r);
        assert!(
            inside || excused,
            "{n} is named in Ghost's source but is not in leftovers::inventory: add it, or \
             to READ_NOT_WRITTEN with why"
        );
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn the_echo_cancellers_paths_are_the_installers() {
    // The installer and the inventory name the same files.
    let script = include_str!("../../../infra/pipewire/install.sh");
    let p = places(Path::new("/home/person"));
    for path in p.echo_cancel_paths() {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name == ECHO_UNIT {
            assert!(script.contains("UNIT=\"ghost-echo-cancel.service\""));
        } else {
            assert!(script.contains(name), "{name}");
        }
    }
}

#[test]
fn the_categories_parse_and_say_what_cannot_be_had_back() {
    for c in Category::ALL {
        assert_eq!(Category::parse(c.id()), Some(c));
    }
    assert!(
        Category::Vault
            .warning()
            .unwrap()
            .contains("cannot be recovered")
    );
    assert!(
        Category::Data
            .warning()
            .unwrap()
            .contains("recordings of your voice")
    );
    assert_eq!(all_but_data().len(), 4);
}

#[test]
fn the_by_hand_list_names_the_two_paths_the_guide_missed() {
    let list: Vec<String> = by_hand().into_iter().map(|(_, p, _)| p).collect();
    for want in [
        "~/.config/autostart/heka-ghost-desktop.desktop",
        "~/.local/share/dev.heka.ghost",
        "~/.cache/dev.heka.ghost",
        "~/.local/state/ghost",
        "$XDG_RUNTIME_DIR/ghost",
        "~/.cursor/mcp.json.ghost-backup-*",
    ] {
        assert!(list.iter().any(|p| p == want), "{want}: {list:?}");
    }
}

#[test]
fn the_readme_lists_every_path() {
    // The README's by-hand list, for someone who removed the package first, names every path
    // the inventory does, so the two cannot drift.
    let readme = include_str!("../../../README.md");
    let start = readme
        .find("<!-- uninstall-by-hand")
        .expect("the README's by-hand list");
    let end = readme.find("<!-- /uninstall-by-hand -->").expect("its end");
    let list = &readme[start..end];
    for (_, path, what) in by_hand() {
        assert!(
            list.contains(&format!("`{path}`")),
            "README's Uninstall does not name `{path}` ({what})"
        );
    }
    assert!(list.contains("secret-tool clear application ghost ghost passwords-vault-key"));
}
