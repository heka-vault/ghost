//! The operator's own AI subscriptions (ADR 0078), through the control API: asked, installed
//! and signed in with fake programs on a temp PATH, never the operator's own or their accounts.

mod common;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use common::World;
use ghost_core::subscriptions::Jobs;
use serde_json::{Value, json};

struct Setup {
    w: World,
    dir: tempfile::TempDir,
}

fn put(dir: &std::path::Path, name: &str, script: &str) {
    let p = dir.join("bin").join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{script}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn setup() -> Setup {
    let mut w = World::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let started: HashMap<String, String> = [
        ("HOME", dir.path().join("home").display().to_string()),
        (
            "PATH",
            format!("{}:/usr/bin:/bin", dir.path().join("bin").display()),
        ),
        ("USER", "op".to_string()),
        // The operator's shell held one; it must not make a program look signed in.
        (
            "ANTHROPIC_AUTH_TOKEN",
            "inherited-token-0123456789".to_string(),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    w.subscriptions = Some(Arc::new(Jobs::new(Arc::new(started))));
    Setup { w, dir }
}

/// A `claude` that answers like 2.1.330 signed out unless a file says it signed in; its
/// `auth login` prints its address, asks for a code and signs in with the right one.
fn fake_claude(dir: &std::path::Path) {
    let help = include_str!("../../ghost-core/src/other_tools/fixtures/claude-help.txt");
    std::fs::write(dir.join("help.txt"), help).unwrap();
    let ok = dir.join("signed-in");
    put(
        dir,
        "claude",
        &format!(
            "case \"$1\" in\n\
             --version) echo '2.1.330 (Claude Code)';;\n\
             --help) cat {help};;\n\
             --system-prompt-file) echo \"error: unknown option '$3'\" >&2; exit 1;;\n\
             auth) if [ \"$2\" = status ]; then\n\
                 if [ -f {ok} ] && [ -z \"$ANTHROPIC_AUTH_TOKEN\" ]; then echo '{{\"loggedIn\": true, \"authMethod\": \"claude.ai\", \"subscriptionType\": \"max\"}}';\n\
                 elif [ -n \"$ANTHROPIC_AUTH_TOKEN\" ]; then echo '{{\"loggedIn\": true, \"authMethod\": \"api_key\"}}';\n\
                 else echo '{{\"loggedIn\": false}}'; exit 1; fi\n\
               else\n\
                 echo 'Opening browser to sign in…'\n\
                 echo \"If the browser didn't open, visit:\"\n\
                 echo 'https://claude.ai/oauth/authorize?code=true&state=st4te'\n\
                 printf 'Paste code here if prompted > '\n\
                 read code; echo \"$code\"\n\
                 [ \"$code\" = 'pasted#CODE-0042' ] && touch {ok} && echo 'Login successful.' && exit 0\n\
                 exit 1\n\
               fi;;\n\
             esac\n",
            help = dir.join("help.txt").display(),
            ok = ok.display()
        ),
    );
}

async fn wait_job(c: &common::TestClient, id: &str, until: &str) -> Value {
    for _ in 0..400 {
        let r = c.get(&format!("/subscriptions/{id}/job")).await;
        assert_eq!(r.status, 200);
        let j = r.json;
        if j["state"] == until {
            return j;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never {until}");
}

#[tokio::test]
async fn the_three_are_listed_as_their_programs_say_and_nothing_inherited_signs_one_in() {
    let s = setup();
    fake_claude(s.dir.path());
    let c = s.w.client();
    let r = c.get("/subscriptions").await;
    assert_eq!(r.status, 200);
    let v = r.json;
    let ids: Vec<&str> = v["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["claude-code", "codex", "antigravity"]);
    let cc = &v["subscriptions"][0];
    assert_eq!(cc["installed"], true);
    assert_eq!(cc["version"], "2.1.330");
    assert_eq!(cc["signed_in"], false, "{cc}");
    assert_eq!(cc["usable"], false);
    assert_eq!(cc["ghost_can_use"], true);
    assert_eq!(
        cc["install_command"],
        "curl -fsSL https://claude.ai/install.sh | bash"
    );
    let codex = &v["subscriptions"][1];
    assert_eq!(codex["installed"], false);
    assert_eq!(codex["ghost_can_use"], false);
    assert_eq!(v["subscriptions"][2]["sign_in_here"], false);
    assert_eq!(v["in_use"], Value::Null);
}

#[tokio::test]
async fn sign_in_pastes_the_code_and_it_never_comes_back_then_use_it() {
    let s = setup();
    fake_claude(s.dir.path());
    let c = s.w.client();
    // Not yet usable: refused with the program's own sentence.
    let r = c.put("/subscriptions/claude-code/use", json!({})).await;
    assert_eq!(r.status, 409);
    let r = c.post("/subscriptions/claude-code/sign-in", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.json["command"], "claude auth login");
    let j = wait_job(&c, "claude-code", "needs_code").await;
    assert_eq!(
        j["url"],
        "https://claude.ai/oauth/authorize?code=true&state=st4te"
    );
    assert!(!j["lines"].to_string().contains("st4te"), "{j}");
    let bad = c
        .post(
            "/subscriptions/claude-code/sign-in/code",
            Some(json!({"code": ""})),
        )
        .await;
    assert_eq!(bad.status, 422);
    let r = c
        .post(
            "/subscriptions/claude-code/sign-in/code",
            Some(json!({"code": "pasted#CODE-0042"})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains("CODE-0042"));
    let done = wait_job(&c, "claude-code", "done").await;
    assert_eq!(done["signed_in"], true);
    let all = c.get("/subscriptions").await.text;
    assert!(!all.contains("CODE-0042"), "{all}");
    // A code with nothing waiting is refused.
    let r = c
        .post(
            "/subscriptions/claude-code/sign-in/code",
            Some(json!({"code": "again"})),
        )
        .await;
    assert_eq!(r.status, 404);
    // Now it is Ghost's model: the provider, the rest of the connection cleared.
    std::fs::write(
        &s.w.settings_path,
        "GHOST_LLM_PROVIDER=openai\nGHOST_LLM_MODEL=gpt-x\nGHOST_WAKE_PHRASES=hey ghost\n",
    )
    .unwrap();
    let r = c.put("/subscriptions/claude-code/use", json!({})).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let settings = std::fs::read_to_string(&s.w.settings_path).unwrap();
    assert!(
        settings.contains("GHOST_LLM_PROVIDER=claude-code"),
        "{settings}"
    );
    assert!(!settings.contains("GHOST_LLM_MODEL"), "{settings}");
    assert!(settings.contains("GHOST_WAKE_PHRASES"), "{settings}");
}

#[tokio::test]
async fn codex_and_antigravity_are_coming_next_and_antigravity_signs_in_in_a_terminal() {
    let s = setup();
    put(s.dir.path(), "codex", "echo 'Logged in using ChatGPT'\n");
    let c = s.w.client();
    let r = c.put("/subscriptions/codex/use", json!({})).await;
    assert_eq!(r.status, 409);
    assert!(
        r.text.contains("can't use OpenAI Codex as its model yet"),
        "{}",
        r.text
    );
    let r = c.post("/subscriptions/antigravity/sign-in", None).await;
    assert_eq!(r.status, 409);
    assert!(r.text.contains("`agy`"), "{}", r.text);
    assert_eq!(
        c.post("/subscriptions/nope/install", None).await.status,
        404
    );
    assert_eq!(
        c.post("/subscriptions/codex/cancel", None).await.status,
        404
    );
    // Installed already: Install is refused.
    assert_eq!(
        c.post("/subscriptions/codex/install", None).await.status,
        409
    );
}

#[tokio::test]
async fn install_runs_the_vendors_command_and_follows_it() {
    let s = setup();
    put(
        s.dir.path(),
        "curl",
        "echo 'echo Installing; mkdir -p $HOME/.local/bin; printf \"#!/bin/sh\\necho 0.1.2\\n\" > $HOME/.local/bin/agy; chmod +x $HOME/.local/bin/agy'\n",
    );
    let c = s.w.client();
    let r = c.post("/subscriptions/antigravity/install", None).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(
        r.json["command"],
        "curl -fsSL https://antigravity.google/cli/install.sh | bash"
    );
    let done = wait_job(&c, "antigravity", "done").await;
    assert!(done["lines"].to_string().contains("Installing"), "{done}");
    let v = c.get("/subscriptions").await.json;
    assert_eq!(v["subscriptions"][2]["installed"], true);
    assert_eq!(v["subscriptions"][2]["job"]["state"], "done");
}
