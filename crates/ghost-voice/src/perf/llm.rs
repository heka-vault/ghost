//! Model and endpoint latency with Ghost's real system prompt and tool schemas
//! (`ghostd.perf.llm`).
//!
//! Three probes per (endpoint, model), each run `n` times, streamed the way a session
//! streams (cache markers on the last two user messages, as Pipecat's Anthropic adapter
//! did):
//!
//! - `plain`: a hello, no tool. TTFB, total, and the stream span (first byte to last;
//!   near zero with many output tokens means the endpoint buffered the reply).
//! - `tool`: "what needs my sign-off": the call to a GitHub read, then the answer
//!   after a canned tool result. Time to the complete tool call, then the answer's TTFB.
//! - caching: every repeat after the first should read the prefix from the cache.
//!
//! Endpoints: `current` is the model connection as configured (ADR 0037), through the
//! same client a session uses, on whichever protocol it speaks. Any other endpoint is built
//! from its own provider, whatever the configured one is, and reached over that provider's
//! protocol: `proxy` is CLIProxy (ANTHROPIC_BASE_URL with ANTHROPIC_AUTH_TOKEN),
//! `openrouter` is OpenRouter's Anthropic Messages API with OPENROUTER_API_KEY, and a
//! provider id (`anthropic`, `openai`, `ollama`, ...) is that provider as it comes.
//! `--openrouter-models` go through OpenRouter's OpenAI-compatible API. Keys are never
//! printed. Spend is small: a few thousand input tokens per call, mostly cached.
//!
//! Timing is `std::time::Instant` around `Model::stream`. The Rust client delivers a
//! reply's tool calls once the reply is whole, so a reply that is only a tool call has its
//! TTFB at the call, where the Python saw the block open.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use ghost_core::apps::{AppSpec, Apps, ProcessEnviron};
use ghost_core::connection::{Env, ModelConnection, PROVIDERS, Protocol, Target, provider};
use ghost_core::{AutonomyPolicy, Store, WriteGate};
use ghost_llm::{
    Context, Event, FinishReason, LLM_MAX_TOKENS, LlmError, Model, Options, ToolCall, Tools, Usage,
};
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;

use super::harness::{Bench, PERF_POLICY};
use crate::config::VoiceSettings;
use crate::intercept::no_action;
use crate::prompt::{Persona, build_system_prompt};
use crate::tools::{ToolDispatcher, ToolsParams};

pub const OPENROUTER_OPENAI: &str = "https://openrouter.ai/api/v1";
pub const HELLOS: [&str; 6] = [
    "Hi Ghost, how's it going?",
    "Hey Ghost, you there?",
    "Morning, Ghost.",
    "Hello Ghost, what's up?",
    "Ghost, are you awake?",
    "Hey, good afternoon.",
];
pub const ASKS: [&str; 6] = [
    "What needs my sign-off?",
    "Anything waiting for my sign-off?",
    "What's waiting on me to sign off?",
    "Which issues need my sign-off right now?",
    "Is there anything for me to sign off on?",
    "What do I have to sign off today?",
];
pub const CLAUDE_MODELS: [&str; 2] = ["claude-sonnet-5-5", "claude-haiku-5-5"];
/// The older endpoints, with or without +sysbp.
pub const NAMED: [&str; 2] = ["proxy", "openrouter"];

/// The canned result of the GitHub read, for the answer probe.
pub fn tool_result() -> Value {
    json!({
        "count": 3,
        "issues": [
            {
                "repo": "ghost-mock/infra",
                "number": 17,
                "title": "Rotate the Discord webhook",
                "labels": ["needs-signoff", "security"],
                "updated_at": "2026-10-16T09:00:00Z",
            },
            {
                "repo": "ghost-mock/app",
                "number": 151,
                "title": "Add dark mode toggle to settings",
                "labels": ["needs-signoff", "frontend"],
                "updated_at": "2026-10-16T09:00:00Z",
            },
            {
                "repo": "ghost-mock/app",
                "number": 142,
                "title": "Choose Redis or Postgres for the session cache",
                "labels": ["needs-signoff", "backend"],
                "updated_at": "2026-10-16T09:00:00Z",
            },
        ],
    })
}

#[derive(Debug, Error)]
pub enum PerfLlmError {
    /// Names what is known; never a value.
    #[error("unknown endpoint '{endpoint}' ({known}; a provider takes +sysbp)")]
    UnknownEndpoint { endpoint: String, known: String },
    /// The client for a target couldn't be built (its Display is already redacted).
    #[error("{0}")]
    Client(#[from] LlmError),
}

/// Haiku rejects `between_tools` (HTTP 400 on both endpoints, 2026-10-19): compare it
/// with thinking off unless a mode was asked for explicitly (`ghostd.perf.harness`).
pub fn thinking_for(model: &str, requested: Option<&str>) -> Option<String> {
    if requested.is_none() && model.contains("haiku") {
        return Some("disabled".into());
    }
    requested.map(String::from)
}

/// One probe's timings, in seconds.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub ttfb: Option<f64>,
    pub total: Option<f64>,
    /// First byte -> last byte.
    pub span: Option<f64>,
    /// The tool call complete (stop_reason tool_use).
    pub tool_done: Option<f64>,
    pub usage: Option<Usage>,
    pub tool: Option<String>,
    /// In the spoken answer.
    pub words: Option<usize>,
    pub error: Option<String>,
}

/// What the model said in a probe: the text and the calls it made.
#[derive(Clone, Debug, Default)]
pub struct Said {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// One row of the run: the target's own client for its protocol, and what its requests
/// carry (thinking on the Messages API, the cap and reasoning effort on Chat Completions,
/// which the client reads off the target).
#[derive(Debug)]
pub struct TargetRow {
    pub ep: String,
    pub label: String,
    /// The model as the provider names it.
    pub wire: String,
    pub client: Arc<Model>,
    pub kind: Protocol,
    pub target: Target,
    /// The Messages API only.
    pub thinking: Option<String>,
    /// The cache marker on the system prompt; the Messages API only.
    pub sysbp: bool,
}

/// Where `ghost-mock-github` is: on the PATH, or beside this binary.
fn mock_github_command() -> String {
    Bench::mock_github_command()
}

/// The system prompt and tool schemas a real session sends, from the mock GitHub app
/// the example connects.
pub async fn ghost_prompt_and_tools(infra_dir: &Path) -> anyhow::Result<(String, Tools)> {
    let tmp = tempfile::Builder::new()
        .prefix("ghost-perf-llm-")
        .tempdir()?;
    let store = Arc::new(Store::open(tmp.path().join("ghost.db"))?);
    let autonomy = match AutonomyPolicy::load(infra_dir.join("policy.example.toml")) {
        Ok(p) => p,
        Err(_) => AutonomyPolicy::parse(PERF_POLICY, "perf").map_err(|e| anyhow::anyhow!("{e}"))?,
    };
    let mut spec = AppSpec::runs("github", &mock_github_command());
    spec.title = "GitHub".into();
    spec.args = vec![
        "--state".into(),
        tmp.path()
            .join("mock-github.json")
            .to_string_lossy()
            .into_owned(),
    ];
    let apps = Arc::new(Apps::new([spec], Arc::new(ProcessEnviron), None));
    apps.connect().await;
    if !apps.apps().contains_key("github") {
        anyhow::bail!("mock GitHub server did not start: {:?}", apps.failed());
    }
    let sources = apps.sources();
    let gate = Arc::new(WriteGate::new(store.clone()).with_autonomy(autonomy));
    for a in &sources {
        gate.register(&a.name, a.operations.clone());
    }
    let fragments: Vec<String> = sources
        .iter()
        .map(|a| a.prompt.clone())
        .filter(|f| !f.is_empty())
        .collect();
    let dispatcher = ToolDispatcher::new(ToolsParams {
        sources,
        gate,
        session_id: "x".into(),
        on_hang_up: no_action(),
        on_hold: None,
        events: None,
        conversations: None,
        on_call: None,
        last_request: None,
        probe: None,
    });
    let tools = dispatcher.schemas();
    let prompt = build_system_prompt(&fragments, &Persona::ghost());
    apps.close().await;
    drop(store);
    drop(tmp);
    Ok((prompt, tools))
}

/// The connection a named endpoint stands for, built from its own provider so nothing
/// of the configured connection (provider, address, protocol, password) leaks into it.
///
/// `proxy` is CLIProxy: ANTHROPIC_BASE_URL with ANTHROPIC_AUTH_TOKEN (CLIPROXY_BASE_URL,
/// then CLIProxy's own address, when it isn't set). `openrouter` is OpenRouter's
/// Anthropic API with OPENROUTER_API_KEY. A provider id is that provider as it comes.
pub fn endpoint_connection(
    ep: &str,
    voice: &VoiceSettings,
) -> Result<ModelConnection, PerfLlmError> {
    let name = ep.strip_suffix("+sysbp").unwrap_or(ep);
    let mut env: HashMap<String, String> = HashMap::new();
    if name == "proxy" {
        env.insert("GHOST_LLM_PROVIDER".into(), "cliproxy".into());
        for key in ["ANTHROPIC_BASE_URL", "CLIPROXY_BASE_URL"] {
            if let Some(v) = voice.var(key).filter(|v| !v.trim().is_empty()) {
                let name = if key == "ANTHROPIC_BASE_URL" {
                    "GHOST_LLM_BASE_URL"
                } else {
                    key
                };
                env.insert(name.into(), v.to_string());
            }
        }
    } else if provider(name).is_some() {
        env.insert("GHOST_LLM_PROVIDER".into(), name.into());
    } else {
        let known = std::iter::once("current")
            .chain(NAMED)
            .chain(PROVIDERS.iter().map(|p| p.id))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(PerfLlmError::UnknownEndpoint {
            endpoint: ep.to_string(),
            known,
        });
    }
    Ok(ModelConnection::from_env(&env))
}

fn target_row(
    ep: &str,
    target: Target,
    thinking: &str,
    sysbp: bool,
) -> Result<TargetRow, PerfLlmError> {
    let client = ghost_llm::shared(&target)?;
    let (thinking, sysbp) = match target.protocol {
        Protocol::Anthropic => {
            let haiku = target.model.contains("haiku");
            (
                thinking_for(&target.model, if haiku { None } else { Some(thinking) }),
                sysbp,
            )
        }
        Protocol::Openai => (None, false),
    };
    Ok(TargetRow {
        ep: ep.to_string(),
        label: target.model.clone(),
        wire: target.model.clone(),
        client,
        kind: target.protocol,
        thinking,
        sysbp,
        target,
    })
}

/// Every (endpoint, model) the run measures. `current` is the connection as configured,
/// with its own model; a named endpoint runs each of `models`, or, when none are given,
/// the Claude defaults on Anthropic's API and the provider's default model on an
/// OpenAI-compatible one. An endpoint whose key is missing is skipped, saying why on
/// `out` (the endpoint's name and the problem; never a key).
pub fn build_targets(
    endpoints: &[String],
    models: &[String],
    openrouter_models: &[String],
    thinking: &str,
    voice: &VoiceSettings,
    env: &HashMap<String, String>,
    out: &mut dyn Write,
) -> Result<Vec<TargetRow>, PerfLlmError> {
    let mut targets = Vec::new();
    for ep in endpoints {
        if ep == "current" {
            // What a session sends now: the connection's protocol, model and shared client.
            targets.push(target_row(
                "current",
                voice.llm_target(Some(env)),
                thinking,
                true,
            )?);
            continue;
        }
        let conn = endpoint_connection(ep, voice)?;
        let problems = conn.problems(env, false);
        if !problems.is_empty() {
            let _ = writeln!(out, "  skipping {ep}: {}", problems.join("; "));
            continue;
        }
        let wanted: Vec<String> = if !models.is_empty() {
            models.to_vec()
        } else if conn.protocol == Protocol::Anthropic {
            CLAUDE_MODELS.iter().map(|m| m.to_string()).collect()
        } else {
            vec![String::new()] // the provider's default
        };
        for m in wanted {
            let mut c = conn.clone();
            c.model = m.clone();
            let target = c.target(env);
            let mut row = target_row(ep, target, thinking, ep.ends_with("+sysbp"))?;
            if !m.is_empty() {
                row.label = m;
            }
            targets.push(row);
        }
    }
    let or_key = env.get("OPENROUTER_API_KEY").map_or("", String::as_str);
    if !openrouter_models.is_empty() && !or_key.is_empty() {
        for m in openrouter_models {
            let env_or: HashMap<String, String> = [
                ("GHOST_LLM_PROVIDER", "custom"),
                ("GHOST_LLM_PROTOCOL", "openai"),
                ("GHOST_LLM_BASE_URL", OPENROUTER_OPENAI),
                ("GHOST_LLM_PASSWORD", "OPENROUTER_API_KEY"),
                ("GHOST_LLM_MODEL", m.as_str()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            let target = ModelConnection::from_env(&env_or).target(env);
            targets.push(target_row("openrouter-openai", target, thinking, false)?);
        }
    }
    Ok(targets)
}

/// One streamed request against a target, timed. The sample reports, it doesn't fail: an
/// error is the `LlmError`'s Display, already redacted; never a body or a header.
async fn call(t: &TargetRow, tools: &Tools, context: Context) -> (Sample, Said) {
    let options = Options {
        max_tokens: LLM_MAX_TOKENS,
        thinking: match t.kind {
            Protocol::Anthropic => t.thinking.clone().filter(|s| !s.is_empty()),
            Protocol::Openai => None,
        },
        cache_system: t.sysbp,
    };
    let mut s = Sample::default();
    let mut said = Said::default();
    let began = Instant::now();
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;
    let mut content = |now: Instant| {
        if first.is_none() {
            first = Some(now);
        }
        last = Some(now);
    };
    let mut stream = std::pin::pin!(t.client.stream(context, tools, &options));
    let mut failed = false;
    while let Some(item) = stream.next().await {
        let now = Instant::now();
        match item {
            Ok(Event::Text(piece)) => {
                content(now);
                said.text.push_str(&piece);
            }
            Ok(Event::Thought(_)) => content(now),
            Ok(Event::ToolCall(c)) => {
                content(now);
                if s.tool_done.is_none() {
                    s.tool_done = Some(now.duration_since(began).as_secs_f64());
                }
                if s.tool.is_none() {
                    s.tool = Some(c.name.clone());
                }
                said.calls.push(c);
            }
            Ok(Event::Usage(u)) => s.usage = Some(u),
            Ok(Event::Finished(reason)) => {
                if reason == FinishReason::ToolUse && s.tool_done.is_none() {
                    s.tool_done = Some(now.duration_since(began).as_secs_f64());
                }
                // The reply is over; a cut reply's redo is the session's recovery, not
                // this probe's.
                break;
            }
            Ok(_) => {}
            Err(e) => {
                s.error = Some(e.to_string());
                failed = true;
                break;
            }
        }
    }
    if !failed {
        s.total = Some(began.elapsed().as_secs_f64());
        if let (Some(f), Some(l)) = (first, last) {
            s.span = Some(l.duration_since(f).as_secs_f64());
        }
    }
    s.ttfb = first.map(|f| f.duration_since(began).as_secs_f64());
    if !said.text.is_empty() {
        s.words = Some(said.text.split_whitespace().count());
    }
    (s, said)
}

/// Python's `statistics.median`: the middle value, or the mean of the two middle ones.
fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

pub fn p50(xs: &[Option<f64>]) -> Option<f64> {
    let mut v: Vec<f64> = xs.iter().flatten().copied().collect();
    median(&mut v)
}

/// The value at `round(0.95 * (n - 1))`, Python's round (half to even).
pub fn p95(xs: &[Option<f64>]) -> Option<f64> {
    let mut v: Vec<f64> = xs.iter().flatten().copied().collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let i = (0.95 * (v.len() - 1) as f64).round_ties_even() as usize;
    Some(v[i.min(v.len() - 1)])
}

fn f(x: Option<f64>) -> String {
    x.map_or_else(|| "-".to_string(), |x| format!("{x:.2}"))
}

/// Python's `None` or the number, as an f-string showed it.
fn int_or_none(x: Option<i64>) -> String {
    x.map_or_else(|| "None".to_string(), |x| x.to_string())
}

/// Python's `None` or the float, as an f-string showed it (`42.0`, `41.5`).
fn float_or_none(x: Option<f64>) -> String {
    x.map_or_else(|| "None".to_string(), |x| format!("{x:?}"))
}

/// The first request's usage, as the Python kept it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct UsageJson {
    #[serde(rename = "in")]
    pub input: u64,
    pub cache_read: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<u64>,
    pub out: u64,
}

impl From<&Usage> for UsageJson {
    fn from(u: &Usage) -> Self {
        Self {
            input: u.input_tokens,
            cache_read: u.cache_read_tokens.unwrap_or(0),
            cache_write: u.cache_write_tokens,
            out: u.output_tokens,
        }
    }
}

/// One target's row of `llm.json`, the Python's keys in its order.
#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub endpoint: String,
    pub model: String,
    pub n: usize,
    pub hello_ttfb: Option<f64>,
    pub hello_ttfb_p95: Option<f64>,
    pub hello_total: Option<f64>,
    pub hello_span: Option<f64>,
    pub hello_out: Option<i64>,
    pub tool_done: Option<f64>,
    pub tool_done_p95: Option<f64>,
    pub answer_ttfb: Option<f64>,
    pub answer_total: Option<f64>,
    pub answer_span: Option<f64>,
    pub answer_out: Option<i64>,
    pub cold_ttfb: Option<f64>,
    pub warm_ttfb: Option<f64>,
    pub tool_ok: String,
    pub words: Option<f64>,
    /// `read/write`, the medians after the first round; `-` where nothing reported.
    pub cache: String,
    pub in_first: Option<UsageJson>,
    pub errors: String,
}

fn median_int(xs: &[u64]) -> Option<i64> {
    let mut v: Vec<f64> = xs.iter().map(|x| *x as f64).collect();
    median(&mut v).map(|m| m as i64)
}

pub fn row(ep: &str, model: &str, plain: &[Sample], tool1: &[Sample], tool2: &[Sample]) -> Row {
    let later: Vec<&Sample> = plain.iter().skip(1).chain(tool1.iter().skip(1)).collect();
    let reads: Vec<u64> = later
        .iter()
        .filter_map(|s| s.usage.as_ref())
        .map(|u| u.cache_read_tokens.unwrap_or(0))
        .collect();
    let writes: Vec<u64> = later
        .iter()
        .filter_map(|s| s.usage.as_ref())
        .map(|u| u.cache_write_tokens.unwrap_or(0))
        .collect();
    let mut errors: Vec<&str> = plain
        .iter()
        .chain(tool1)
        .chain(tool2)
        .filter_map(|s| s.error.as_deref())
        .collect();
    errors.sort_unstable();
    errors.dedup();
    let outs: Vec<u64> = plain
        .iter()
        .filter_map(|s| s.usage.as_ref())
        .map(|u| u.output_tokens)
        .collect();
    let answer_outs: Vec<u64> = tool2
        .iter()
        .filter_map(|s| s.usage.as_ref())
        .map(|u| u.output_tokens)
        .collect();
    let later_ttfb: Vec<Option<f64>> = later.iter().map(|s| s.ttfb).collect();
    let ttfbs = |xs: &[Sample]| xs.iter().map(|s| s.ttfb).collect::<Vec<_>>();
    let cache = |xs: &[u64]| median_int(xs).map_or_else(|| "-".to_string(), |m| m.to_string());
    Row {
        endpoint: ep.to_string(),
        model: model.to_string(),
        n: plain.len(),
        hello_ttfb: p50(&ttfbs(plain)),
        hello_ttfb_p95: p95(&ttfbs(plain)),
        hello_total: p50(&plain.iter().map(|s| s.total).collect::<Vec<_>>()),
        hello_span: p50(&plain.iter().map(|s| s.span).collect::<Vec<_>>()),
        hello_out: median_int(&outs),
        tool_done: p50(&tool1.iter().map(|s| s.tool_done).collect::<Vec<_>>()),
        tool_done_p95: p95(&tool1.iter().map(|s| s.tool_done).collect::<Vec<_>>()),
        answer_ttfb: p50(&ttfbs(tool2)),
        answer_total: p50(&tool2.iter().map(|s| s.total).collect::<Vec<_>>()),
        answer_span: p50(&tool2.iter().map(|s| s.span).collect::<Vec<_>>()),
        answer_out: median_int(&answer_outs),
        cold_ttfb: plain.first().and_then(|s| s.ttfb),
        warm_ttfb: p50(&later_ttfb),
        tool_ok: format!(
            "{}/{}",
            tool1
                .iter()
                .filter(|s| s.tool.as_deref().is_some_and(|t| t.starts_with("github_")))
                .count(),
            tool1.len()
        ),
        words: p50(&tool2
            .iter()
            .map(|s| s.words.map(|w| w as f64))
            .collect::<Vec<_>>()),
        cache: format!("{}/{}", cache(&reads), cache(&writes)),
        in_first: plain
            .first()
            .and_then(|s| s.usage.as_ref())
            .map(UsageJson::from),
        errors: errors.join("; ").chars().take(300).collect(),
    }
}

pub fn line(r: &Row) -> String {
    let mut out = format!(
        "  {:<18} {:<32} hello ttfb {} (span {}, {} tok)  tool {}  answer ttfb {} total {}  \
         tool ok {}  cache {}",
        r.endpoint,
        r.model,
        f(r.hello_ttfb),
        f(r.hello_span),
        int_or_none(r.hello_out),
        f(r.tool_done),
        f(r.answer_ttfb),
        f(r.answer_total),
        r.tool_ok,
        r.cache,
    );
    if !r.errors.is_empty() {
        out.push_str("  ERR ");
        out.extend(r.errors.chars().take(120));
    }
    out
}

/// `llm.md`: the 16-column table.
pub fn markdown(rows: &[Row]) -> String {
    let mut md = vec![
        "| endpoint | model | hello ttfb p50 | p95 | hello total p50 | span p50 | out tok | \
         tool call done p50 | p95 | answer ttfb p50 | answer total p50 | answer span (out tok) \
         | tool ok | words p50 | cache read/write (2nd+) | errors |"
            .to_string(),
        format!("|{}", "---|".repeat(16)),
    ];
    for r in rows {
        md.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} ({}) | {} | {} | {} | {} |",
            r.endpoint,
            r.model,
            f(r.hello_ttfb),
            f(r.hello_ttfb_p95),
            f(r.hello_total),
            f(r.hello_span),
            int_or_none(r.hello_out),
            f(r.tool_done),
            f(r.tool_done_p95),
            f(r.answer_ttfb),
            f(r.answer_total),
            f(r.answer_span),
            int_or_none(r.answer_out),
            r.tool_ok,
            float_or_none(r.words),
            r.cache,
            r.errors,
        ));
    }
    md.join("\n") + "\n"
}

/// `json.dumps(rows, indent=1)`.
fn json_indent_1(rows: &[Row]) -> anyhow::Result<String> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    rows.serialize(&mut ser)?;
    Ok(String::from_utf8(buf)?)
}

/// The `llm` lane's arguments.
#[derive(Clone, Debug)]
pub struct LlmRun {
    pub models: Vec<String>,
    pub endpoints: Vec<String>,
    pub openrouter_models: Vec<String>,
    pub thinking: String,
    pub n: usize,
    pub out: PathBuf,
    /// Where `policy.example.toml` is.
    pub infra_dir: PathBuf,
}

pub async fn run_llm(a: &LlmRun) -> anyhow::Result<i32> {
    let (system, tools) = ghost_prompt_and_tools(&a.infra_dir).await?;
    let voice = VoiceSettings::from_env()?;
    let env: HashMap<String, String> = std::env::vars().collect();

    // Every target runs once per round, so a slow minute on one endpoint hits all of them.
    // Each round's lines differ, like a new session's first words: only a prefix up to the
    // system prompt and tools can come from the cache, and only if something marks it.
    let targets = build_targets(
        &a.endpoints,
        &a.models,
        &a.openrouter_models,
        &a.thinking,
        &voice,
        &env,
        &mut std::io::stdout(),
    )?;
    let mut samples: Vec<(Vec<Sample>, Vec<Sample>, Vec<Sample>)> =
        (0..targets.len()).map(|_| Default::default()).collect();
    for rnd in 0..a.n {
        let (hello, ask) = (HELLOS[rnd % HELLOS.len()], ASKS[rnd % ASKS.len()]);
        for (t, (plain, tool1, tool2)) in targets.iter().zip(samples.iter_mut()) {
            let (s0, _) = call(t, &tools, Context::new(&system).user(hello)).await;
            plain.push(s0);
            let (s1, said) = call(t, &tools, Context::new(&system).user(ask)).await;
            tool1.push(s1);
            if said.calls.is_empty() {
                continue;
            }
            let mut context = Context::new(&system).user(ask);
            context.push_reply(said.text, said.calls.clone());
            for c in &said.calls {
                context.push_result(&c.id, &tool_result());
            }
            let (s2, _) = call(t, &tools, context).await;
            tool2.push(s2);
        }
        println!("  round {}/{} done", rnd + 1, a.n);
    }
    let rows: Vec<Row> = targets
        .iter()
        .zip(&samples)
        .map(|(t, (plain, tool1, tool2))| row(&t.ep, &t.label, plain, tool1, tool2))
        .collect();
    for r in &rows {
        println!("{}", line(r));
    }
    std::fs::create_dir_all(&a.out)?;
    std::fs::write(a.out.join("llm.json"), json_indent_1(&rows)?)?;
    std::fs::write(a.out.join("llm.md"), markdown(&rows))?;
    println!("\nllm: {}", a.out.join("llm.md").display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    //! `ghostd perf llm`'s targets (ADR 0037): each named endpoint is its own provider,
    //! reached over that provider's protocol with its own key, whatever the configured
    //! connection is. No request is sent here.

    use ghost_core::connection::{CLIPROXY_DEFAULT_URL, REASONING_HEADROOM};

    use super::*;

    // The configured connection is OpenAI, the case the review found: before, `proxy` kept
    // the provider and sent the OpenAI key to api.openai.com as an Anthropic request.
    fn env() -> HashMap<String, String> {
        [
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_MODEL", "gpt-5.5"),
            ("OPENAI_API_KEY", "sk-openai-0123"),
            ("ANTHROPIC_AUTH_TOKEN", "sk-proxy-4567"),
            ("OPENROUTER_API_KEY", "sk-or-89ab"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    /// Every connection field from here, so the shell's own environment doesn't leak in.
    fn voice(over: &[(&str, &str)]) -> VoiceSettings {
        let mut fields: HashMap<String, String> = [
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_MODEL", "gpt-5.5"),
        ]
        .into_iter()
        .chain(over.iter().copied())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        fields.retain(|_, v| !v.is_empty());
        VoiceSettings::from_vars(|name| fields.get(name).cloned()).expect("valid settings")
    }

    fn targets(
        endpoints: &[&str],
        models: &[&str],
        over: &[(&str, &str)],
    ) -> Result<Vec<TargetRow>, PerfLlmError> {
        let mut out = Vec::new();
        build_targets(
            &endpoints.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &models.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &[],
            "between_tools",
            &voice(over),
            &env(),
            &mut out,
        )
    }

    #[test]
    fn proxy_is_cliproxy_over_anthropics_api_whatever_the_provider() {
        let rows = targets(
            &["proxy"],
            &[],
            &[("ANTHROPIC_BASE_URL", "http://proxy.lan:8317")],
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            CLAUDE_MODELS
        );
        for r in &rows {
            let t = &r.target;
            assert_eq!(
                (t.provider.as_str(), t.protocol, r.kind),
                ("cliproxy", Protocol::Anthropic, Protocol::Anthropic)
            );
            assert_eq!(t.base_url, "http://proxy.lan:8317");
            assert_eq!(t.api_key.as_str(), env()["ANTHROPIC_AUTH_TOKEN"]);
            assert_eq!(r.client.protocol(), Protocol::Anthropic);
            assert!(!r.sysbp);
        }
    }

    #[test]
    fn proxy_without_an_address_is_cliproxys_own() {
        let t = endpoint_connection("proxy+sysbp", &voice(&[])).unwrap();
        assert_eq!(t.provider, "cliproxy");
        assert_eq!(t.base_url, CLIPROXY_DEFAULT_URL);
        let rows = targets(&["proxy+sysbp"], &[], &[]).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.sysbp));
    }

    #[test]
    fn openrouter_is_its_anthropic_api_with_its_own_key() {
        let rows = targets(&["openrouter"], &["claude-sonnet-5-5"], &[]).unwrap();
        let [r] = rows.as_slice() else {
            panic!("one row, got {}", rows.len());
        };
        let t = &r.target;
        assert_eq!(
            (t.provider.as_str(), t.protocol),
            ("openrouter", Protocol::Anthropic)
        );
        assert_eq!(t.model, "anthropic/claude-sonnet-5.5");
        assert_eq!(r.label, "claude-sonnet-5-5");
        assert_eq!(t.api_key.as_str(), env()["OPENROUTER_API_KEY"]);
        assert_eq!(r.client.protocol(), Protocol::Anthropic);
    }

    #[test]
    fn a_provider_id_speaking_openai_gets_the_openai_client_and_its_default_model() {
        let rows = targets(&["openai"], &[], &[]).unwrap();
        let [r] = rows.as_slice() else {
            panic!("one row, got {}", rows.len());
        };
        let t = &r.target;
        assert_eq!(
            (t.protocol, r.kind, t.model.as_str()),
            (Protocol::Openai, Protocol::Openai, "gpt-5")
        );
        assert_eq!(r.client.protocol(), Protocol::Openai);
        assert_eq!(
            t.chat_params(2000)["max_completion_tokens"],
            json!(2000 + REASONING_HEADROOM)
        );
        assert!(r.thinking.is_none());
        assert!(!r.sysbp);
    }

    #[test]
    fn current_is_the_configured_connection_on_its_own_protocol() {
        let rows = targets(&["current"], &[], &[]).unwrap();
        let [r] = rows.as_slice() else {
            panic!("one row, got {}", rows.len());
        };
        let t = &r.target;
        assert_eq!(
            (t.provider.as_str(), t.protocol, t.model.as_str()),
            ("openai", Protocol::Openai, "gpt-5.5")
        );
        assert_eq!(r.client.protocol(), Protocol::Openai);
        assert_eq!(t.chat_params(2000)["reasoning_effort"], json!("none"));
        assert_eq!(t.api_key.as_str(), env()["OPENAI_API_KEY"]);
    }

    #[test]
    fn an_endpoint_without_its_key_is_skipped() {
        let mut env = env();
        env.remove("ANTHROPIC_API_KEY");
        let mut out = Vec::new();
        let rows = build_targets(
            &["anthropic".to_string(), "current".to_string()],
            &[],
            &[],
            "between_tools",
            &voice(&[]),
            &env,
            &mut out,
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.ep.as_str()).collect::<Vec<_>>(),
            ["current"]
        );
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("skipping anthropic"), "{out}");
        assert!(!out.contains("sk-"), "{out}");
    }

    #[test]
    fn an_unknown_endpoint_says_what_is_known() {
        let err = targets(&["nope"], &[], &[]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.starts_with("unknown endpoint 'nope' ("), "{msg}");
        assert!(
            msg.contains("current, proxy, openrouter, anthropic"),
            "{msg}"
        );
        assert!(msg.ends_with("; a provider takes +sysbp)"), "{msg}");
    }

    #[test]
    fn openrouter_models_go_over_its_openai_api() {
        let mut out = Vec::new();
        let rows = build_targets(
            &[],
            &[],
            &["openai/gpt-5.5".to_string(), "qwen/qwen3-235b".to_string()],
            "between_tools",
            &voice(&[]),
            &env(),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.ep.as_str()).collect::<Vec<_>>(),
            ["openrouter-openai", "openrouter-openai"]
        );
        let [a, b] = rows.as_slice() else {
            panic!("two rows, got {}", rows.len());
        };
        let (a, b) = (&a.target, &b.target);
        assert_eq!(a.protocol, Protocol::Openai);
        assert_eq!(a.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(a.api_key.as_str(), env()["OPENROUTER_API_KEY"]);
        assert_eq!(a.chat_params(2000)["reasoning_effort"], json!("none"));
        assert_eq!(
            Value::Object(b.chat_params(2000)),
            json!({"max_tokens": 2000})
        );
        assert!(out.is_empty());
    }

    #[test]
    fn haiku_gets_thinking_off_unless_a_mode_was_asked_for() {
        assert_eq!(
            thinking_for("claude-haiku-5-5", None).as_deref(),
            Some("disabled")
        );
        assert_eq!(
            thinking_for("claude-haiku-5-5", Some("adaptive")).as_deref(),
            Some("adaptive")
        );
        assert_eq!(thinking_for("claude-sonnet-5-5", None), None);
        let rows = targets(&["proxy"], &[], &[]).unwrap();
        let by_label = |l: &str| rows.iter().find(|r| r.label == l).unwrap();
        assert_eq!(
            by_label("claude-sonnet-5-5").thinking.as_deref(),
            Some("between_tools")
        );
        assert_eq!(
            by_label("claude-haiku-5-5").thinking.as_deref(),
            Some("disabled")
        );
    }

    #[test]
    fn p50_and_p95_as_the_python_computed_them() {
        let xs = [Some(3.0), None, Some(1.0), Some(2.0), Some(4.0)];
        assert_eq!(p50(&xs), Some(2.5));
        assert_eq!(p95(&xs), Some(4.0)); // round(0.95 * 3) = 3
        assert_eq!(p50(&[None, None]), None);
        assert_eq!(p95(&[]), None);
        assert_eq!(p95(&[Some(7.0)]), Some(7.0));
        // 31 values: round(0.95 * 30) = round(28.5) = 28 in Python, half to even.
        let many: Vec<Option<f64>> = (0..31).map(|i| Some(i as f64)).collect();
        assert_eq!(p95(&many), Some(28.0));
    }

    #[test]
    fn a_row_keeps_the_pythons_keys_and_the_table_its_columns() {
        let usage = |out: u64, read: u64| Usage {
            input_tokens: 3000,
            output_tokens: out,
            cache_read_tokens: Some(read),
            cache_write_tokens: Some(0),
            reasoning_tokens: None,
        };
        let sample = |ttfb: f64, tool: Option<&str>, u: Usage| Sample {
            ttfb: Some(ttfb),
            total: Some(ttfb + 1.0),
            span: Some(0.5),
            tool_done: tool.map(|_| ttfb + 0.2),
            usage: Some(u),
            tool: tool.map(String::from),
            words: Some(20),
            error: None,
        };
        let plain = [
            sample(1.0, None, usage(10, 0)),
            sample(0.8, None, usage(12, 2900)),
        ];
        let tool1 = [
            sample(1.2, Some("github_list_issues"), usage(40, 0)),
            sample(0.9, Some("hang_up"), usage(40, 2900)),
        ];
        let tool2 = [sample(1.1, None, usage(60, 3000))];
        let r = row("proxy", "claude-sonnet-5-5", &plain, &tool1, &tool2);
        let v = serde_json::to_value(&r).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(keys.contains(&"hello_ttfb") && keys.contains(&"in_first"));
        assert_eq!(r.n, 2);
        assert_eq!(r.hello_ttfb, Some(0.9));
        assert_eq!(r.hello_out, Some(11));
        assert_eq!(r.tool_ok, "1/2");
        assert_eq!(r.cache, "2900/0");
        assert_eq!(r.cold_ttfb, Some(1.0));
        assert!(
            (r.warm_ttfb.unwrap() - 0.85).abs() < 1e-9,
            "{:?}",
            r.warm_ttfb
        );
        assert_eq!(r.words, Some(20.0));
        assert_eq!(
            v["in_first"],
            json!({"in": 3000, "cache_read": 0, "cache_write": 0, "out": 10})
        );
        let text = json_indent_1(std::slice::from_ref(&r)).unwrap();
        assert!(
            text.starts_with("[\n {\n  \"endpoint\": \"proxy\",\n  \"model\""),
            "{text}"
        );
        let md = markdown(std::slice::from_ref(&r));
        let header = md.lines().next().unwrap();
        assert_eq!(header.matches('|').count(), 17);
        let body = md.lines().nth(2).unwrap();
        assert_eq!(body.matches('|').count(), 17);
        assert!(body.contains("| 1/2 | 20.0 | 2900/0 |  |"), "{body}");
        let l = line(&r);
        assert!(
            l.starts_with("  proxy              claude-sonnet-5-5"),
            "{l}"
        );
        assert!(!l.contains("ERR"));
    }
}
