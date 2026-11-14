//! The provider check (ADR 0056) over the operator's Claude Code (ADR 0069): the same fixed
//! synthetic prompts as every route's, asked through one persistent process as a session
//! asks it, so what it times is what a turn pays.
//!
//! What it finds: whether the program answers (or why not, in plain words), its first word
//! over a few short turns of the same process (the first after the process is ready, the
//! rest warm), whether the reply streams, whether a tool call makes the round trip through
//! the relay (the call parked, a result given back, the answer after it), and whether the
//! program's requests read their prefix from the cache. Thinking is the program's own: Ghost
//! asks for low effort and says so.

use std::time::Instant;

use futures::StreamExt;
use serde_json::json;

use super::ClaudeCode;
use crate::check::{
    LATENCY_PROMPT, ModelCheck, State, TOOL_NAME, TOOL_PROMPT, Timing, Verdict, plain,
};
use crate::context::{Context, Message, ToolCall};
use crate::model::{Event, LlmError};
use crate::tools::{ToolSchema, Tools};

/// The check's own system prompt: fixed, nothing of the operator's.
pub const SYSTEM: &str = "You are the subject of Ghost's provider check. Answer briefly. When \
                          asked the time, call the clock_read tool.";

/// One turn as the check reads it.
#[derive(Default)]
struct Timed {
    first_byte: Option<f64>,
    first_word: Option<f64>,
    pieces: usize,
    text: String,
    calls: Vec<ToolCall>,
    cache_read: Option<u64>,
    error: Option<LlmError>,
}

async fn timed(cc: &ClaudeCode, ctx: &Context, tools: &Tools) -> Timed {
    let began = Instant::now();
    let mut t = Timed::default();
    let mut s = cc.stream(ctx.clone(), tools);
    while let Some(ev) = s.next().await {
        t.first_byte.get_or_insert(began.elapsed().as_secs_f64());
        match ev {
            Ok(Event::Text(x)) if !x.trim().is_empty() => {
                t.first_word.get_or_insert(began.elapsed().as_secs_f64());
                t.pieces += 1;
                t.text.push_str(&x);
            }
            Ok(Event::ToolCall(c)) => t.calls.push(c),
            Ok(Event::Usage(u)) => {
                if let Some(n) = u.cache_read_tokens {
                    t.cache_read = Some(t.cache_read.unwrap_or(0).max(n));
                }
            }
            Err(e) => {
                t.error = Some(e);
                break;
            }
            _ => {}
        }
    }
    t
}

/// Check the program as `cc` runs it. Never fails: what went wrong is in the result.
pub async fn check(cc: &ClaudeCode, role: &str, samples: usize) -> ModelCheck {
    let mut out = ModelCheck::failed(role, cc.model(), "");
    out.error = None;
    let tool = Tools::new(vec![ToolSchema::new(
        TOOL_NAME,
        "Reads the current time. Takes no arguments.",
        json!({}),
        Vec::new(),
    )]);
    // 1. The process, started and its relay connected, as a session's opening does.
    let started = Instant::now();
    if let Err(e) = cc.warm(SYSTEM, &tool).await {
        out.error = Some(plain(&e));
        return out;
    }
    let ready = started.elapsed().as_secs_f64();
    // 2. Timed: the same process, turn after turn, as a conversation goes.
    let mut ctx = Context::new(SYSTEM);
    let mut bytes = Vec::new();
    let mut words = Vec::new();
    let mut pieces = Vec::new();
    let mut cache = None;
    for _ in 0..samples.max(1) {
        ctx.push(Message::user(LATENCY_PROMPT));
        let t = timed(cc, &ctx, &tool).await;
        if let Some(e) = &t.error {
            out.error.get_or_insert_with(|| plain(e));
            if !out.reachable {
                return out;
            }
            ctx.push_reply(String::new(), Vec::new());
            continue;
        }
        out.reachable = true;
        ctx.push_reply(t.text.clone(), Vec::new());
        bytes.extend(t.first_byte);
        words.extend(t.first_word);
        if t.first_word.is_some() {
            pieces.push(t.pieces);
        }
        cache = cache.max(t.cache_read);
    }
    out.answered_by = Some(format!("Claude Code ({})", cc.model()));
    out.samples = words.len();
    out.first_byte = Timing::of(&bytes);
    out.first_word = Timing::of(&words);
    out.streaming = if pieces.is_empty() {
        Verdict::of(State::Untested, "no reply had words in it")
    } else if pieces.iter().any(|&n| n >= 2) {
        Verdict::of(State::Yes, format!("the program was ready in {ready:.1} s"))
    } else {
        Verdict::of(
            State::No,
            "every reply arrived all at once, so Ghost can't start speaking until the whole reply is in",
        )
    };
    // 3. A call through the relay and back: parked, given a result, answered after it.
    ctx.push(Message::user(TOOL_PROMPT));
    let began = Instant::now();
    let t = timed(cc, &ctx, &tool).await;
    out.tools = match (&t.error, t.calls.iter().find(|c| c.name == TOOL_NAME)) {
        (Some(e), _) => Verdict::of(State::No, plain(e)),
        (None, Some(call)) => {
            ctx.push_reply(t.text.clone(), vec![call.clone()]);
            ctx.push_result(&call.id, &json!({"time": "12:00"}));
            let after = timed(cc, &ctx, &tool).await;
            match (&after.error, after.first_word) {
                (Some(e), _) => Verdict::of(State::No, plain(e)),
                (None, Some(_)) => Verdict::of(
                    State::Yes,
                    format!(
                        "a call through Ghost's relay and the answer after it took {:.1} s",
                        began.elapsed().as_secs_f64()
                    ),
                ),
                (None, None) => Verdict::of(
                    State::No,
                    "it called the tool but said nothing after the result",
                ),
            }
        }
        (None, None) => Verdict::of(
            State::No,
            "it answered in words instead of calling the tool; apps and Ghost's own tools won't work well",
        ),
    };
    // 4. The program places its own cache markers; the warm turns say whether they read.
    out.caching = match cache {
        Some(n) if n > 0 => Verdict::of(State::Yes, format!("{n} tokens read from the cache")),
        Some(_) => Verdict::of(
            State::No,
            "the program's requests didn't read from the cache",
        ),
        None => Verdict::of(State::NotReported, "the program didn't report it"),
    };
    out.thinking = Verdict::of(
        State::NotApplicable,
        "Claude Code decides its own thinking; Ghost asks it for low effort",
    );
    out
}
