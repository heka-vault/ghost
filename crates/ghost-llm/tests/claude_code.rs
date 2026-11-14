//! Claude Code as Ghost's model (ADR 0069), against a fake `claude` that speaks the
//! recorded stream-json (`tests/fixtures/claude-code/fake_claude.py`). Temp folders only:
//! never the operator's program, account or files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use ghost_llm::claude_code::{ClaudeCode, NOT_RUN, Settings};
use ghost_llm::{
    Context, Event, FinishReason, LlmError, Message, ProgramFailure, ToolSchema, Tools,
};
use serde_json::{Value, json};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude-code");
const HELP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ghost-core/src/other_tools/fixtures/claude-help.txt"
);

struct Fake {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
}

impl Fake {
    fn new(scenario: Value) -> Self {
        Self::with_help(scenario, std::fs::read_to_string(HELP).unwrap())
    }

    fn with_help(mut scenario: Value, help: String) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let dir = home.join(".fake-claude");
        std::fs::create_dir_all(&dir).unwrap();
        let help_path = tmp.path().join("help.txt");
        std::fs::write(&help_path, help).unwrap();
        scenario["help"] = json!(help_path);
        std::fs::write(dir.join("scenario.json"), scenario.to_string()).unwrap();
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("claude");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\nexec python3 -I {FIXTURES}/fake_claude.py \"$@\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fake {
            _tmp: tmp,
            home,
            bin,
        }
    }

    fn settings(&self) -> Settings {
        let started: HashMap<String, String> = [
            ("HOME", self.home.to_str().unwrap().to_string()),
            ("PATH", std::env::var("PATH").unwrap()),
            // What the workstation's own environment carried: never passed on.
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:9".into()),
            (
                "ANTHROPIC_AUTH_TOKEN",
                "inherited-proxy-token-0123456789".into(),
            ),
            ("CLAUDE_CODE_ENTRYPOINT", "sdk-ts".into()),
            ("CLAUDECODE", "1".into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let mut s = Settings::new(self.bin.clone(), "haiku", started);
        s.quiet_most = Duration::from_secs(20);
        s.runtime_dir = self.run_dir();
        s
    }

    /// The runtime folder the processes' folders go in: a temp one, never the operator's.
    fn run_dir(&self) -> PathBuf {
        self.home.parent().unwrap().join("run")
    }

    /// The process folders in the runtime folder now.
    fn folders(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.run_dir())
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    fn model(&self) -> ClaudeCode {
        ClaudeCode::new(self.settings())
    }

    fn log(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.home.join(".fake-claude").join(name))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The runs that were sessions (stream-json), not questions.
    fn sessions(&self) -> Vec<Value> {
        self.log("runs.jsonl")
            .into_iter()
            .filter(|r| {
                r["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| a == "stream-json")
            })
            .collect()
    }
}

fn tools() -> Tools {
    Tools::new(vec![
        ToolSchema::new("clock_read", "Reads the time.", json!({}), vec![]),
        ToolSchema::new(
            "github_issue_comment",
            "Comments on an issue.",
            json!({"body": {"type": "string"}}),
            vec!["body".into()],
        ),
    ])
}

async fn collect(model: &ClaudeCode, ctx: Context) -> Vec<Result<Event, LlmError>> {
    tokio::time::timeout(
        Duration::from_secs(30),
        model.stream(ctx, &tools()).collect::<Vec<_>>(),
    )
    .await
    .expect("the turn ended")
}

fn text_of(events: &[Result<Event, LlmError>]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Ok(Event::Text(t)) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn ctx(turns: &[&str]) -> Context {
    let mut c = Context::new("You are Ghost, a test.");
    for t in turns {
        c.push(Message::user(*t));
    }
    c
}

#[tokio::test]
async fn a_text_reply_streams_as_the_events_an_http_model_gives_and_nothing_is_inherited() {
    let fake = Fake::new(json!({"turns": [[{"text": "Hello there, friendly test assistant!"}]]}));
    let model = fake.model();
    let events = collect(&model, ctx(&["Say hello in five words."])).await;
    assert_eq!(text_of(&events), "Hello there, friendly test assistant!");
    assert!(events.iter().any(|e| matches!(e, Ok(Event::Usage(_)))));
    assert_eq!(
        events.last().unwrap().as_ref().unwrap(),
        &Event::Finished(FinishReason::End)
    );
    let runs = fake.sessions();
    assert_eq!(runs.len(), 1);
    let argv: Vec<String> = serde_json::from_value(runs[0]["argv"].clone()).unwrap();
    for must in [
        "--tools",
        "--setting-sources",
        "--strict-mcp-config",
        "--allowedTools=mcp__ghost__*",
        "--disable-slash-commands",
        "--no-session-persistence",
    ] {
        assert!(argv.iter().any(|a| a == must), "{must} in {argv:?}");
    }
    assert_eq!(
        runs[0]["inherited"],
        json!({}),
        "nothing ANTHROPIC_* or CLAUDE_* reached it"
    );
    assert_eq!(
        fake.log("turns.jsonl")[0]["text"],
        "Say hello in five words."
    );
}

#[tokio::test]
async fn the_prompt_is_a_private_file_never_the_command_line() {
    use std::os::unix::fs::PermissionsExt;
    let fake = Fake::new(json!({"turns": [[{"text": "Hello."}]]}));
    let model = fake.model();
    let persona = "You are Ledger. PERSONA-dry-exact. MEMORY-the-operator-banks-at-Halifax. \
                   RULES-UK-dates-always.";
    let mut c = Context::new(persona);
    c.push(Message::user("Hi."));
    let events = collect(&model, c).await;
    assert_eq!(text_of(&events), "Hello.");
    let runs = fake.sessions();
    let argv: Vec<String> = serde_json::from_value(runs[0]["argv"].clone()).unwrap();
    let line = argv.join(" ");
    for part in persona.split_whitespace() {
        assert!(
            !line.contains(part),
            "{part} is on the command line: {line}"
        );
    }
    let (_, bearer) = relay_of(&fake);
    let token = bearer.strip_prefix("Bearer ").unwrap();
    assert!(!line.contains(token), "the bearer is on the command line");
    // The program read Ghost's prompt from a 0600 file, in a 0700 folder of its own.
    let seen = &fake.log("prompt.jsonl")[0];
    assert_eq!(seen["text"], persona);
    assert_eq!(seen["mode"], "0o600");
    let folder = Path::new(seen["path"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(folder.starts_with(fake.run_dir()));
    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    // Gone with the process.
    drop(model);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!folder.exists(), "the prompt outlived its process");
    assert!(fake.folders().is_empty());
}

#[tokio::test]
async fn a_process_that_died_leaves_no_prompt_and_a_crashs_leftovers_go_at_the_next_start() {
    let fake = Fake::new(json!({"turns": [[{"die": true}], [{"text": "Back."}]]}));
    // What a Ghost killed mid-session left: its process folder, the prompt in it.
    let left = fake.run_dir().join("p4194999-crashed");
    std::fs::create_dir_all(&left).unwrap();
    std::fs::write(left.join("system-prompt.md"), "an older persona").unwrap();
    let model = fake.model();
    let mut c = ctx(&["Hi."]);
    let died = collect(&model, c.clone()).await;
    assert!(died.iter().any(|e| e.is_err()));
    assert!(
        !left.exists(),
        "a stopped Ghost's leftovers are swept at the next start"
    );
    assert!(
        fake.folders().is_empty(),
        "the dead process's folder went with it"
    );
    c.push_reply("x", vec![]);
    c.push(Message::user("Again."));
    let again = collect(&model, c).await;
    assert_eq!(text_of(&again), "Back.");
    assert_eq!(fake.folders().len(), 1, "one live process, one folder");
}

#[tokio::test]
async fn a_version_without_the_prompt_file_is_refused_rather_than_given_it_on_argv() {
    let fake = Fake::new(json!({"no_prompt_file": true, "turns": [[{"text": "never"}]]}));
    let events = collect(&fake.model(), ctx(&["Hi."])).await;
    assert_eq!(
        events.iter().find_map(|e| e.as_ref().err()),
        Some(&LlmError::Program(ProgramFailure::Unsupported))
    );
    assert!(fake.sessions().is_empty());
    let found =
        ghost_core::other_tools::program::look(&fake.bin, &fake.settings().started_env).await;
    assert_eq!(found.missing, vec!["--system-prompt-file"]);
}

#[tokio::test]
async fn a_tool_call_is_parked_for_the_stage_and_its_result_is_the_programs_answer() {
    let fake = Fake::new(json!({"turns": [[
        {"tool": "clock_read", "input": {}},
        {"text": "It is noon."}
    ]]}));
    let model = fake.model();
    let mut c = ctx(&["What time is it?"]);
    let first = collect(&model, c.clone()).await;
    assert!(first.contains(&Ok(Event::ToolStarting("clock_read".into()))));
    let call = first
        .iter()
        .find_map(|e| match e {
            Ok(Event::ToolCall(c)) => Some(c.clone()),
            _ => None,
        })
        .expect("the call, for the stage's dispatcher");
    assert_eq!(call.name, "clock_read");
    assert_eq!(
        first.last().unwrap().as_ref().unwrap(),
        &Event::Finished(FinishReason::ToolUse)
    );
    assert!(
        fake.log("results.jsonl").is_empty(),
        "nothing ran: the call waits at the relay"
    );
    // The stage ran it (through the gate) and asks again with the result.
    c.push_reply("", vec![call.clone()]);
    c.push_result(&call.id, &json!({"time": "12:00"}));
    let second = collect(&model, c).await;
    assert_eq!(text_of(&second), "It is noon.");
    let results = fake.log("results.jsonl");
    assert_eq!(results.len(), 1);
    let answer = &results[0]["answer"]["result"];
    assert_eq!(answer["isError"], false);
    assert_eq!(answer["content"][0]["text"], r#"{"time":"12:00"}"#);
    assert_eq!(fake.sessions().len(), 1, "one process for the whole turn");
}

#[tokio::test]
async fn a_reply_dropped_mid_text_interrupts_the_program_and_the_next_turn_is_heard() {
    let long = (1..=200)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let fake = Fake::new(json!({"turns": [
        [{"text": long, "pace": 0.02}],
        [{"text": "Ready."}]
    ]}));
    let model = fake.model();
    let mut c = ctx(&["Count for a while."]);
    {
        let mut s = model.stream(c.clone(), &tools());
        let first = tokio::time::timeout(Duration::from_secs(20), s.next())
            .await
            .unwrap();
        assert!(matches!(first, Some(Ok(Event::Text(_)))), "{first:?}");
        // The operator talks over it: the stage drops the stream.
    }
    c.push_reply("w1", vec![]);
    c.push(Message::user("Say ready."));
    let began = Instant::now();
    let events = collect(&model, c).await;
    assert_eq!(text_of(&events), "Ready.");
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(
        fake.log("control.jsonl")
            .iter()
            .any(|c| c["subtype"] == "interrupt")
    );
    assert_eq!(fake.sessions().len(), 1, "interrupted, not restarted");
}

#[tokio::test]
async fn a_call_the_stage_never_made_is_refused_and_the_program_stopped_on_it() {
    let fake = Fake::new(json!({"turns": [
        [{"tool": "github_issue_comment", "input": {"body": "hi"}}, {"text": "Done."}],
        [{"text": "Okay, I won't."}]
    ]}));
    let model = fake.model();
    let mut c = ctx(&["Comment hi on the issue."]);
    let first = collect(&model, c.clone()).await;
    let call = first
        .iter()
        .find_map(|e| match e {
            Ok(Event::ToolCall(c)) => Some(c.clone()),
            _ => None,
        })
        .unwrap();
    // The operator went on before Ghost acted: the stage keeps the reply without its calls.
    c.push_reply("", vec![]);
    c.push(Message::user("Actually, don't."));
    let second = collect(&model, c).await;
    assert_eq!(text_of(&second), "Okay, I won't.");
    let results = fake.log("results.jsonl");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["name"], call.name);
    let answer = &results[0]["answer"]["result"];
    assert_eq!(
        answer["isError"], true,
        "the program is told it was not run"
    );
    assert_eq!(answer["content"][0]["text"], NOT_RUN);
}

#[tokio::test]
async fn a_process_that_dies_mid_turn_is_a_spoken_failure_and_the_next_turn_starts_fresh() {
    let fake = Fake::new(json!({"turns": [
        [{"text": "Hello."}],
        [{"die": true}],
        [{"text": "Back again."}]
    ]}));
    let model = fake.model();
    let mut c = ctx(&["Hi."]);
    let _ = collect(&model, c.clone()).await;
    c.push_reply("Hello.", vec![]);
    c.push(Message::user("Tell me a thing."));
    let died = collect(&model, c.clone()).await;
    let err = died
        .iter()
        .find_map(|e| e.as_ref().err())
        .expect("an error");
    assert_eq!(err, &LlmError::Program(ProgramFailure::Died));
    let said = ghost_llm::spoken_failure(err);
    assert!(said.contains("Claude Code stopped partway"), "{said}");
    c.push_reply(said, vec![]);
    c.push(Message::user("Try again."));
    let again = collect(&model, c).await;
    assert_eq!(text_of(&again), "Back again.");
    assert_eq!(fake.sessions().len(), 2, "a fresh process");
    let turns = fake.log("turns.jsonl");
    let fresh = turns.last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(
        fresh.contains("Operator: Hi.") && fresh.contains("You: Hello."),
        "{fresh}"
    );
    assert!(fresh.ends_with("Try again."), "{fresh}");
}

#[tokio::test]
async fn a_signed_out_program_is_said_as_its_sign_in_and_a_refused_retry_is_not_waited() {
    let fake = Fake::new(json!({"turns": [
        [{"error": "authentication_failed"}],
        [{"retry": 401}]
    ]}));
    let model = fake.model();
    let mut c = ctx(&["Hi."]);
    let events = collect(&model, c.clone()).await;
    let err = events.iter().find_map(|e| e.as_ref().err()).unwrap();
    assert_eq!(err, &LlmError::Program(ProgramFailure::SignedOut));
    let said = ghost_llm::spoken_failure(err);
    assert!(
        said.contains("claude auth login") && !said.contains("never repeats"),
        "{said}"
    );
    c.push_reply(said, vec![]);
    c.push(Message::user("Hi again."));
    let began = Instant::now();
    let events = collect(&model, c).await;
    assert_eq!(
        events.iter().find_map(|e| e.as_ref().err()),
        Some(&LlmError::Program(ProgramFailure::SignedOut))
    );
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "the program's ten retries are not waited"
    );
    // The interrupt that stopped them goes out before the next turn.
    let _ = collect(&model, ctx(&["x"])).await;
    assert!(
        fake.log("control.jsonl")
            .iter()
            .any(|c| c["subtype"] == "interrupt")
    );
}

#[tokio::test]
async fn a_version_without_a_needed_switch_is_refused_before_anything_runs() {
    let help = std::fs::read_to_string(HELP)
        .unwrap()
        .replace("--strict-mcp-config", "--something-else");
    let fake = Fake::with_help(json!({"turns": [[{"text": "never"}]]}), help);
    let events = collect(&fake.model(), ctx(&["Hi."])).await;
    assert_eq!(
        events.iter().find_map(|e| e.as_ref().err()),
        Some(&LlmError::Program(ProgramFailure::Unsupported))
    );
    assert!(
        fake.sessions().is_empty(),
        "never run without its own tools off"
    );
    let missing = ClaudeCode::new(Settings::new(
        "/nonexistent/claude".into(),
        "haiku",
        HashMap::new(),
    ));
    let events = collect(&missing, ctx(&["Hi."])).await;
    assert_eq!(
        events.iter().find_map(|e| e.as_ref().err()),
        Some(&LlmError::Program(ProgramFailure::Missing))
    );
}

/// The relay's address and bearer, from the app file the fake was given.
fn relay_of(fake: &Fake) -> (String, String) {
    let runs = fake.sessions();
    let argv: Vec<String> = serde_json::from_value(runs[0]["argv"].clone()).unwrap();
    let at = argv.iter().position(|a| a == "--mcp-config").unwrap();
    let path = Path::new(&argv[at + 1]);
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(path).unwrap().permissions());
    assert_eq!(
        mode & 0o777,
        0o600,
        "the bearer's file is the operator's alone"
    );
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let ghost = &cfg["mcpServers"]["ghost"];
    (
        ghost["url"].as_str().unwrap().to_string(),
        ghost["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

async fn post(url: &str, auth: Option<&str>, origin: Option<&str>, body: Value) -> (u16, Value) {
    let mut r = reqwest::Client::new().post(url).json(&body);
    if let Some(a) = auth {
        r = r.header("Authorization", a);
    }
    if let Some(o) = origin {
        r = r.header("Origin", o);
    }
    let resp = r.send().await.unwrap();
    let status = resp.status().as_u16();
    let v = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, v)
}

#[tokio::test]
async fn the_relay_refuses_a_wrong_bearer_a_browser_an_unoffered_tool_and_a_session_ended() {
    let fake = Fake::new(json!({"turns": [[{"text": "Hi."}]]}));
    let model = fake.model();
    model
        .warm("You are Ghost, a test.", &tools())
        .await
        .unwrap();
    let (url, bearer) = relay_of(&fake);
    let call = |name: &str| json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": {}}});
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (s, v) = post(&url, Some(&bearer), None, list.clone()).await;
    assert_eq!(s, 200);
    let names: Vec<&str> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["clock_read", "github_issue_comment"],
        "exactly the session's tools"
    );
    let wrong = format!("Bearer {}", "0".repeat(64));
    assert_eq!(post(&url, Some(&wrong), None, list.clone()).await.0, 401);
    assert_eq!(post(&url, None, None, list.clone()).await.0, 401);
    assert_eq!(
        post(
            &url,
            Some(&bearer),
            Some("http://evil.example"),
            list.clone()
        )
        .await
        .0,
        403
    );
    let (s, v) = post(&url, Some(&bearer), None, call("bash")).await;
    assert_eq!(s, 200);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not offered"),
        "{v}"
    );
    drop(model); // the session ends
    tokio::time::sleep(Duration::from_millis(100)).await;
    let gone = reqwest::Client::new()
        .post(&url)
        .header("Authorization", &bearer)
        .json(&list)
        .send()
        .await;
    assert!(gone.is_err(), "nothing listens once the session has ended");
}

#[tokio::test]
async fn the_models_are_the_programs_own_list_and_asking_them_asks_no_model() {
    let fake = Fake::new(json!({"turns": []}));
    let models = fake.model().models().await.unwrap();
    assert_eq!(
        models.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>(),
        ["haiku", "sonnet"]
    );
    assert!(fake.log("turns.jsonl").is_empty());
}
