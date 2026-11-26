//! The table, the text a program prints, and the jobs with fake programs: shell scripts in a
//! temp folder put first on the PATH, never the operator's own programs or accounts.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use zeroize::Zeroizing;

use super::jobs::Limits;
use super::text::{self, Screen};
use super::*;

const CLAUDE: &[&str] = &["claude.ai", "claude.com", "anthropic.com"];

#[test]
fn the_table_has_the_three_and_the_vendors_own_commands() {
    let ids: Vec<&str> = TABLE.iter().map(|s| s.id).collect();
    assert_eq!(ids, ["claude-code", "codex", "antigravity"]);
    for s in TABLE {
        assert_eq!(Sub::parse(s.id), Some(s.sub));
        assert!(
            s.install.starts_with("curl -fsSL https://"),
            "{}",
            s.install
        );
        assert!(!s.install.contains("sudo"), "{}", s.install);
        assert!(s.install_source.starts_with("https://"));
        assert!(!s.domains.is_empty());
    }
    assert_eq!(
        Sub::ClaudeCode.spec().install,
        "curl -fsSL https://claude.ai/install.sh | bash"
    );
    assert_eq!(
        Sub::Codex.spec().install,
        "curl -fsSL https://chatgpt.com/codex/install.sh | sh"
    );
    assert_eq!(
        Sub::Antigravity.spec().install,
        "curl -fsSL https://antigravity.google/cli/install.sh | bash"
    );
    assert_eq!(Sub::ClaudeCode.spec().provider, Some("claude-code"));
    assert_eq!(Sub::Codex.spec().provider, None);
    assert_eq!(Sub::parse("cursor"), None);
}

fn started(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn the_environment_carries_no_key_and_the_browser_only_for_a_sign_in() {
    let s = started(&[
        ("HOME", "/home/op"),
        ("PATH", "/usr/bin"),
        ("USER", "op"),
        ("ANTHROPIC_AUTH_TOKEN", "proxy-token-0123456789"),
        ("ANTHROPIC_API_KEY", "sk-ant-fixture"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "x"),
        ("OPENAI_API_KEY", "sk-fixture"),
        ("CODEX_API_KEY", "y"),
        ("CODEX_HOME", "/home/op/.codex-work"),
        ("GEMINI_API_KEY", "z"),
        ("GOOGLE_API_KEY", "w"),
        ("OPENROUTER_API_KEY", "v"),
        ("DISPLAY", ":0"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ]);
    for sub in [Sub::ClaudeCode, Sub::Codex, Sub::Antigravity] {
        for env in [base_env(sub, &s), sign_in_env(sub, &s)] {
            for k in env.keys() {
                assert!(
                    ![
                        "ANTHROPIC",
                        "CLAUDE_",
                        "OPENAI",
                        "GEMINI",
                        "GOOGLE",
                        "OPENROUTER"
                    ]
                    .iter()
                    .any(|p| k.starts_with(p))
                        && k != "CODEX_API_KEY",
                    "{sub:?}: {k}"
                );
            }
            assert_eq!(env["PATH"], "/usr/bin:/home/op/.local/bin");
        }
        assert!(!base_env(sub, &s).contains_key("DISPLAY"));
        let si = sign_in_env(sub, &s);
        assert_eq!(si["DISPLAY"], ":0");
        assert_eq!(
            si["DBUS_SESSION_BUS_ADDRESS"],
            "unix:path=/run/user/1000/bus"
        );
    }
    assert_eq!(
        base_env(Sub::Codex, &s)["CODEX_HOME"],
        "/home/op/.codex-work"
    );
    assert!(!base_env(Sub::ClaudeCode, &s).contains_key("CODEX_HOME"));
    assert_eq!(
        base_env(Sub::ClaudeCode, &s)["ENABLE_CLAUDEAI_MCP_SERVERS"],
        "false"
    );
    assert!(!base_env(Sub::Codex, &s).contains_key("ENABLE_CLAUDEAI_MCP_SERVERS"));
}

#[test]
fn escapes_and_controls_are_taken_out_and_a_hyperlink_gives_its_address() {
    let mut s = Screen::new(CLAUDE);
    s.feed(b"\x1b[1mOpening browser to sign in\xe2\x80\xa6\x1b[0m\r\n");
    s.feed(b"\x1b]8;;https://claude.ai/oauth/authorize?code=true&state=abc\x1b\\");
    s.feed(b"click here\x1b]8;;\x1b\\\n\x07\x08\x1b(B");
    s.feed(b"Paste code here if prompted > ");
    assert_eq!(s.lines(), ["Opening browser to sign in…", "click here"]);
    assert_eq!(
        s.url.as_deref(),
        Some("https://claude.ai/oauth/authorize?code=true&state=abc")
    );
    assert_eq!(s.pending(), "Paste code here if prompted >");
    assert!(text::asks_for_code(&s.pending()));
}

#[test]
fn an_address_is_kept_only_on_the_vendors_domains_and_shown_without_its_query() {
    let d = CLAUDE;
    let line = "visit: https://claude.ai/oauth/authorize?client_id=x&state=secret-state.";
    assert_eq!(
        text::sign_in_url(line, d).as_deref(),
        Some("https://claude.ai/oauth/authorize?client_id=x&state=secret-state")
    );
    assert_eq!(
        text::sign_in_url("https://console.anthropic.com/oauth?x=1", d).as_deref(),
        Some("https://console.anthropic.com/oauth?x=1")
    );
    for bad in [
        "http://claude.ai/oauth/authorize?x=1",
        "https://claude.ai.evil.example/oauth",
        "https://evilclaude.ai/oauth",
        "https://user:pw@claude.ai/oauth",
        "https://claude.ai:8443/oauth",
        "https://evil.example/?next=https//claude.ai",
        "https://127.0.0.1/oauth",
    ] {
        assert_eq!(text::sign_in_url(bad, d), None, "{bad}");
    }
    let mut s = Screen::new(d);
    s.feed(format!("{line}\n").as_bytes());
    let shown = s.lines().join("\n");
    assert!(!shown.contains("secret-state"), "{shown}");
    assert!(
        shown.contains("https://claude.ai/oauth/authorize…"),
        "{shown}"
    );
    assert!(s.url.as_deref().unwrap().contains("secret-state"));
    // Codex's own callback is http on loopback: not a link, and not hidden either.
    let mut c = Screen::new(&["openai.com", "chatgpt.com"]);
    c.feed(b"Starting local login server on http://localhost:1455.\n");
    c.feed(b"https://auth.openai.com/oauth/authorize?response_type=code&state=s1\n");
    assert_eq!(
        c.url.as_deref(),
        Some("https://auth.openai.com/oauth/authorize?response_type=code&state=s1")
    );
}

#[test]
fn a_prompt_for_a_code_is_noticed_and_other_lines_are_not() {
    for yes in [
        "Paste code here if prompted >",
        "Enter the authorization code:",
        "Paste the code from your browser",
        "Code?",
    ] {
        assert!(text::asks_for_code(yes), "{yes}");
    }
    for no in [
        "Opening browser to sign in…",
        "Successfully logged in",
        "If your browser did not open, navigate to this URL to authenticate:",
        "",
        "the code is on its way, please wait while we check",
    ] {
        assert!(!text::asks_for_code(no), "{no}");
    }
}

#[test]
fn a_device_code_is_shown_and_a_word_is_not() {
    assert_eq!(
        text::device_code("Enter this one-time code: WXYZ-1234").as_deref(),
        Some("WXYZ-1234")
    );
    assert_eq!(text::device_code("WXYZ-1234 with no word for it"), None);
    assert_eq!(text::device_code("Paste code here if prompted >"), None);
    assert_eq!(text::device_code("the code is well-known"), None);
}

#[test]
fn a_pasted_code_is_hidden_wherever_it_is_echoed() {
    let code = "abc123XYZ#state-9876";
    assert_eq!(
        text::hide_code(&format!("Paste code here if prompted > {code}"), code),
        format!("Paste code here if prompted > {}", text::HIDDEN)
    );
    // Drawn in two pieces by a program's own line editor.
    let out = text::hide_code("> abc123XYZ#st", code);
    assert!(!out.contains("abc123XYZ"), "{out}");
    let mut s = Screen::new(CLAUDE);
    s.hide(Zeroizing::new(code.into()));
    s.feed(format!("got {code}\nok\n").as_bytes());
    assert!(!s.lines().join("\n").contains("abc123"), "{:?}", s.lines());
    assert!(text::check_code("  ").is_err());
    assert!(text::check_code("a\nb").is_err());
    assert!(text::check_code(&"x".repeat(600)).is_err());
    assert!(text::check_code(code).is_ok());
}

#[test]
fn lines_are_bounded_and_a_redraw_is_one_line() {
    let mut s = Screen::new(CLAUDE);
    for i in 0..(text::LINES_MAX + 20) {
        s.feed(format!("line {i}\n").as_bytes());
    }
    assert_eq!(s.lines().len(), text::LINES_MAX);
    assert_eq!(s.dropped, 20);
    let mut s = Screen::new(CLAUDE);
    s.feed(format!("{}\n", "y".repeat(2000)).as_bytes());
    assert!(s.lines()[0].chars().count() <= text::LINE_MAX + 1);
    let mut s = Screen::new(CLAUDE);
    s.feed(b"Waiting\rWaiting\r\nWaiting\n");
    assert_eq!(s.lines(), ["Waiting"]);
}

#[test]
fn versions_and_status_answers_read() {
    assert_eq!(
        version_of("2.1.330 (Claude Code)").as_deref(),
        Some("2.1.330")
    );
    assert_eq!(version_of("codex-cli 0.166.0").as_deref(), Some("0.166.0"));
    assert_eq!(version_of("agy v1.4.2\n").as_deref(), Some("1.4.2"));
    assert_eq!(version_of("no version here."), None);
    assert!(!codex_signed_in(false, "Not logged in"));
    assert!(!codex_signed_in(true, "Not logged in"));
    assert!(codex_signed_in(true, "Logged in using ChatGPT"));
    assert!(!agy_signed_in(
        true,
        "Please sign in to view available models. Launch the CLI without arguments to sign in."
    ));
    assert!(agy_signed_in(true, "gemini-3.8-pro\ngemini-3.8-flash"));
}

// --- the jobs, with fake programs --------------------------------------------------------

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::create_dir_all(dir.path().join("home")).unwrap();
        Self { dir }
    }

    fn bin(&self) -> std::path::PathBuf {
        self.dir.path().join("bin")
    }

    fn put(&self, name: &str, script: &str) {
        let p = self.bin().join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{script}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn started(&self) -> Arc<HashMap<String, String>> {
        Arc::new(started(&[
            ("HOME", self.dir.path().join("home").to_str().unwrap()),
            ("PATH", &format!("{}:/usr/bin:/bin", self.bin().display())),
            ("USER", "op"),
            ("ANTHROPIC_AUTH_TOKEN", "inherited-token-0123456789"),
        ]))
    }

    fn jobs(&self) -> Jobs {
        Jobs::with_limits(
            self.started(),
            Limits {
                sign_in: Duration::from_secs(20),
                install: Duration::from_secs(20),
            },
        )
    }
}

async fn until(jobs: &Jobs, sub: Sub, done: impl Fn(&JobSnapshot) -> bool) -> JobSnapshot {
    for _ in 0..400 {
        if let Some(j) = jobs.job(sub)
            && done(&j)
        {
            return j;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the job never got there: {:?}", jobs.job(sub));
}

/// A fake `codex` that prints its sign-in address, asks for a code, reads it, writes it back
/// and to a file, and is signed in after when the code was right. `login status` answers
/// from that file. It also records whether it got the inherited token.
fn fake_codex(f: &Fake) {
    let got = f.dir.path().join("got");
    f.put(
        "codex",
        &format!(
            "case \"$1 $2\" in\n\
             'login status') if [ -f {got} ]; then echo 'Logged in using ChatGPT'; else echo 'Not logged in'; exit 1; fi;;\n\
             'login ') echo \"token=${{ANTHROPIC_AUTH_TOKEN:-none}} display=${{DISPLAY:-none}}\"\n\
               echo 'If your browser did not open, navigate to this URL to authenticate:'\n\
               printf '\\033]8;;https://auth.openai.com/oauth/authorize?state=s3cret\\033\\\\https://auth.openai.com/oauth/authorize?state=s3cret\\033]8;;\\033\\\\\\n'\n\
               printf 'Paste the code here > '\n\
               read code\n\
               echo \"got $code\"\n\
               [ \"$code\" = 'GOODcode-123456' ] && echo \"$code\" > {got} && echo 'Successfully logged in' && exit 0\n\
               echo 'That code did not work'; exit 1;;\n\
             *) echo 'codex-cli 0.166.0';;\n\
             esac\n",
            got = got.display()
        ),
    );
}

#[tokio::test]
async fn a_sign_in_shows_its_link_waits_for_the_code_and_never_shows_it() {
    let f = Fake::new();
    fake_codex(&f);
    let jobs = f.jobs();
    let started = jobs.start_sign_in(Sub::Codex).unwrap();
    assert_eq!(started.command, "codex login");
    let waiting = until(&jobs, Sub::Codex, |j| j.state == JobState::NeedsCode).await;
    assert_eq!(
        waiting.url.as_deref(),
        Some("https://auth.openai.com/oauth/authorize?state=s3cret")
    );
    let shown = waiting.lines.join("\n");
    assert!(shown.contains("token=none"), "{shown}"); // nothing inherited reached it
    assert!(!shown.contains("s3cret"), "{shown}");
    // A code while it isn't asked, or a bad one, is refused with the rule, never the code.
    let bad = jobs
        .send_code(Sub::Codex, Zeroizing::new("two\nlines".into()))
        .await
        .unwrap_err();
    assert!(!bad.sentence("Codex").contains("two"));
    jobs.send_code(Sub::Codex, Zeroizing::new("GOODcode-123456".into()))
        .await
        .unwrap();
    let done = until(&jobs, Sub::Codex, |j| j.state.ended()).await;
    assert_eq!(done.state, JobState::Done, "{done:?}");
    assert_eq!(done.signed_in, Some(true));
    let everything = format!("{done:?}");
    assert!(!everything.contains("GOODcode"), "{everything}");
    assert!(
        done.lines.iter().any(|l| l.contains(text::HIDDEN)),
        "{:?}",
        done.lines
    );
    // Asked again, the program says it is signed in.
    let seen = look(Sub::Codex, jobs.started()).await;
    assert!(seen.signed_in);
    assert_eq!(seen.version.as_deref(), Some("0.166.0"));
}

#[tokio::test]
async fn a_wrong_code_fails_the_sign_in_and_says_so() {
    let f = Fake::new();
    fake_codex(&f);
    let jobs = f.jobs();
    jobs.start_sign_in(Sub::Codex).unwrap();
    until(&jobs, Sub::Codex, |j| j.state == JobState::NeedsCode).await;
    jobs.send_code(Sub::Codex, Zeroizing::new("WRONGcode-999".into()))
        .await
        .unwrap();
    let done = until(&jobs, Sub::Codex, |j| j.state.ended()).await;
    assert_eq!(done.state, JobState::Failed);
    assert_eq!(done.signed_in, Some(false));
    assert!(done.message.unwrap().contains("exited with 1"));
    assert!(!format!("{:?}", done.lines).contains("WRONGcode"));
}

#[tokio::test]
async fn one_job_at_a_time_and_a_cancel_stops_the_whole_group() {
    let f = Fake::new();
    let marker = f.dir.path().join("child-alive");
    // The program starts a child of its own that would write a file after a while.
    f.put(
        "codex",
        &format!(
            "case \"$1\" in login) (sleep 2; touch {}) & echo 'waiting'; sleep 30;; *) echo 'Not logged in'; exit 1;; esac\n",
            marker.display()
        ),
    );
    let jobs = f.jobs();
    jobs.start_sign_in(Sub::Codex).unwrap();
    assert_eq!(jobs.start_sign_in(Sub::Codex).unwrap_err(), Refused::Busy);
    assert_eq!(jobs.start_install(Sub::Codex).unwrap_err(), Refused::Busy);
    until(&jobs, Sub::Codex, |j| {
        j.lines.iter().any(|l| l == "waiting")
    })
    .await;
    jobs.cancel(Sub::Codex).unwrap();
    let done = until(&jobs, Sub::Codex, |j| j.state.ended()).await;
    assert_eq!(done.state, JobState::Cancelled);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(
        !marker.exists(),
        "the program's own child outlived the cancel"
    );
    assert_eq!(jobs.cancel(Sub::Codex).unwrap_err(), Refused::NoJob);
}

#[tokio::test]
async fn a_sign_in_left_too_long_is_stopped() {
    let f = Fake::new();
    f.put(
        "codex",
        "case \"$1\" in login) printf 'Paste code > '; sleep 30;; *) exit 1;; esac\n",
    );
    let jobs = Jobs::with_limits(
        f.started(),
        Limits {
            sign_in: Duration::from_millis(400),
            install: Duration::from_secs(5),
        },
    );
    jobs.start_sign_in(Sub::Codex).unwrap();
    let done = until(&jobs, Sub::Codex, |j| j.state.ended()).await;
    assert_eq!(done.state, JobState::Failed);
    assert!(done.message.unwrap().starts_with("Stopped after"));
}

#[tokio::test]
async fn antigravity_is_signed_in_in_a_terminal_and_a_missing_program_is_said() {
    let f = Fake::new();
    let jobs = f.jobs();
    // Not installed: said so before any terminal is tried.
    assert_eq!(
        jobs.start_sign_in(Sub::Antigravity).unwrap_err(),
        Refused::NotInstalled
    );
    assert_eq!(
        jobs.start_sign_in(Sub::ClaudeCode).unwrap_err(),
        Refused::NotInstalled
    );
    let seen = look(Sub::ClaudeCode, jobs.started()).await;
    assert!(seen.binary.is_none() && !seen.signed_in);
}

#[tokio::test]
async fn an_install_runs_the_command_as_given_and_streams_its_lines() {
    // `sh -c` of the vendor's command, here with `curl` standing in on the PATH: it prints,
    // writes to both streams, and puts the program in ~/.local/bin as the real one does.
    let f = Fake::new();
    let home = f.dir.path().join("home");
    let args = f.dir.path().join("curl-args");
    f.put(
        "curl",
        &format!(
            "echo \"$*\" > {args}\n\
             echo 'echo Installing Codex; echo \"progress \\033[32m50%\\033[0m\" >&2; \
             mkdir -p $HOME/.local/bin; printf \"#!/bin/sh\\necho codex-cli 9.9.9\\n\" > $HOME/.local/bin/codex; \
             chmod +x $HOME/.local/bin/codex; echo Done.'\n",
            args = args.display()
        ),
    );
    let jobs = f.jobs();
    let started = jobs.start_install(Sub::Codex).unwrap();
    assert_eq!(started.command, Sub::Codex.spec().install);
    let done = until(&jobs, Sub::Codex, |j| j.state.ended()).await;
    assert_eq!(done.state, JobState::Done, "{done:?}");
    assert!(
        done.lines.contains(&"Installing Codex".to_string()),
        "{:?}",
        done.lines
    );
    assert!(
        done.lines.contains(&"progress 50%".to_string()),
        "{:?}",
        done.lines
    );
    assert_eq!(
        std::fs::read_to_string(&args).unwrap().trim(),
        "-fsSL https://chatgpt.com/codex/install.sh"
    );
    assert!(home.join(".local/bin/codex").exists());
    // Installed now (found in ~/.local/bin, not on the PATH Ghost was given).
    assert_eq!(
        jobs.start_install(Sub::Codex).unwrap_err(),
        Refused::Installed
    );
    let seen = look(Sub::Codex, jobs.started()).await;
    assert_eq!(seen.version.as_deref(), Some("9.9.9"));
    assert!(
        which("codex", jobs.started())
            .unwrap()
            .starts_with(Path::new(&home))
    );
}

#[tokio::test]
async fn a_failed_install_says_so() {
    let f = Fake::new();
    f.put(
        "curl",
        "echo 'curl: (6) Could not resolve host' >&2; exit 6\n",
    );
    let jobs = f.jobs();
    jobs.start_install(Sub::Antigravity).unwrap();
    let done = until(&jobs, Sub::Antigravity, |j| j.state.ended()).await;
    assert_eq!(done.state, JobState::Failed);
    assert!(
        done.lines
            .iter()
            .any(|l| l.contains("Could not resolve host"))
    );
}

#[test]
fn the_jobs_are_started_only_from_the_control_apis_operator_routes() {
    // ADR 0078: an install, a sign-in and a pasted code are the operator's click on their
    // desktop. No tool of Ghost's own app, outside client, run, phone route or model reaches
    // them: only the control API's handlers call them, and this module.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut callers = Vec::new();
    let mut stack = vec![crates.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                if !p.ends_with("target") && !p.ends_with("node_modules") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().is_some_and(|x| x == "rs") {
                let t = std::fs::read_to_string(&p).unwrap();
                if ["start_install(", "start_sign_in(", "send_code("]
                    .iter()
                    .any(|w| t.contains(w))
                {
                    callers.push(
                        p.strip_prefix(crates)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
    }
    let allowed = [
        "ghost-api/src/subscriptions.rs",
        "ghost-api/tests/subscriptions.rs",
        "ghost-core/src/subscriptions/jobs.rs",
        "ghost-core/src/subscriptions/tests.rs",
    ];
    for c in &callers {
        assert!(
            allowed.contains(&c.as_str()),
            "{c} starts an install or a sign-in"
        );
    }
}
