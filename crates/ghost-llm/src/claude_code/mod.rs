//! The operator's own Claude Code as Ghost's model (ADR 0069).
//!
//! Ghost runs the program the operator installed and signed in, as it is, one process per
//! session (and per run with nobody there), and talks to it over its documented
//! `stream-json` protocol: Ghost's whole prompt as its system prompt, the operator's turns
//! on its standard input, the reply's text, thinking and tool calls read off its standard
//! output as they stream. Its built-in tools are off (`--tools ""`), the operator's settings,
//! hooks, plugins, skills, slash commands and apps are not loaded, and the one app it is
//! given is Ghost's relay ([`relay`]), whose tools are exactly the session's and which runs
//! nothing: each call is parked and comes out of [`ClaudeCode::stream`] as the same
//! `Event::ToolCall` an HTTP model's reply carries, for the stage's dispatcher and the gate.
//! The result the stage sends in its next call is the parked request's answer.
//!
//! The program keeps the conversation's history; Ghost's transcript stays the record (ADR
//! 0014). [`ClaudeCode::stream`] gets Ghost's context every call, as every model does, and
//! keeps the two in step: what the program has been given is remembered, the new part of
//! Ghost's context is fed (a tool's result to its parked call, the operator's words as a
//! turn), and a context that no longer begins with what was fed (a compaction, a handover's
//! new prompt, a session resumed) starts a fresh process with the history before the turn
//! given as the turn's first lines. A turn dropped by the stage (the operator talked over
//! it) interrupts the program the way the Agent SDK does, refuses every parked call, and
//! reads the program to its turn's end before anything else is written.
//!
//! Never: reading or copying its sign-in, writing its settings, passing it anything named
//! `ANTHROPIC_*` or `CLAUDE_*` from Ghost's environment, offering a sign-in. Its sign-in is
//! the operator's, made in a terminal with its own command.

pub mod check;
pub mod relay;
pub mod wire;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime};

use futures::Stream;
use futures::stream::BoxStream;
use ghost_core::other_tools::program;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, OwnedMutexGuard, mpsc};
use tracing::{debug, info, warn};

use crate::context::{Context, Message, ToolCall};
use crate::model::{Event, FinishReason, LlmError, ProgramFailure, Usage};
use crate::tools::Tools;
use relay::{Answer, Parked, Relay};
use wire::Wire;

/// The app name the relay is given in `--mcp-config`: its tools are `mcp__ghost__<tool>`.
pub const APP: &str = "ghost";
/// The effort Ghost asks for: thinking cannot be turned off on the current models
/// (`supportsThinkingOff: false` in the program's own list), and low effort measured the
/// quickest first word (ADR 0069).
pub const EFFORT: &str = "low";
/// How long the program may take to start and connect the relay.
pub const READY_WITHIN: Duration = Duration::from_secs(30);
/// How long a turn waits with nothing from the program before it is a timeout.
pub const QUIET_MOST: Duration = Duration::from_secs(120);
/// After the reply stopped for its tool calls, how long Ghost waits for one more to reach
/// the relay before the ones there are the reply's. The program calls a tool as its block
/// closes (2026-11-14: 4 ms after it), so this only gathers a batch.
pub const GATHER: Duration = Duration::from_millis(150);
/// The most a turn waits for an interrupted reply to end.
pub const DRAIN_WITHIN: Duration = Duration::from_secs(5);
/// How long a call through the relay may take, for the program's own timeout (a change
/// waiting for the operator's yes is a call in progress): ten minutes.
pub const TOOL_TIMEOUT_MS: &str = "600000";
/// A refused parked call's answer.
pub const NOT_RUN: &str = "Not run: the operator went on before Ghost made this call.";

/// How the program is run.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The program, found on the PATH Ghost was started with.
    pub binary: PathBuf,
    /// An alias the program takes (`haiku`, `sonnet`) or a model's full name.
    pub model: String,
    /// The environment Ghost was started in; only [`program::clean_env`] of it reaches the
    /// program.
    pub started_env: HashMap<String, String>,
    pub quiet_most: Duration,
}

impl Settings {
    pub fn new(
        binary: PathBuf,
        model: impl Into<String>,
        started_env: HashMap<String, String>,
    ) -> Self {
        Self {
            binary,
            model: model.into(),
            started_env,
            quiet_most: QUIET_MOST,
        }
    }
}

/// The program's environment: [`program::clean_env`] and the relay's call timeout.
pub fn environment(settings: &Settings) -> BTreeMap<String, String> {
    let mut env = program::clean_env(&settings.started_env);
    env.insert("MCP_TOOL_TIMEOUT".into(), TOOL_TIMEOUT_MS.into());
    env
}

/// The program's argv after its name. `system` is Ghost's whole prompt; `apps` the 0600
/// file naming the relay.
pub fn argv(settings: &Settings, system: &str, apps: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--system-prompt",
    ]
    .map(String::from)
    .to_vec();
    a.push(system.to_string());
    a.extend(
        [
            "--tools",
            "",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--mcp-config",
        ]
        .map(String::from),
    );
    a.push(apps.display().to_string());
    a.extend(
        [
            "--allowedTools=mcp__ghost__*",
            "--disable-slash-commands",
            "--no-session-persistence",
            "--permission-mode",
            "dontAsk",
            "--settings",
            r#"{"alwaysThinkingEnabled":false}"#,
            "--effort",
            EFFORT,
            "--model",
        ]
        .map(String::from),
    );
    a.push(settings.model.clone());
    a
}

/// What the program's process sends a turn: its lines, the relay's calls, its end.
#[derive(Debug)]
pub enum Incoming {
    Line(Value),
    Call(Parked),
    Ended,
}

/// The switches a program lacks, asked once per binary and kept until it changes.
static SWITCHES: LazyLock<StdMutex<HashMap<(PathBuf, Option<SystemTime>), Vec<&'static str>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

async fn missing_switches(settings: &Settings) -> Vec<&'static str> {
    let stamp = std::fs::metadata(&settings.binary)
        .and_then(|m| m.modified())
        .ok();
    let key = (settings.binary.clone(), stamp);
    if let Some(m) = SWITCHES.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return m.clone();
    }
    let missing = program::missing_at(&settings.binary, &settings.started_env).await;
    SWITCHES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, missing.clone());
    missing
}

/// One running program.
struct Proc {
    child: Child,
    lines: mpsc::UnboundedSender<String>,
    rx: mpsc::UnboundedReceiver<Incoming>,
    relay: Relay,
    system: String,
    models: Vec<(String, String)>,
    interrupts: u64,
    _work: tempfile::TempDir,
}

impl Proc {
    fn send(&self, line: String) -> bool {
        self.lines.send(line).is_ok()
    }

    fn interrupt(&mut self) {
        self.interrupts += 1;
        let _ = self.send(wire::interrupt(self.interrupts));
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Start the program for `system` and `tools`: its folder (an empty temp folder), the
/// relay, the process, and its `initialize` answered, so its apps are connected before
/// the first turn (a cold process with an app took 3.5 s to its first call otherwise).
async fn start(settings: &Settings, system: &str, tools: &Tools) -> Result<Proc, LlmError> {
    if !settings.binary.is_file() {
        return Err(LlmError::Program(ProgramFailure::Missing));
    }
    let missing = missing_switches(settings).await;
    if !missing.is_empty() {
        warn!(
            "claude code: this version lacks {}; not run",
            missing.join(", ")
        );
        return Err(LlmError::Program(ProgramFailure::Unsupported));
    }
    let began = Instant::now();
    let work = tempfile::Builder::new()
        .prefix("ghost-claude-")
        .tempdir()
        .map_err(|_| LlmError::Program(ProgramFailure::Failed))?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let relay = Relay::bind(tools, tx.clone())
        .await
        .map_err(|_| LlmError::Program(ProgramFailure::Failed))?;
    let apps = work.path().join("apps.json");
    let config = serde_json::json!({"mcpServers": {APP: {
        "type": "http", "url": relay.url(),
        "headers": {"Authorization": format!("Bearer {}", relay.bearer())}}}});
    write_private(&apps, &config.to_string())
        .map_err(|_| LlmError::Program(ProgramFailure::Failed))?;
    let mut child = Command::new(&settings.binary)
        .args(argv(settings, system, &apps))
        .env_clear()
        .envs(environment(settings))
        .current_dir(work.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LlmError::Program(ProgramFailure::Missing)
            } else {
                LlmError::Program(ProgramFailure::Died)
            }
        })?;
    let (lines, mut to_write) = mpsc::unbounded_channel::<String>();
    let mut stdin = child.stdin.take().expect("piped");
    tokio::spawn(async move {
        while let Some(line) = to_write.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                return;
            }
        }
    });
    let stdout = child.stdout.take().expect("piped");
    let out_tx = tx.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            match serde_json::from_str::<Value>(&line) {
                Ok(v) => {
                    if out_tx.send(Incoming::Line(v)).is_err() {
                        return;
                    }
                }
                Err(_) => debug!("claude code: a line that isn't JSON ({} bytes)", line.len()),
            }
        }
        let _ = out_tx.send(Incoming::Ended);
    });
    // What it writes on standard error is its own: read and dropped, never logged.
    let mut stderr = child.stderr.take().expect("piped");
    tokio::spawn(async move {
        let mut sink = vec![0u8; 4096];
        while matches!(stderr.read(&mut sink).await, Ok(n) if n > 0) {}
    });
    drop(tx);
    let _ = lines.send(wire::initialize());
    let models = tokio::time::timeout(READY_WITHIN, async {
        while let Some(m) = rx.recv().await {
            match m {
                Incoming::Line(v) => {
                    if let Wire::Ready { models } = wire::read(&v) {
                        return Ok(models);
                    }
                }
                Incoming::Call(p) => {
                    let _ = p.answer.send(Answer::Refused(NOT_RUN.into()));
                }
                Incoming::Ended => break,
            }
        }
        Err(LlmError::Program(ProgramFailure::Died))
    })
    .await
    .map_err(|_| LlmError::Unreachable {
        host: "Claude Code".into(),
        kind: "ReadTimeout",
    })??;
    info!(
        "claude code: ready in {:.2}s ({} tools through the relay)",
        began.elapsed().as_secs_f64(),
        tools.len()
    );
    Ok(Proc {
        child,
        lines,
        rx,
        relay,
        system: system.to_string(),
        models,
        interrupts: 0,
        _work: work,
    })
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())
}

/// What one model's process and its conversation are at.
#[derive(Default)]
struct State {
    proc: Option<Proc>,
    /// Ghost's messages as they were when the program was last given its part: the
    /// program has seen every one (or what the stage kept of its own reply).
    known: Vec<Message>,
    /// The process has been given nothing yet: the history comes with the first turn.
    fresh: bool,
    /// Calls waiting for the stage's results, by the id Ghost gave them, in order.
    parked: Vec<(String, tokio::sync::oneshot::Sender<Answer>)>,
    next_id: u64,
    /// A turn was dropped or failed: read the program to its turn's end first.
    draining: bool,
}

impl State {
    fn stop(&mut self) {
        self.refuse_parked();
        self.proc = None;
        self.known.clear();
        self.fresh = true;
        self.draining = false;
    }

    fn refuse_parked(&mut self) {
        for (_, answer) in self.parked.drain(..) {
            let _ = answer.send(Answer::Refused(NOT_RUN.into()));
        }
    }

    /// Read the program until the interrupted or failed turn has ended; refuse what it
    /// calls meanwhile. A program that won't end it is stopped.
    async fn drain(&mut self) {
        self.draining = false;
        self.refuse_parked();
        let Some(p) = self.proc.as_mut() else { return };
        let ended = tokio::time::timeout(DRAIN_WITHIN, async {
            while let Some(m) = p.rx.recv().await {
                match m {
                    Incoming::Line(v) => {
                        if matches!(wire::read(&v), Wire::Result { .. }) {
                            return true;
                        }
                    }
                    Incoming::Call(c) => {
                        let _ = c.answer.send(Answer::Refused(NOT_RUN.into()));
                    }
                    Incoming::Ended => return false,
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        if !ended {
            warn!("claude code: the interrupted turn didn't end; starting the program again");
            self.stop();
        }
    }
}

/// The operator's side of what is new: the trailing turns of theirs (and Ghost's notes)
/// after the last reply or result.
fn trailing_turns(tail: &[Message]) -> (usize, String) {
    let start = tail
        .iter()
        .rposition(|m| matches!(m, Message::Assistant { .. } | Message::Tool { .. }))
        .map_or(0, |i| i + 1);
    let text = tail[start..]
        .iter()
        .filter_map(|m| match m {
            Message::User(t) => Some(t.clone()),
            Message::System(t) => Some(format!("(Note: {t})")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (tail.len() - start, text)
}

/// The history before a fresh process's first turn, as the program is given it: the
/// operator's words and Ghost's, and the names of the calls made. What a tool returned is
/// left out: it was an app's text, and it stays inside the bounds it was given in.
pub fn history(messages: &[Message]) -> String {
    let lines: Vec<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::User(t) => Some(format!("Operator: {t}")),
            Message::Assistant { text, tool_calls } if !text.trim().is_empty() => {
                Some(format!("You: {text}"))
            }
            Message::Assistant { tool_calls, .. } if !tool_calls.is_empty() => Some(format!(
                "(you called {})",
                tool_calls
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Message::System(t) => Some(format!("Note: {t}")),
            _ => None,
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "(Earlier in this conversation, as Ghost's transcript keeps it; what tools returned is \
         left out.)\n{}\n\n(Now:)\n",
        lines.join("\n")
    )
}

/// Ghost's model, as the operator's Claude Code. One per session or run: dropped, its
/// process is stopped and its relay stops listening.
pub struct ClaudeCode {
    settings: Arc<Settings>,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for ClaudeCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCode")
            .field("model", &self.settings.model)
            .finish_non_exhaustive()
    }
}

type Tx = mpsc::UnboundedSender<Result<Event, LlmError>>;

impl ClaudeCode {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: Arc::new(settings),
            state: Arc::new(Mutex::new(State {
                fresh: true,
                ..State::default()
            })),
        }
    }

    pub fn model(&self) -> &str {
        &self.settings.model
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Start the process ahead of the first turn (a session's opening, during the
    /// greeting), with the prompt and tools that turn will have. Nothing is asked of the
    /// model. Started already with this prompt: nothing to do.
    pub async fn warm(&self, system: &str, tools: &Tools) -> Result<f64, LlmError> {
        let mut st = self.state.lock().await;
        if let Some(p) = st.proc.as_mut()
            && p.system == system
            && p.alive()
        {
            p.relay.set_tools(tools);
            return Ok(0.0);
        }
        st.stop();
        let began = Instant::now();
        st.proc = Some(start(&self.settings, system, tools).await?);
        Ok(began.elapsed().as_secs_f64())
    }

    /// The models the program says it takes (`initialize`'s list), from the running
    /// process when there is one.
    pub async fn models(&self) -> Result<Vec<(String, String)>, LlmError> {
        {
            let st = self.state.lock().await;
            if let Some(p) = &st.proc {
                return Ok(p.models.clone());
            }
        }
        let p = start(&self.settings, "", &Tools::default()).await?;
        Ok(p.models)
    }

    /// A session's model call: Ghost's context as it is now, the session's tools, the
    /// reply's events. Dropping the stream before it ends interrupts the program.
    pub fn stream(
        &self,
        context: Context,
        tools: &Tools,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = self.state.clone();
        let settings = self.settings.clone();
        let tools = tools.clone();
        tokio::spawn(async move {
            let mut st = state.lock_owned().await;
            turn(&mut st, &settings, context, &tools, &tx).await;
        });
        Box::pin(receiver_stream(rx))
    }

    /// One short answer with no tools (a compaction, a watch's model step): the program
    /// run once with `--output-format json`, the text read from its result.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, LlmError> {
        once(&self.settings, system, user).await
    }
}

fn receiver_stream<T: Send + 'static>(
    rx: mpsc::UnboundedReceiver<T>,
) -> impl Stream<Item = T> + Send {
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
}

/// What the program sent next, or why nothing came.
enum Next {
    Dropped,
    Got(Option<Incoming>),
    Quiet,
}

/// What reading a reply came to.
enum Ended {
    /// The reply is whole (its end, or its calls for the stage).
    Whole,
    /// The stage stopped listening: interrupt.
    Dropped,
    /// It failed; `interrupt` when the program is still on the turn.
    Failed { interrupt: bool },
    /// The process ended.
    Died,
}

async fn turn(
    st: &mut OwnedMutexGuard<State>,
    settings: &Settings,
    ctx: Context,
    tools: &Tools,
    tx: &Tx,
) {
    if st.draining {
        st.drain().await;
    }
    if let Some(p) = st.proc.as_mut()
        && !p.alive()
    {
        st.stop();
    }
    // A context that no longer begins with what the program was given, or another prompt:
    // a fresh process, with the history in its first turn.
    let same_prompt = st.proc.as_ref().is_some_and(|p| p.system == ctx.system);
    if !same_prompt || !ctx.messages.starts_with(&st.known) {
        if st.proc.is_some() {
            info!("claude code: the conversation changed under the program; starting it again");
        }
        st.stop();
    }
    if st.proc.is_none() {
        match start(settings, &ctx.system, tools).await {
            Ok(p) => st.proc = Some(p),
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        }
    }
    if let Some(p) = &st.proc {
        p.relay.set_tools(tools);
    }
    let tail: Vec<Message> = ctx.messages[st.known.len()..].to_vec();
    // The stage's results, to their parked calls.
    let mut delivered = 0;
    for m in &tail {
        if let Message::Tool {
            tool_call_id,
            content,
        } = m
            && let Some(i) = st.parked.iter().position(|(id, _)| id == tool_call_id)
        {
            let (_, answer) = st.parked.remove(i);
            let _ = answer.send(Answer::Result(content.clone()));
            delivered += 1;
        }
    }
    let (n_turns, said) = trailing_turns(&tail);
    let preamble = if st.fresh {
        history(&ctx.messages[..ctx.messages.len() - n_turns])
    } else {
        String::new()
    };
    if !said.is_empty() || !preamble.is_empty() || (delivered == 0 && st.parked.is_empty()) {
        if !st.parked.is_empty() {
            // Calls the stage never made (the operator went on): refused, and the program
            // stopped on them before the new turn.
            st.refuse_parked();
            if let Some(p) = st.proc.as_mut() {
                p.interrupt();
            }
            st.drain().await;
            if st.proc.is_none() {
                // It wouldn't stop: a fresh one, with the history.
                match start(settings, &ctx.system, tools).await {
                    Ok(p) => st.proc = Some(p),
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
                let text = format!(
                    "{}{}",
                    history(&ctx.messages[..ctx.messages.len() - n_turns]),
                    said
                );
                return feed_and_read(st, settings, ctx, &text, tx).await;
            }
        }
        if delivered == 0 || !said.is_empty() {
            let text = if said.is_empty() && preamble.is_empty() {
                "(Go on.)".to_string()
            } else {
                format!("{preamble}{said}")
            };
            return feed_and_read(st, settings, ctx, &text, tx).await;
        }
    }
    st.known = ctx.messages;
    read_and_finish(st, settings, tx).await;
}

async fn feed_and_read(
    st: &mut OwnedMutexGuard<State>,
    settings: &Settings,
    ctx: Context,
    text: &str,
    tx: &Tx,
) {
    let sent = st.proc.as_ref().is_some_and(|p| p.send(wire::user(text)));
    if !sent {
        st.stop();
        let _ = tx.send(Err(LlmError::Program(ProgramFailure::Died)));
        return;
    }
    st.fresh = false;
    st.known = ctx.messages;
    read_and_finish(st, settings, tx).await;
}

async fn read_and_finish(st: &mut OwnedMutexGuard<State>, settings: &Settings, tx: &Tx) {
    match read(st, settings, tx).await {
        Ended::Whole => {}
        Ended::Dropped => {
            st.refuse_parked();
            if let Some(p) = st.proc.as_mut() {
                p.interrupt();
            }
            st.drain().await;
        }
        Ended::Failed { interrupt } => {
            st.refuse_parked();
            if interrupt && let Some(p) = st.proc.as_mut() {
                p.interrupt();
            }
            st.draining = true;
        }
        Ended::Died => {
            warn!("claude code: the program stopped during a turn");
            st.stop();
            let _ = tx.send(Err(LlmError::Program(ProgramFailure::Died)));
        }
    }
}

/// Read the program's reply as events, until it is whole, the stage stops listening, or
/// it fails.
async fn read(st: &mut OwnedMutexGuard<State>, settings: &Settings, tx: &Tx) -> Ended {
    let mut blocks = 0usize; // tool_use blocks of the reply in flight
    let mut in_message = false;
    let mut stopped_for_tools = false;
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut gather_until: Option<tokio::time::Instant> = None;
    let mut usage = Usage::default();
    loop {
        let deadline =
            gather_until.unwrap_or_else(|| tokio::time::Instant::now() + settings.quiet_most);
        let next = {
            let Some(p) = st.proc.as_mut() else {
                return Ended::Died;
            };
            tokio::select! {
                biased;
                () = tx.closed() => Next::Dropped,
                m = p.rx.recv() => Next::Got(m),
                () = tokio::time::sleep_until(deadline) => Next::Quiet,
            }
        };
        let next = match next {
            Next::Dropped => return Ended::Dropped,
            Next::Quiet if gather_until.is_some() => return flush(calls, tx),
            Next::Quiet => {
                let _ = tx.send(Err(LlmError::Unreachable {
                    host: "Claude Code".into(),
                    kind: "ReadTimeout",
                }));
                return Ended::Failed { interrupt: true };
            }
            Next::Got(m) => m,
        };
        let Some(m) = next else { return Ended::Died };
        match m {
            Incoming::Ended => return Ended::Died,
            Incoming::Call(parked) => {
                st.next_id += 1;
                let id = format!("ghost-cc-{}", st.next_id);
                calls.push(ToolCall::new(id.clone(), parked.name, parked.arguments));
                st.parked.push((id, parked.answer));
                // A call with no reply in flight (the next of a batch the program runs one
                // at a time), or the last the reply announced: the reply's calls are these,
                // once no other follows within `GATHER`.
                if !in_message || (stopped_for_tools && calls.len() >= blocks) {
                    gather_until = Some(tokio::time::Instant::now() + GATHER);
                }
            }
            Incoming::Line(v) => match wire::read(&v) {
                Wire::MessageStart(u) => {
                    in_message = true;
                    blocks = 0;
                    usage = u;
                }
                Wire::ToolStarting(name) => {
                    blocks += 1;
                    let name = name
                        .strip_prefix(wire::TOOL_PREFIX)
                        .unwrap_or(&name)
                        .to_string();
                    if tx.send(Ok(Event::ToolStarting(name))).is_err() {
                        return Ended::Dropped;
                    }
                }
                Wire::Text(t) => {
                    if !t.is_empty() && tx.send(Ok(Event::Text(t))).is_err() {
                        return Ended::Dropped;
                    }
                }
                Wire::Thought(t) => {
                    if !t.is_empty() && tx.send(Ok(Event::Thought(t))).is_err() {
                        return Ended::Dropped;
                    }
                }
                Wire::MessageDelta { stop, output } => {
                    usage.output_tokens = output;
                    let _ = tx.send(Ok(Event::Usage(usage.clone())));
                    stopped_for_tools = stop.as_deref() == Some("tool_use");
                }
                Wire::MessageStop => {
                    in_message = false;
                    if stopped_for_tools {
                        // Its calls reach the relay as each block closes; wait for the rest
                        // the reply announced, a moment at most once one is here.
                        if !calls.is_empty() && calls.len() >= blocks {
                            gather_until = Some(tokio::time::Instant::now() + GATHER);
                        } else if !calls.is_empty() {
                            gather_until = Some(tokio::time::Instant::now() + GATHER * 4);
                        }
                    }
                }
                Wire::Retry {
                    status,
                    kind,
                    attempt,
                    delay_ms,
                } => {
                    if let Some(e) = wire::retry_verdict(status, &kind, attempt, delay_ms) {
                        let _ = tx.send(Err(e));
                        return Ended::Failed { interrupt: true };
                    }
                    info!(
                        "claude code: the provider asked for a wait; the program tries once more"
                    );
                }
                Wire::LimitReached => {
                    let _ = tx.send(Err(LlmError::Program(ProgramFailure::Limit)));
                    return Ended::Failed { interrupt: true };
                }
                Wire::SyntheticError(kind) => {
                    let _ = tx.send(Err(wire::error_of(&kind, None)));
                    return Ended::Failed { interrupt: false };
                }
                Wire::Result { error, status, .. } => {
                    if error {
                        let _ = tx.send(Err(wire::error_of("", status)));
                        return Ended::Whole;
                    }
                    if !calls.is_empty() {
                        return flush(calls, tx);
                    }
                    let _ = tx.send(Ok(Event::Finished(FinishReason::End)));
                    return Ended::Whole;
                }
                Wire::Ready { .. } | Wire::BlockStart | Wire::Other => {}
            },
        }
    }
}

/// The reply's calls, for the stage: each, then the reply's end. The program waits on
/// them at the relay until the stage's next call brings their results.
fn flush(calls: Vec<ToolCall>, tx: &Tx) -> Ended {
    for c in calls {
        if tx.send(Ok(Event::ToolCall(c))).is_err() {
            return Ended::Dropped;
        }
    }
    if tx.send(Ok(Event::Finished(FinishReason::ToolUse))).is_err() {
        return Ended::Dropped;
    }
    Ended::Whole
}

/// The program run once: Ghost's system prompt, one question on standard input, no tools,
/// no apps, the same switches as a session's.
async fn once(settings: &Settings, system: &str, user: &str) -> Result<String, LlmError> {
    if !settings.binary.is_file() {
        return Err(LlmError::Program(ProgramFailure::Missing));
    }
    if !missing_switches(settings).await.is_empty() {
        return Err(LlmError::Program(ProgramFailure::Unsupported));
    }
    let work = tempfile::Builder::new()
        .prefix("ghost-claude-")
        .tempdir()
        .map_err(|_| LlmError::Program(ProgramFailure::Failed))?;
    let apps = work.path().join("apps.json");
    write_private(&apps, r#"{"mcpServers":{}}"#)
        .map_err(|_| LlmError::Program(ProgramFailure::Failed))?;
    let mut args = argv(settings, system, &apps);
    // One answer as JSON, from text on standard input.
    for (i, a) in args.clone().iter().enumerate() {
        if a == "--input-format" {
            args[i + 1] = "text".into();
        }
        if a == "--output-format" {
            args[i + 1] = "json".into();
        }
    }
    args.retain(|a| a != "--include-partial-messages" && a != "--verbose");
    let mut child = Command::new(&settings.binary)
        .args(args)
        .env_clear()
        .envs(environment(settings))
        .current_dir(work.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| LlmError::Program(ProgramFailure::Missing))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(user.as_bytes()).await;
    }
    let out = tokio::time::timeout(settings.quiet_most, child.wait_with_output())
        .await
        .map_err(|_| LlmError::Unreachable {
            host: "Claude Code".into(),
            kind: "ReadTimeout",
        })?
        .map_err(|_| LlmError::Program(ProgramFailure::Died))?;
    let v: Value =
        serde_json::from_slice(&out.stdout).map_err(|_| LlmError::Program(ProgramFailure::Died))?;
    if v["is_error"].as_bool().unwrap_or(false) {
        let status = v["api_error_status"].as_u64().map(|s| s as u16);
        let text = v["result"].as_str().unwrap_or("").to_lowercase();
        // The kind only: whether its words name the sign-in or a limit.
        let kind =
            if text.contains("authenticat") || text.contains("log in") || text.contains("login") {
                "authentication_failed"
            } else if text.contains("limit") {
                "rate_limit"
            } else {
                ""
            };
        return Err(wire::error_of(kind, status));
    }
    Ok(v["result"].as_str().unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn settings() -> Settings {
        let started: HashMap<String, String> = [
            ("HOME", "/home/op"),
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_BASE_URL", "http://localhost:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "proxy-token-0123456789"),
            ("CLAUDE_CODE_ENTRYPOINT", "sdk-ts"),
            ("CLAUDECODE", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        Settings::new("/usr/bin/claude".into(), "haiku", started)
    }

    #[test]
    fn the_program_runs_with_its_own_tools_settings_and_apps_off_and_nothing_inherited() {
        let s = settings();
        let a = argv(&s, "You are Ghost.", Path::new("/tmp/x/apps.json"));
        let has = |pair: &[&str]| {
            a.windows(pair.len())
                .any(|w| w.iter().zip(pair).all(|(x, y)| x == y))
        };
        assert_eq!(a[0], "--print");
        assert!(has(&["--input-format", "stream-json"]));
        assert!(has(&["--output-format", "stream-json"]));
        assert!(has(&["--tools", ""]), "built-in tools off");
        assert!(
            has(&["--setting-sources", ""]),
            "no user or project settings, hooks or plugins"
        );
        assert!(has(&[
            "--strict-mcp-config",
            "--mcp-config",
            "/tmp/x/apps.json"
        ]));
        assert!(
            a.contains(&"--allowedTools=mcp__ghost__*".to_string()),
            "the variadic flag in its = form"
        );
        assert!(
            a.contains(&"--disable-slash-commands".to_string()),
            "no skills or commands"
        );
        assert!(a.contains(&"--no-session-persistence".to_string()));
        assert!(
            has(&["--permission-mode", "dontAsk"]),
            "anything not allowed is refused, never asked"
        );
        assert!(has(&["--system-prompt", "You are Ghost."]));
        assert!(has(&["--model", "haiku"]));
        assert!(
            !a.iter().any(|x| x == "--bare"),
            "--bare never reads the sign-in"
        );
        assert!(!a.iter().any(|x| x.contains("dangerously")));
        // Every switch Ghost uses is one the detection requires the program to have.
        for x in a.iter().filter(|x| x.starts_with("--")) {
            let name = x.split('=').next().unwrap();
            assert!(
                program::REQUIRED_SWITCHES.contains(&name),
                "{name} is used but not required of the program"
            );
        }
        let env = environment(&s);
        assert_eq!(
            env.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "ENABLE_CLAUDEAI_MCP_SERVERS",
                "HOME",
                "LANG",
                "MCP_TOOL_TIMEOUT",
                "PATH"
            ]
        );
        assert!(
            env.keys()
                .all(|k| !k.starts_with("ANTHROPIC") && !k.starts_with("CLAUDE"))
        );
    }

    #[test]
    fn a_fresh_process_hears_the_history_without_what_tools_returned() {
        let msgs = vec![
            Message::user("what's on my list?"),
            Message::reply(
                "",
                vec![ToolCall::new("c1", "github_issue_list", Map::new())],
            ),
            Message::result("c1", &serde_json::json!({"secret": "app text"})),
            Message::assistant("Three issues."),
        ];
        let h = history(&msgs);
        assert!(h.contains("Operator: what's on my list?"));
        assert!(h.contains("(you called github_issue_list)"));
        assert!(h.contains("You: Three issues."));
        assert!(!h.contains("app text"), "a tool's result stays out: {h}");
        assert_eq!(history(&[]), "");
    }

    #[test]
    fn the_new_turns_are_the_ones_after_the_last_reply() {
        let tail = vec![
            Message::assistant("Sure."),
            Message::user("and then?"),
            Message::system("the operator switched agents"),
        ];
        let (n, said) = trailing_turns(&tail);
        assert_eq!(n, 2);
        assert_eq!(said, "and then?\n\n(Note: the operator switched agents)");
    }
}
