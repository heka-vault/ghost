//! The model's stage: the context, the streamed reply, every tool call of it, the
//! speculative answer, and the compaction (Pipecat's LLM service and context aggregators,
//! over `ghost_llm`).
//!
//! The stage owns the session's [`Context`]: the user aggregator hands it the turn, it
//! streams the model's reply as frames ([`Frame::LlmStart`], [`Frame::LlmText`],
//! [`Frame::FunctionCallsStarted`], [`Frame::LlmEnd`]), runs the calls through the
//! [`ToolDispatcher`] (every one of a reply, together), appends the results and runs the
//! model again until a reply has no call, or a call said not to (`hang_up`, `hold`, a
//! transfer, whose frames then go in at the pipeline's source).
//!
//! A speculative answer ([`Frame::Speculate`]) starts the stream at Smart Turn's verdict
//! with the turn so far; its frames wait in the stage until the grace confirms the turn
//! ([`Frame::SpeculationConfirmed`]) and are thrown away if the operator goes on. One that
//! reaches a tool call withdraws itself ([`Frame::EagerCancel`] back up), so nothing is
//! written for a turn that hadn't ended.
//!
//! Every lesson the Python daemon learned about the model is in `ghost_llm` (every tool
//! call of a reply, the cut-off redo); the turn's token ends a stream the operator talked
//! over, and what was said so far is what the context keeps.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use ghost_core::conversations::SUMMARY_TEMPLATE;
use ghost_core::source::BoxFuture;
use ghost_llm::{CUT_OFF_NOTE, Context, Event, LlmError, Message, Model, Options, ToolCall, Tools};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::conversation::ConversationRuntime;
use crate::frames::{Direction, Frame, Metric, Out, Stage};
use crate::tools::{CallResult, ToolDispatcher};
use crate::turns::TurnControl;

pub const COMPACTION_PROMPT: &str =
    "You are condensing the older part of a spoken conversation between an \
operator and their assistant into the assistant's memory of it.

Keep, as plain factual statements: decisions and agreements; facts, names and numbers the \
operator stated; what was asked for and whether it was done; open questions and action \
items; preferences about how the operator likes things done. Fold in any earlier memory \
that is still true. Drop greetings, small talk, and tool chatter. Do not invent anything. \
Write only the memory, no preamble.";

/// The model as the stage reaches it: `ghost_llm::Model`, or a fake in tests and the
/// replay harness.
pub trait ModelPort: Send + Sync {
    fn stream(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>>;
    fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> BoxFuture<'_, Result<String, LlmError>>;
    fn model_name(&self) -> String;
}

impl ModelPort for Model {
    fn stream(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        Box::pin(Model::stream(self, context, tools, options))
    }

    fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> BoxFuture<'_, Result<String, LlmError>> {
        Box::pin(async move { Model::complete(self, &system, &user, max_tokens).await })
    }

    fn model_name(&self) -> String {
        self.target().model.clone()
    }
}

pub type SharedContext = Arc<Mutex<Context>>;
pub type Inject = Arc<dyn Fn(Vec<Frame>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Compaction (ADR 0014): the in-session summarizer, tokens-only trigger.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Compaction {
    pub trigger_tokens: u64,
    pub keep_messages: usize,
    pub summary_tokens: u32,
}

struct Speculation {
    text: String,
    events: mpsc::UnboundedReceiver<Event>,
    task: JoinHandle<()>,
    /// The answer reached a tool call: withdrawn; the confirmed turn runs its own.
    withdrawn: Arc<std::sync::atomic::AtomicBool>,
}

pub struct LlmStage {
    model: Arc<dyn ModelPort>,
    context: SharedContext,
    tools: Tools,
    options: Options,
    dispatcher: Arc<ToolDispatcher>,
    turns: Arc<TurnControl>,
    inject: Inject,
    runtime: Option<Arc<ConversationRuntime>>,
    compaction: Option<Compaction>,
    speculation: Option<Speculation>,
    last_input_tokens: u64,
}

impl LlmStage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: Arc<dyn ModelPort>,
        context: SharedContext,
        tools: Tools,
        options: Options,
        dispatcher: Arc<ToolDispatcher>,
        turns: Arc<TurnControl>,
        inject: Inject,
        runtime: Option<Arc<ConversationRuntime>>,
    ) -> Self {
        Self {
            model,
            context,
            tools,
            options,
            dispatcher,
            turns,
            inject,
            runtime,
            compaction: None,
            speculation: None,
            last_input_tokens: 0,
        }
    }

    pub fn with_compaction(mut self, c: Option<Compaction>) -> Self {
        self.compaction = c.filter(|c| c.trigger_tokens > 0);
        self
    }

    pub fn context(&self) -> SharedContext {
        self.context.clone()
    }

    fn snapshot(&self) -> Context {
        self.context.lock().unwrap().clone()
    }

    // --- the reply ----------------------------------------------------------------------

    /// One model run on the context, its calls, and the runs the results need. The turn's
    /// token is taken once: a run interrupted between two calls stops, rather than going
    /// on under the next turn's token.
    async fn run(&mut self, out: &Out) {
        let turn = self.turns.current();
        loop {
            if turn.is_cancelled() {
                return;
            }
            let context = self.snapshot();
            let stream = self.model.stream(context, &self.tools, &self.options);
            let Some(reply) = self.consume(stream, &turn, out, None).await else {
                return; // interrupted
            };
            if !self.after_reply(reply, out).await {
                return;
            }
        }
    }

    /// Stream one reply as frames. `prefix` are events already received (a speculation's);
    /// None when the turn was interrupted.
    async fn consume(
        &mut self,
        mut stream: BoxStream<'static, Result<Event, LlmError>>,
        turn: &tokio_util::sync::CancellationToken,
        out: &Out,
        mut prefix: Option<mpsc::UnboundedReceiver<Event>>,
    ) -> Option<Reply> {
        out.push(Frame::LlmStart).await;
        out.push(Frame::ReplyTurn(turn.clone())).await;
        let mut reply = Reply::default();
        let began = Instant::now();
        let mut first = true;
        loop {
            let next = if let Some(rx) = &mut prefix {
                match rx.recv().await {
                    Some(ev) => Some(Ok(ev)),
                    None => {
                        prefix = None;
                        continue;
                    }
                }
            } else {
                tokio::select! {
                    biased;
                    _ = turn.cancelled() => {
                        self.context.lock().unwrap().push_reply(reply.text.clone(), Vec::new());
                        out.push(Frame::LlmEnd).await;
                        return None;
                    }
                    ev = stream.next() => ev,
                }
            };
            let Some(ev) = next else { break };
            match ev {
                Ok(Event::Text(t)) => {
                    if first {
                        first = false;
                        out.push(Frame::Metrics(Metric::Ttfb {
                            value: began.elapsed().as_secs_f64(),
                            model: Some(self.model.model_name()),
                        }))
                        .await;
                    }
                    reply.text.push_str(&t);
                    out.push(Frame::LlmText(t)).await;
                }
                Ok(Event::Thought(_)) => out.push(Frame::LlmThought).await,
                Ok(Event::ToolCall(c)) => {
                    if first {
                        first = false;
                        out.push(Frame::Metrics(Metric::Ttfb {
                            value: began.elapsed().as_secs_f64(),
                            model: Some(self.model.model_name()),
                        }))
                        .await;
                    }
                    reply.calls.push(c);
                }
                Ok(Event::Usage(u)) => {
                    self.last_input_tokens = u.input_tokens
                        + u.cache_read_tokens.unwrap_or(0)
                        + u.cache_write_tokens.unwrap_or(0);
                    out.push(Frame::Metrics(Metric::Usage {
                        input: u.input_tokens,
                        output: u.output_tokens,
                        cache_read: u.cache_read_tokens.unwrap_or(0),
                        cache_write: u.cache_write_tokens.unwrap_or(0),
                        model: Some(self.model.model_name()),
                    }))
                    .await;
                }
                Ok(Event::Finished(_)) => {}
                Ok(Event::CutOff { .. }) => out.push(Frame::ReplyCut).await,
                Ok(Event::Redo) => {
                    // The model's stream appended the cut reply and the note; mirror it.
                    let mut ctx = self.context.lock().unwrap();
                    ctx.push_reply(reply.text.clone(), Vec::new());
                    ctx.push(Message::user(CUT_OFF_NOTE));
                    reply.text.clear();
                }
                Ok(Event::Say(line)) => {
                    reply.text.push_str(&line);
                    out.push(Frame::LlmText(line)).await;
                }
                Err(e) => {
                    warn!("model: {e}");
                    out.push(Frame::Error(e.to_string())).await;
                    break;
                }
            }
        }
        Some(reply)
    }

    /// The calls of a whole reply, in the Python's order: the calls announced, each one
    /// in progress, the response's end, then the results. True when the model runs again.
    async fn after_reply(&mut self, reply: Reply, out: &Out) -> bool {
        let names: Vec<String> = reply.calls.iter().map(|c| c.name.clone()).collect();
        if !names.is_empty() {
            out.push(Frame::FunctionCallsStarted(names)).await;
            for c in &reply.calls {
                out.push(Frame::FunctionCallInProgress {
                    name: c.name.clone(),
                    id: c.id.clone(),
                })
                .await;
            }
        }
        self.context
            .lock()
            .unwrap()
            .push_reply(reply.text.clone(), reply.calls.clone());
        out.push(Frame::LlmEnd).await;
        if reply.calls.is_empty() {
            self.maybe_compact().await;
            return false;
        }
        // Every call of the reply, together.
        let dispatcher = self.dispatcher.clone();
        let results: Vec<(ToolCall, CallResult)> =
            futures::future::join_all(reply.calls.iter().map(|c| {
                let dispatcher = dispatcher.clone();
                async move {
                    let r = dispatcher.call(&c.name, c.arguments.clone(), &c.id).await;
                    (c.clone(), r)
                }
            }))
            .await;
        let mut run_again = true;
        let mut after: Vec<Frame> = Vec::new();
        for (call, result) in results {
            {
                let mut ctx = self.context.lock().unwrap();
                ctx.push_result(&call.id, &Value::Object(result.result.clone()));
            }
            out.push(Frame::FunctionCallResult {
                name: call.name.clone(),
                id: call.id.clone(),
            })
            .await;
            if !result.run_llm {
                run_again = false;
            }
            after.extend(result.after);
        }
        if !after.is_empty() {
            (self.inject)(after).await;
        }
        if !run_again {
            // hold and hang_up: nothing follows the result, so the thinking sound stops
            out.push(Frame::ThinkingStop).await;
        }
        run_again
    }

    // --- speculation --------------------------------------------------------------------

    async fn speculate(&mut self, text: String, out: &Out) {
        self.drop_speculation();
        let mut context = self.snapshot();
        context.push(Message::user(text.clone()));
        let stream = self.model.stream(context, &self.tools, &self.options);
        let (tx, rx) = mpsc::unbounded_channel();
        let withdrawn = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = withdrawn.clone();
        let out = out.clone();
        let model_name = self.model.model_name();
        let task = tokio::spawn(async move {
            let mut stream = stream;
            let began = Instant::now();
            let mut first = true;
            while let Some(ev) = stream.next().await {
                if first && matches!(ev, Ok(Event::Text(_)) | Ok(Event::ToolCall(_))) {
                    first = false;
                    // The held answer's first byte, before its call is seen.
                    out.push(Frame::Metrics(Metric::Ttfb {
                        value: began.elapsed().as_secs_f64(),
                        model: Some(model_name.clone()),
                    }))
                    .await;
                }
                if matches!(ev, Ok(Event::ToolCall(_))) {
                    // The answer reached a tool call: no tool runs for a turn that hadn't
                    // ended. The confirmed turn makes the call.
                    w.store(true, std::sync::atomic::Ordering::SeqCst);
                    out.push_up(Frame::EagerCancel).await;
                    return;
                }
                if let Ok(ev) = ev
                    && tx.send(ev).is_err()
                {
                    return;
                }
            }
        });
        self.speculation = Some(Speculation {
            text,
            events: rx,
            task,
            withdrawn,
        });
    }

    fn drop_speculation(&mut self) {
        if let Some(s) = self.speculation.take() {
            s.task.abort();
        }
    }

    /// The grace ended with the same words: the held answer is the answer, spoken now.
    async fn confirm_speculation(&mut self, out: &Out) {
        let Some(mut s) = self.speculation.take() else {
            // Nothing held (withdrawn, or none): the turn's own run.
            return;
        };
        self.context
            .lock()
            .unwrap()
            .push(Message::user(s.text.clone()));
        if s.withdrawn.load(std::sync::atomic::Ordering::SeqCst) {
            s.task.abort();
            self.run(out).await;
            return;
        }
        let turn = self.turns.current();
        // Replay what was held, then follow the rest live; an empty stream of a finished
        // task ends it.
        let (tx, rx) = mpsc::unbounded_channel::<Event>();
        let mut held = Vec::new();
        while let Ok(ev) = s.events.try_recv() {
            held.push(ev);
        }
        for ev in held {
            let _ = tx.send(ev);
        }
        let rest = s.events;
        let live: BoxStream<'static, Result<Event, LlmError>> =
            Box::pin(futures::stream::unfold(rest, |mut rest| async move {
                rest.recv().await.map(|ev| (Ok(ev), rest))
            }));
        drop(tx);
        let Some(reply) = self.consume(live, &turn, out, Some(rx)).await else {
            return;
        };
        self.after_reply(reply, out).await;
        // The run loop continues on the results, if any called for one.
        if !self.context.lock().unwrap().messages.is_empty()
            && matches!(
                self.context.lock().unwrap().messages.last(),
                Some(Message::Tool { .. })
            )
        {
            self.run(out).await;
        }
    }

    // --- compaction ---------------------------------------------------------------------

    async fn maybe_compact(&mut self) {
        let Some(c) = self.compaction else { return };
        if self.last_input_tokens < c.trigger_tokens {
            return;
        }
        let (older, kept, system) = {
            let ctx = self.context.lock().unwrap();
            let n = ctx.messages.len();
            if n <= c.keep_messages + 1 {
                return;
            }
            let split = n - c.keep_messages;
            (
                ctx.messages[..split].to_vec(),
                ctx.messages[split..].to_vec(),
                ctx.system.clone(),
            )
        };
        let _ = system;
        let transcript: Vec<String> = older
            .iter()
            .map(|m| match m {
                Message::User(t) => format!("Operator: {t}"),
                Message::Assistant { text, tool_calls } => {
                    if text.is_empty() {
                        format!(
                            "(you called {})",
                            tool_calls
                                .iter()
                                .map(|c| c.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else {
                        format!("You: {text}")
                    }
                }
                Message::Tool { .. } => String::new(),
                Message::System(t) => format!("Note: {t}"),
            })
            .filter(|l| !l.is_empty())
            .collect();
        let summary = match self
            .model
            .complete(
                COMPACTION_PROMPT.to_string(),
                transcript.join("\n"),
                c.summary_tokens,
            )
            .await
        {
            Ok(s) => s.trim().to_string(),
            Err(e) => {
                warn!("compaction failed: {e}");
                return;
            }
        };
        if summary.is_empty() {
            return;
        }
        let mut messages = vec![Message::user(
            SUMMARY_TEMPLATE.replace("{summary}", &summary),
        )];
        messages.extend(kept);
        {
            let mut ctx = self.context.lock().unwrap();
            ctx.messages = messages.clone();
        }
        self.last_input_tokens = 0;
        info!(
            "compacted the context: {} older messages into a summary",
            older.len()
        );
        if let Some(rt) = &self.runtime {
            rt.on_summary_applied(&messages).await;
        }
    }
}

#[derive(Default)]
struct Reply {
    text: String,
    calls: Vec<ToolCall>,
}

#[async_trait]
impl Stage for LlmStage {
    fn name(&self) -> &'static str {
        "llm"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Up {
            out.forward(frame, direction).await;
            return;
        }
        match frame {
            Frame::ContextAppend { messages, run_llm } => {
                self.drop_speculation();
                self.context.lock().unwrap().messages.extend(messages);
                if run_llm {
                    self.run(out).await;
                }
            }
            Frame::ContextUpdate { messages, run_llm } => {
                self.drop_speculation();
                self.context.lock().unwrap().messages = messages;
                if run_llm {
                    self.run(out).await;
                }
            }
            Frame::SystemPrompt(s) => self.context.lock().unwrap().system = s,
            Frame::SetTools(t) => self.tools = t,
            Frame::RunLlm => self.run(out).await,
            Frame::Speculate(text) => self.speculate(text, out).await,
            Frame::SpeculationConfirmed => self.confirm_speculation(out).await,
            Frame::SpeculationCancelled => self.drop_speculation(),
            Frame::Interruption => {
                self.drop_speculation();
                out.forward(Frame::Interruption, direction).await;
            }
            Frame::End | Frame::Cancel => {
                self.drop_speculation();
                out.forward(frame, direction).await;
            }
            other => out.forward(other, direction).await,
        }
    }

    async fn stopped(&mut self) {
        self.drop_speculation();
    }
}

/// A scripted model for tests and the replay harness: each line of the script has its
/// replies in order (a tool call, then the answer), streamed a word at a time after
/// `ttfb`. A line is the n-th user turn of the context (system notes aside). A reply
/// whose stream was dropped before its end (a speculation withdrawn or thrown away) is
/// served again, as a real model regenerates its answer on the confirmed turn.
pub struct ScriptedModel {
    lines: Vec<Vec<ScriptedReply>>,
    served: Arc<Mutex<Vec<usize>>>,
    ttfb: std::time::Duration,
    name: String,
    pub calls: Mutex<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptedReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

impl ScriptedModel {
    pub fn new(lines: Vec<Vec<ScriptedReply>>, ttfb: std::time::Duration) -> Arc<Self> {
        let n = lines.len();
        Arc::new(Self {
            lines,
            served: Arc::new(Mutex::new(vec![0; n])),
            ttfb,
            name: "scripted".into(),
            calls: Mutex::new(0),
        })
    }

    pub fn say(text: &str) -> ScriptedReply {
        ScriptedReply {
            text: text.into(),
            tool_calls: Vec::new(),
        }
    }

    pub fn call(text: &str, name: &str, args: serde_json::Map<String, Value>) -> ScriptedReply {
        ScriptedReply {
            text: text.into(),
            tool_calls: vec![ToolCall::new(format!("call_{name}"), name, args)],
        }
    }

    /// Which line of the script the context is on: its user turns, the notes aside. A
    /// turn past the script (a line the pause split in two) is the last line's.
    fn line_of(&self, context: &Context) -> Option<usize> {
        let turns = context
            .messages
            .iter()
            .filter(|m| matches!(m, Message::User(t) if !t.starts_with("[system]")))
            .count();
        turns
            .checked_sub(1)
            .map(|l| l.min(self.lines.len().saturating_sub(1)))
    }
}

impl ModelPort for ScriptedModel {
    fn stream(
        &self,
        context: Context,
        _tools: &Tools,
        _options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        *self.calls.lock().unwrap() += 1;
        let line = self.line_of(&context);
        let reply = line
            .and_then(|l| {
                let i = *self.served.lock().unwrap().get(l)?;
                self.lines.get(l)?.get(i).cloned()
            })
            .unwrap_or_default();
        let words: Vec<String> = reply.text.split_inclusive(' ').map(String::from).collect();
        let done = {
            let served = self.served.clone();
            move || {
                if let Some(l) = line
                    && let Some(n) = served.lock().unwrap().get_mut(l)
                {
                    *n += 1;
                }
            }
        };
        Box::pin(async_stream_words(words, reply.tool_calls, self.ttfb, done))
    }

    fn complete(
        &self,
        _system: String,
        _user: String,
        _max_tokens: u32,
    ) -> BoxFuture<'_, Result<String, LlmError>> {
        Box::pin(async { Ok("A summary.".to_string()) })
    }

    fn model_name(&self) -> String {
        self.name.clone()
    }
}

/// `done` runs when the last event has been taken: a stream dropped before that was a
/// speculation that never became the answer.
fn async_stream_words(
    words: Vec<String>,
    calls: Vec<ToolCall>,
    ttfb: std::time::Duration,
    done: impl Fn() + Send + 'static,
) -> impl futures::Stream<Item = Result<Event, LlmError>> + Send {
    let mut events: Vec<Event> = words.into_iter().map(Event::Text).collect();
    let n_calls = calls.len();
    events.extend(calls.into_iter().map(Event::ToolCall));
    events.push(Event::Usage(ghost_llm::Usage {
        input_tokens: 100,
        output_tokens: 20,
        cache_read_tokens: Some(0),
        cache_write_tokens: Some(0),
        reasoning_tokens: None,
    }));
    events.push(Event::Finished(if n_calls > 0 {
        ghost_llm::FinishReason::ToolUse
    } else {
        ghost_llm::FinishReason::End
    }));
    let total = events.len();
    futures::stream::unfold(
        (events.into_iter(), 0usize, Some(done)),
        move |(mut it, taken, done)| async move {
            if taken == 0 {
                tokio::time::sleep(ttfb).await;
            }
            let ev = it.next()?;
            let taken = taken + 1;
            let done = if taken == total {
                if let Some(d) = &done {
                    d();
                }
                None
            } else {
                done
            };
            Some((Ok(ev), (it, taken, done)))
        },
    )
}
