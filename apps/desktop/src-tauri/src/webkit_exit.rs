//! The app's last step on its way out (issue #115): its own WebKit web-content processes
//! are ended with SIGKILL, once the daemon has stopped.
//!
//! Left to themselves, they see the app's connection close and `exit()`, which runs Mesa's
//! exit handlers; with software GL (llvmpipe under Xvfb, and the same `libgallium` on real
//! hardware) one quit in five to eight aborted there with `free(): corrupted unsorted
//! chunks`, a crash Ubuntu's reporter can show as a dialog after Ghost has already quit
//! cleanly. A killed process runs no exit handlers and leaves no core. The web-content
//! process keeps nothing: WebKit's storage (the window's `localStorage`) is in its network
//! process, which is left to exit on its own.

use std::path::Path;

/// The kernel's name for WebKit's web-content process: `WebKitWebProcess`, cut to the 15
/// characters `comm` keeps.
const WEB_PROCESS: &str = "WebKitWebProces";

/// The children of `parent` named `comm`, read from `proc_root` (`/proc`).
pub fn children_named(proc_root: &Path, parent: u32, comm: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut out: Vec<u32> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let dir = proc_root.join(pid.to_string());
            let named =
                std::fs::read_to_string(dir.join("comm")).is_ok_and(|c| c.trim_end() == comm);
            named && parent_of(&dir) == Some(parent)
        })
        .collect();
    out.sort_unstable();
    out
}

/// The fourth field of `/proc/<pid>/stat`, after the parenthesised name (which may hold
/// spaces and parentheses itself).
fn parent_of(dir: &Path) -> Option<u32> {
    let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

/// End this process's web-content children; how many were ended.
pub fn end_web_processes() -> usize {
    let me = std::process::id();
    let mut ended = 0;
    for pid in children_named(Path::new("/proc"), me, WEB_PROCESS) {
        if let Some(p) = rustix::process::Pid::from_raw(pid as i32)
            && rustix::process::kill_process(p, rustix::process::Signal::KILL).is_ok()
        {
            ended += 1;
        }
    }
    ended
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_entry(root: &Path, pid: u32, comm: &str, ppid: u32) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        std::fs::write(
            dir.join("stat"),
            format!("{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560 0 0"),
        )
        .unwrap();
    }

    #[test]
    fn only_this_apps_web_content_processes_are_ended() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        proc_entry(root, 100, "heka-ghost-desk", 1);
        proc_entry(root, 101, WEB_PROCESS, 100);
        proc_entry(root, 102, "WebKitNetworkPr", 100);
        proc_entry(root, 103, "ghostd", 100);
        proc_entry(root, 201, WEB_PROCESS, 200); // another app's
        proc_entry(root, 104, "a (weird) name", 100);
        std::fs::create_dir_all(root.join("self")).unwrap();
        assert_eq!(children_named(root, 100, WEB_PROCESS), vec![101]);
        assert_eq!(children_named(root, 100, "a (weird) name"), vec![104]);
        assert!(children_named(root, 999, WEB_PROCESS).is_empty());
    }
}
