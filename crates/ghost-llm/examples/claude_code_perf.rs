//! What running the operator's Claude Code costs a spoken turn (ADR 0069, ADR 0018), against
//! the direct connection, with the installed `claude` and small synthetic prompts only.
//!
//! MEASUREMENT ONLY. Where the program's own sign-in can't be used (the workstation's had
//! expired on 2026-11-14), the harness gives it an address and token of the harness's own,
//! from `GHOST_PERF_CC_BASE_URL` and `GHOST_PERF_CC_TOKEN` (`Settings::harness_env`). The
//! product never does: what this times is what the program adds, not the subscription
//! path. The direct connection is timed through the same address.
//!
//!   GHOST_PERF_CC_BASE_URL=… GHOST_PERF_CC_TOKEN=… \
//!     cargo run -p ghost-llm --example claude_code_perf -- [rounds]

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use futures::StreamExt;
use ghost_llm::claude_code::{ClaudeCode, Settings};
use ghost_llm::{Context, Event, Message, Model, Options, ToolSchema, Tools};
use serde_json::json;

const SYSTEM: &str = "You are a terse voice assistant. Answer in one short spoken sentence, no \
                      markdown. When asked the time, call the clock_read tool.";
const MODEL_ALIAS: &str = "haiku";
const MODEL_ID: &str = "claude-haiku-5-5";

fn tools() -> Tools {
    Tools::new(vec![ToolSchema::new(
        "clock_read",
        "Reads the current time. Takes no arguments.",
        json!({}),
        Vec::new(),
    )])
}

fn secs(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0).round() / 1000.0
}

/// The first word, the end, and the call if one came.
async fn turn(cc: &ClaudeCode, ctx: &Context) -> (Option<f64>, f64, Option<ghost_llm::ToolCall>, String) {
    let began = Instant::now();
    let mut first = None;
    let mut call = None;
    let mut text = String::new();
    let mut s = cc.stream(ctx.clone(), &tools());
    while let Some(ev) = s.next().await {
        match ev {
            Ok(Event::Text(t)) => {
                if !t.trim().is_empty() {
                    first.get_or_insert(secs(began.elapsed()));
                }
                text.push_str(&t);
            }
            Ok(Event::ToolCall(c)) => {
                first.get_or_insert(secs(began.elapsed()));
                call = Some(c);
            }
            Err(e) => {
                eprintln!("error: {e}");
                break;
            }
            _ => {}
        }
    }
    (first, secs(began.elapsed()), call, text)
}

#[tokio::main]
async fn main() {
    let rounds: usize = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(3);
    let base = std::env::var("GHOST_PERF_CC_BASE_URL").expect("GHOST_PERF_CC_BASE_URL");
    let token = std::env::var("GHOST_PERF_CC_TOKEN").expect("GHOST_PERF_CC_TOKEN");
    let started: HashMap<String, String> = ["HOME", "PATH", "USER", "LANG"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    let binary = ghost_core::other_tools::program::which(started.get("PATH").map(String::as_str))
        .expect("claude on the PATH");
    for round in 0..rounds {
        let mut settings = Settings::new(binary.clone(), MODEL_ALIAS, started.clone());
        settings.harness_env = BTreeMap::from([
            ("ANTHROPIC_BASE_URL".to_string(), base.clone()),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), token.clone()),
        ]);
        let cc = ClaudeCode::new(settings);
        let began = Instant::now();
        cc.warm(SYSTEM, &tools()).await.expect("ready");
        let ready = secs(began.elapsed());
        let mut ctx = Context::new(SYSTEM);
        let mut words = Vec::new();
        for (i, q) in ["Name a colour.", "Name a fruit.", "Name an animal."].iter().enumerate() {
            ctx.push(Message::user(*q));
            let (first, _, _, text) = turn(&cc, &ctx).await;
            ctx.push_reply(text, Vec::new());
            words.push((if i == 0 { "first_turn" } else { "warm" }, first));
        }
        // One tool through the relay: the call at the stage, then the answer after its result.
        ctx.push(Message::user("What time is it?"));
        let t0 = Instant::now();
        let (to_call, _, call, text) = turn(&cc, &ctx).await;
        let call = call.expect("a call");
        ctx.push_reply(text, vec![call.clone()]);
        ctx.push_result(&call.id, &json!({"time": "12:00"}));
        let (after, _, _, text) = turn(&cc, &ctx).await;
        let whole_tool_turn = secs(t0.elapsed());
        ctx.push_reply(text, Vec::new());
        // Barge-in: a long reply dropped at its first word, then the next turn's lock, which
        // waits for the program to end the interrupted reply.
        ctx.push(Message::user("Count from one to one hundred in words."));
        let mut s = cc.stream(ctx.clone(), &tools());
        while let Some(ev) = s.next().await {
            if matches!(ev, Ok(Event::Text(ref t)) if !t.trim().is_empty()) {
                break;
            }
        }
        let cut = Instant::now();
        drop(s);
        cc.warm(SYSTEM, &tools()).await.expect("still running");
        let interrupted = secs(cut.elapsed());
        println!(
            "{}",
            json!({"round": round, "via": "claude-code", "ready": ready, "first_word": words,
                   "tool_call_at_stage": to_call, "answer_after_result": after,
                   "one_tool_turn": whole_tool_turn, "barge_in_to_program_idle": interrupted})
        );
    }
    // The direct connection, through the same address.
    let mut env = HashMap::new();
    env.insert("GHOST_LLM_PROVIDER".to_string(), "custom".to_string());
    env.insert("GHOST_LLM_BASE_URL".to_string(), base.clone());
    env.insert("GHOST_LLM_PASSWORD".to_string(), "PERF_TOKEN".to_string());
    env.insert("PERF_TOKEN".to_string(), token.clone());
    env.insert("GHOST_LLM_MODEL".to_string(), MODEL_ID.to_string());
    let target = ghost_core::connection::ModelConnection::from_env(&env).target(&env);
    let model = Model::new(target).unwrap();
    let options = Options {
        thinking: None,
        ..Options::default()
    };
    let direct = std::env::args().nth(2).as_deref() != Some("program-only");
    for round in 0..if direct { rounds.min(2) } else { 0 } {
        let mut ctx = Context::new(SYSTEM);
        let mut words = Vec::new();
        for q in ["Name a colour.", "Name a fruit."] {
            ctx.push(Message::user(q));
            let began = Instant::now();
            let mut first = None;
            let mut text = String::new();
            let mut s = Box::pin(model.stream(ctx.clone(), &tools(), &options));
            while let Some(ev) = s.next().await {
                if let Ok(Event::Text(t)) = ev {
                    if !t.trim().is_empty() {
                        first.get_or_insert(secs(began.elapsed()));
                    }
                    text.push_str(&t);
                }
            }
            ctx.push_reply(text, Vec::new());
            words.push(first);
        }
        ctx.push(Message::user("What time is it?"));
        let began = Instant::now();
        let mut to_call = None;
        let mut s = Box::pin(model.stream(ctx.clone(), &tools(), &options));
        while let Some(ev) = s.next().await {
            if let Ok(Event::ToolCall(_)) = ev {
                to_call.get_or_insert(secs(began.elapsed()));
            }
        }
        println!(
            "{}",
            json!({"round": round, "via": "direct", "first_word": words, "tool_call_at_stage": to_call})
        );
    }
}
