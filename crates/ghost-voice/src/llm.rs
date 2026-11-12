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
//! reaches a tool call before the turn ends withdraws itself ([`Frame::EagerCancel`] back
//! up), so nothing is written for a turn that hadn't ended; one that reaches it once the
//! turn is confirmed makes the call as the turn's own (`held`).
//!
//! Every lesson the Python daemon learned about the model is in `ghost_llm` (every tool
//! call of a reply, the cut-off redo); the turn's token ends a stream the operator talked
//! over, and what was said so far is what the context keeps.
//!
//! With a fast model set (ADR 0050) a turn's first call goes to it, with the same context
//! and the same tools and one more, `think_harder`. What it writes is held in the stage
//! until it ends: plain speech is the reply; a tool call, `think_harder` or any other,
//! hands the turn to the main model ([`Frame::HandOver`]), which makes the call itself,
//! and nothing the fast model wrote is spoken or kept. The main model answers every call
//! after the first, so what runs a tool is always the main model.
//!
//! The reply's first clause is handed to the voice as it is written
//! (`crate::tts::Prerender`, ADR 0049), so it is rendered by the time the reply is released.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use ghost_core::conversations::SUMMARY_TEMPLATE;
use ghost_core::source::BoxFuture;
/// Appended to a turn the operator said over Ghost's reply, for the model.
pub const BARGED_IN_NOTE: &str = "\n\n(Said over your reply, which stopped there. The first \
word or two of anything said over your voice is lost to the echo canceller: if the request \
is unclear, ask in a few words instead of guessing.)";

use ghost_llm::{
    CUT_OFF_NOTE, Context, Event, LlmError, Message, Model, Options, ToolCall, ToolSchema, Tools,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::conversation::ConversationRuntime;
use crate::frames::{Direction, Frame, Metric, Out, Stage};
use crate::tools::{CallResult, ToolDispatcher};
use crate::tts::Prerender;
use crate::turns::TurnControl;

/// The tool a fast model calls to hand the turn to the main one (ADR 0050). It is never
/// run: asking is the hand-over.
pub const THINK_HARDER_TOOL: &str = "think_harder";
pub const THINK_HARDER_ABOUT: &str = "Hand this turn to the main model, saying nothing first. \
Call it when answering takes a tool or an app, a lookup, more than a sentence or two of \
thought, or when you are not sure. Do not call it for small talk, a greeting, an \
acknowledgment, or a question the conversation already answers.";

/// What the fast model is offered: every tool the main model has, in the same order, and
/// `think_harder` after them, so its cached prompt is the main model's tools plus one.
pub fn fast_tools(tools: &Tools) -> Tools {
    let mut all: Vec<ToolSchema> = tools.iter().cloned().collect();
    all.push(ToolSchema::new(
        THINK_HARDER_TOOL,
        THINK_HARDER_ABOUT,
        json!({}),
        Vec::new(),
    ));
    Tools::new(all)
}

/// How the fast model is asked: as the main model is, without the thinking setting.
/// A small model answers small talk and needs none, and it may refuse the main model's:
/// Haiku 5.5 answers `between_tools`, Ghost's default, with HTTP 400 ("not supported for
/// this model", 2026-11-12), and every turn was handed over for it.
pub fn fast_options(options: &Options) -> Options {
    Options {
        thinking: None,
        ..options.clone()
    }
}

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

/// Where a held answer stands, shared by the stage and the task streaming it. The two
/// race: the grace can end while the stream is still on its way to a tool call. Each
/// side moves on from `STREAMING` with a compare-exchange, so exactly one wins: a tool
/// call reached while streaming withdraws the answer and the confirmed turn runs its
/// own; one reached once confirmed is the turn's own call and streams on. Live, 2026-10-30,
/// a 1.5 s first token against a 1.2 s grace met the old rule, which withdrew whatever
/// the state, and the confirmed turn's stream ended with no call, no text and no run:
/// Ghost sat silent after every request that needed an app.
mod held {
    pub const STREAMING: u8 = 0;
    pub const CONFIRMED: u8 = 1;
    pub const WITHDRAWN: u8 = 2;
}

/// What the stage follows of one model call.
#[derive(Debug)]
enum Held {
    Event(Event),
    /// The model that answers wrote its first token `secs` after the call began.
    First {
        model: String,
        secs: f64,
    },
    /// The fast model asked for the main one, `secs` after the call began: nothing it
    /// wrote was passed on, and what follows is the main model's.
    HandOver {
        names: Vec<String>,
        secs: f64,
    },
}

type Events = mpsc::UnboundedReceiver<Result<Held, LlmError>>;

/// The calls the fast model started when it handed a held answer over.
type Handed = Arc<Mutex<Option<Vec<String>>>>;

/// One model call for the stage: the fast model first when one is given, then the main.
struct Call {
    main: Arc<dyn ModelPort>,
    /// The fast model, what it is offered (`fast_tools`) and how it is asked
    /// (`fast_options`), for a turn's first call.
    fast: Option<(Arc<dyn ModelPort>, Tools, Options)>,
    context: Context,
    tools: Tools,
    options: Options,
    prerender: Option<Prerender>,
    /// A held answer: where it stands, and where its withdrawal is said.
    held: Option<(Arc<std::sync::atomic::AtomicU8>, Out)>,
    handed: Handed,
}

impl Call {
    /// Run the call on a task of its own; its events come on the channel, and dropping
    /// the receiver ends the request.
    fn start(self) -> (Events, JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (rx, tokio::spawn(self.drive(tx)))
    }

    async fn drive(self, tx: mpsc::UnboundedSender<Result<Held, LlmError>>) {
        use std::sync::atomic::Ordering::SeqCst;
        let pre = self.prerender.as_ref();
        if let Some((fast, fast_tools, fast_options)) = &self.fast {
            // The fast model's reply is kept here until it ends: speech is passed on
            // whole, and a call means none of it is.
            let began = Instant::now();
            if let Some(p) = pre {
                p.begin();
            }
            let mut stream = fast.stream(self.context.clone(), fast_tools, fast_options);
            let mut kept: Vec<Event> = Vec::new();
            let mut first: Option<f64> = None;
            let mut names: Vec<String> = Vec::new();
            let mut spoke = false;
            let mut failed = false;
            loop {
                let ev = tokio::select! {
                    biased;
                    () = tx.closed() => return,
                    ev = stream.next() => ev,
                };
                let Some(ev) = ev else { break };
                match ev {
                    Ok(Event::ToolCall(call)) => names.push(call.name),
                    // Cut off at the cap, or redone: more than the fast model is for.
                    Ok(Event::CutOff { .. } | Event::Redo | Event::Say(_)) => {
                        failed = true;
                        break;
                    }
                    Ok(ev) => {
                        if let Event::Text(t) = &ev {
                            first.get_or_insert_with(|| began.elapsed().as_secs_f64());
                            spoke |= !t.trim().is_empty();
                            if let Some(p) = pre {
                                p.text(t);
                            }
                        }
                        kept.push(ev);
                    }
                    Err(e) => {
                        warn!("fast model: {e}; the main model answers");
                        failed = true;
                        break;
                    }
                }
            }
            drop(stream);
            if names.is_empty() && spoke && !failed {
                if let Some(p) = pre {
                    p.end();
                }
                let _ = tx.send(Ok(Held::First {
                    model: fast.model_name(),
                    secs: first.unwrap_or_default(),
                }));
                for ev in kept {
                    if tx.send(Ok(Held::Event(ev))).is_err() {
                        return;
                    }
                }
                return;
            }
            if let Some(p) = pre {
                p.cancel();
            }
            *self.handed.lock().unwrap() = Some(names.clone());
            let handed = Held::HandOver {
                names,
                secs: began.elapsed().as_secs_f64(),
            };
            if tx.send(Ok(handed)).is_err() {
                return;
            }
        }
        let began = Instant::now();
        if let Some(p) = pre {
            p.begin();
        }
        let mut stream = self.main.stream(self.context, &self.tools, &self.options);
        let mut first = true;
        let mut called = false;
        loop {
            let ev = tokio::select! {
                biased;
                () = tx.closed() => return,
                ev = stream.next() => ev,
            };
            let Some(ev) = ev else { break };
            if first && matches!(ev, Ok(Event::Text(_)) | Ok(Event::ToolCall(_))) {
                first = false;
                let _ = tx.send(Ok(Held::First {
                    model: self.main.model_name(),
                    secs: began.elapsed().as_secs_f64(),
                }));
            }
            match &ev {
                Ok(Event::ToolCall(_)) => {
                    called = true;
                    if let Some(p) = pre {
                        p.cancel(); // what was written before a call is narration
                    }
                    if let Some((state, out)) = &self.held
                        && state
                            .compare_exchange(held::STREAMING, held::WITHDRAWN, SeqCst, SeqCst)
                            .is_ok()
                    {
                        // The answer reached a tool call before the turn ended: no tool
                        // runs for a turn that hadn't ended. The confirmed turn makes the
                        // call. Once confirmed, the call is the turn's own and streams on.
                        out.push_up(Frame::EagerCancel).await;
                        return;
                    }
                }
                Ok(Event::Text(t)) => {
                    if let Some(p) = pre {
                        p.text(t);
                    }
                }
                Ok(Event::Redo) => {
                    if let Some(p) = pre {
                        p.begin(); // the redo's words are another reply's
                    }
                }
                _ => {}
            }
            if tx.send(ev.map(Held::Event)).is_err() {
                return;
            }
        }
        if !called && let Some(p) = pre {
            p.end();
        }
    }
}

struct Speculation {
    text: String,
    events: Events,
    task: JoinHandle<()>,
    /// One of `held::*`.
    state: Arc<std::sync::atomic::AtomicU8>,
    /// Set when the fast model handed this answer over.
    handed: Handed,
}

pub struct LlmStage {
    model: Arc<dyn ModelPort>,
    /// The model a turn's first call goes to, when one is set (ADR 0050).
    fast: Option<Arc<dyn ModelPort>>,
    context: SharedContext,
    tools: Tools,
    options: Options,
    dispatcher: Arc<ToolDispatcher>,
    turns: Arc<TurnControl>,
    inject: Inject,
    runtime: Option<Arc<ConversationRuntime>>,
    compaction: Option<Compaction>,
    speculation: Option<Speculation>,
    /// The reply's first clause, rendered as it is written (ADR 0049).
    prerender: Option<Prerender>,
    /// A held answer the fast model handed over was then withdrawn: the turn's own run,
    /// for these words, goes to the main model, and these are the calls it was after.
    handed_for: Option<(String, Vec<String>)>,
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
            fast: None,
            context,
            tools,
            options,
            dispatcher,
            turns,
            inject,
            runtime,
            compaction: None,
            speculation: None,
            prerender: None,
            handed_for: None,
            last_input_tokens: 0,
        }
    }

    /// A fast model for a turn's first call (ADR 0050); None, the one model answers all.
    pub fn with_fast(mut self, fast: Option<Arc<dyn ModelPort>>) -> Self {
        self.fast = fast;
        self
    }

    /// Render the reply's first clause as it is written (`crate::tts::Prerender`).
    pub fn with_prerender(mut self, prerender: Option<Prerender>) -> Self {
        self.prerender = prerender;
        self
    }

    /// One model call on `context`. `first` is a turn's first call: the fast model's,
    /// when there is one.
    fn call(
        &self,
        context: Context,
        first: bool,
        held: Option<(Arc<std::sync::atomic::AtomicU8>, Out)>,
        handed: Handed,
    ) -> (Events, JoinHandle<()>) {
        let fast = self
            .fast
            .as_ref()
            .filter(|_| first)
            .map(|fast| {
                (
                    fast.clone(),
                    fast_tools(&self.tools),
                    fast_options(&self.options),
                )
            });
        Call {
            main: self.model.clone(),
            fast,
            context,
            tools: self.tools.clone(),
            options: self.options.clone(),
            prerender: self.prerender.clone(),
            held,
            handed,
        }
        .start()
    }

    fn cancel_prerender(&self) {
        if let Some(p) = &self.prerender {
            p.cancel();
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

    /// The tools as they are now (ADR 0046): an app connected or disconnected, or told
    /// Ghost its tools changed, or a tool was found by search, since the list was last
    /// built. Asked before every model call; the list the model is sent, and the apps'
    /// paragraphs of the prompt, are replaced only when the list differs, so the cached
    /// prompt is lost once per change and not on every turn.
    fn refresh_tools(&mut self) {
        let now = self.dispatcher.schemas();
        if now == self.tools {
            return;
        }
        info!(
            "tools: the list changed ({} to {} tools); advertised from this call",
            self.tools.len(),
            now.len()
        );
        self.tools = now;
        if let Some(rt) = &self.runtime
            && rt.has_reach()
        {
            self.context.lock().unwrap().system = rt.system_prompt(None);
        }
    }

    // --- the reply ----------------------------------------------------------------------

    /// One model run on the context, its calls, and the runs the results need. The turn's
    /// token is taken once: a run interrupted between two calls stops, rather than going
    /// on under the next turn's token.
    async fn run(&mut self, out: &Out) {
        let turn = self.turns.current();
        let mut first = true;
        loop {
            if turn.is_cancelled() {
                return;
            }
            self.refresh_tools();
            let context = self.snapshot();
            // A turn's first call, the operator's words last in the context, is the fast
            // model's; what follows a tool's result is the main model's.
            let said = match context.messages.last() {
                Some(Message::User(text)) if first => Some(text.clone()),
                _ => None,
            };
            first = false;
            let mut fast = said.is_some();
            if let Some((text, names)) = self.handed_for.take()
                && said.as_deref() == Some(text.as_str())
            {
                // The fast model already asked, for these words, in the answer that was
                // held and then withdrawn: straight to the main model.
                fast = false;
                out.push(Frame::HandOver { names, secs: 0.0 }).await;
            }
            let (events, task) = self.call(context, fast, None, Handed::default());
            let reply = self.consume(events, &turn, out).await;
            task.abort();
            let Some(reply) = reply else {
                return; // interrupted
            };
            if !self.after_reply(reply, &turn, out).await {
                return;
            }
        }
    }

    /// Stream one reply as frames, from the call's events (a held answer's are already
    /// waiting); None when the turn was interrupted.
    async fn consume(
        &mut self,
        mut events: Events,
        turn: &tokio_util::sync::CancellationToken,
        out: &Out,
    ) -> Option<Reply> {
        out.push(Frame::LlmStart).await;
        out.push(Frame::ReplyTurn(turn.clone())).await;
        let mut reply = Reply::default();
        let mut answering = self.model.model_name();
        loop {
            let next = tokio::select! {
                biased;
                _ = turn.cancelled() => {
                    self.cancel_prerender();
                    self.context.lock().unwrap().push_reply(reply.text.clone(), Vec::new());
                    out.push(Frame::LlmEnd).await;
                    return None;
                }
                ev = events.recv() => ev,
            };
            let Some(ev) = next else { break };
            match ev {
                Ok(Held::First { model, secs }) => {
                    answering = model;
                    out.push(Frame::Metrics(Metric::Ttfb {
                        value: secs,
                        model: Some(answering.clone()),
                    }))
                    .await;
                }
                Ok(Held::HandOver { names, secs }) => {
                    info!(
                        "fast model: handed the turn over after {secs:.2}s ({})",
                        if names.is_empty() {
                            "nothing to say".to_string()
                        } else {
                            names.join(", ")
                        }
                    );
                    reply.text.clear(); // nothing of the fast model's is the reply
                    out.push(Frame::HandOver { names, secs }).await;
                }
                Ok(Held::Event(Event::Text(t))) => {
                    reply.text.push_str(&t);
                    out.push(Frame::LlmText(t)).await;
                }
                Ok(Held::Event(Event::Thought(_))) => out.push(Frame::LlmThought).await,
                Ok(Held::Event(Event::ToolCall(c))) => reply.calls.push(c),
                Ok(Held::Event(Event::Usage(u))) => {
                    self.last_input_tokens = u.input_tokens
                        + u.cache_read_tokens.unwrap_or(0)
                        + u.cache_write_tokens.unwrap_or(0);
                    out.push(Frame::Metrics(Metric::Usage {
                        input: u.input_tokens,
                        output: u.output_tokens,
                        cache_read: u.cache_read_tokens.unwrap_or(0),
                        cache_write: u.cache_write_tokens.unwrap_or(0),
                        model: Some(answering.clone()),
                    }))
                    .await;
                }
                Ok(Held::Event(Event::Finished(_))) => {}
                Ok(Held::Event(Event::CutOff { .. })) => out.push(Frame::ReplyCut).await,
                Ok(Held::Event(Event::Redo)) => {
                    // The model's stream appended the cut reply and the note; mirror it.
                    let mut ctx = self.context.lock().unwrap();
                    ctx.push_reply(reply.text.clone(), Vec::new());
                    ctx.push(Message::user(CUT_OFF_NOTE));
                    reply.text.clear();
                }
                Ok(Held::Event(Event::Say(line))) => {
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
    /// A call to an app stops when `turn` is cancelled (the operator spoke) or the
    /// session's calls are stopped (they hung up): the stage is not held by an app that
    /// may take minutes, each call still gets a result in the context, and a change still
    /// gets its outcome in the audit (`ghost_core::apps::with_cancel`).
    async fn after_reply(
        &mut self,
        reply: Reply,
        turn: &tokio_util::sync::CancellationToken,
        out: &Out,
    ) -> bool {
        if !reply.calls.is_empty()
            && let Some(at) = self.turns.settles()
        {
            // The turn closed on the short grace (ADR 0049) and the model was quick: its
            // call waits until the grace would have run, as it did before, in case the
            // operator is only pausing.
            tokio::select! {
                biased;
                () = turn.cancelled() => {}
                () = tokio::time::sleep_until(at.into()) => {}
            }
        }
        if !reply.calls.is_empty() && (turn.is_cancelled() || !self.turns.acting()) {
            // The operator went on before Ghost answered: the turn is reopened, and nothing
            // is done for half of it. What was said of the reply goes too (`take_back`).
            self.context
                .lock()
                .unwrap()
                .push_reply(reply.text.clone(), Vec::new());
            out.push(Frame::LlmEnd).await;
            return false;
        }
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
        let stop = tokio_util::sync::CancellationToken::new();
        let hung_up = self.turns.calls_token();
        let n = reply.calls.len();
        self.turns.calls_began(n);
        let results: Vec<(ToolCall, CallResult)> = {
            let work = ghost_core::apps::with_cancel(
                stop.clone(),
                futures::future::join_all(reply.calls.iter().map(|c| {
                    let dispatcher = dispatcher.clone();
                    async move {
                        let r = dispatcher.call(&c.name, c.arguments.clone(), &c.id).await;
                        (c.clone(), r)
                    }
                })),
            );
            tokio::pin!(work);
            loop {
                tokio::select! {
                    results = &mut work => break results,
                    () = turn.cancelled(), if !stop.is_cancelled() => stop.cancel(),
                    () = hung_up.cancelled(), if !stop.is_cancelled() => stop.cancel(),
                }
            }
        };
        self.turns.calls_ended(n);
        // Stopped by a hang-up: nobody is there for an answer to the results.
        let mut run_again = !hung_up.is_cancelled();
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

    /// The operator's turn as the model sees it. Said over Ghost's reply, it carries a
    /// note: the echo canceller loses the first word or two of what is said over the
    /// voice (2026-10-30 19:57: "Can you close that one for me" reached the model as
    /// "Oh, you that one for me?", and it guessed).
    fn user_turn(&self, text: &str) -> Message {
        if self.turns.barged_in() {
            Message::user(format!("{text}{BARGED_IN_NOTE}"))
        } else {
            Message::user(text)
        }
    }

    async fn speculate(&mut self, text: String, out: &Out) {
        self.drop_speculation();
        self.refresh_tools();
        let mut context = self.snapshot();
        context.push(self.user_turn(&text));
        let state = Arc::new(std::sync::atomic::AtomicU8::new(held::STREAMING));
        let handed = Handed::default();
        // Its first token is timed where it is made, before any call of it is seen.
        let (events, task) = self.call(
            context,
            true,
            Some((state.clone(), out.clone())),
            handed.clone(),
        );
        self.speculation = Some(Speculation {
            text,
            events,
            task,
            state,
            handed,
        });
    }

    fn drop_speculation(&mut self) {
        if let Some(s) = self.speculation.take() {
            s.task.abort();
            self.cancel_prerender();
            self.remember_hand_over(&s);
        }
    }

    /// A held answer the fast model handed over, and that withdrew itself at the main
    /// model's tool call: the turn's own run need not ask the fast model again.
    fn remember_hand_over(&mut self, s: &Speculation) {
        use std::sync::atomic::Ordering::SeqCst;
        if s.state.load(SeqCst) == held::WITHDRAWN
            && let Some(names) = s.handed.lock().unwrap().clone()
        {
            self.handed_for = Some((s.text.clone(), names));
        }
    }

    /// The operator went on before any of the answer to `text` was heard or done
    /// (`TurnControl::reopen`): the turn and what was made of its answer leave the context,
    /// and the aggregator hands over the whole turn when it ends. The run it interrupted has
    /// already returned: frames reach this stage one at a time.
    fn take_back(&mut self, text: &str) {
        self.drop_speculation();
        self.cancel_prerender();
        self.turns.settle_reopened();
        let mut ctx = self.context.lock().unwrap();
        let Some(at) = ctx.messages.iter().rposition(|m| {
            // As the stage wrote it: the words, with the note when they cut Ghost off.
            matches!(m, Message::User(t) if t == text
                    || t.strip_suffix(BARGED_IN_NOTE) == Some(text))
        }) else {
            return;
        };
        let unanswered = ctx.messages[at + 1..].iter().all(|m| match m {
            Message::Assistant { tool_calls, .. } => tool_calls.is_empty(),
            Message::User(t) => t == CUT_OFF_NOTE,
            _ => false,
        });
        if unanswered {
            ctx.messages.truncate(at);
        } else {
            warn!("a reopened turn had an answer that acted; it stays in the context");
        }
    }

    /// The grace ended with the same words: the held answer is the answer, spoken now.
    async fn confirm_speculation(&mut self, text: String, out: &Out) {
        let held = match self.speculation.take() {
            Some(s) if s.text == text => Some(s),
            Some(other) => {
                other.task.abort(); // an answer to other words
                None
            }
            None => None,
        };
        let Some(s) = held else {
            // No answer to these words is held (it was dropped on the way, or never
            // started): the turn still gets one, from its own run.
            warn!(
                "the turn closed on a held answer the model's stage does not have; answering it now"
            );
            self.cancel_prerender();
            let turn = self.user_turn(&text);
            self.context.lock().unwrap().push(turn);
            self.run(out).await;
            return;
        };
        let turn = self.user_turn(&s.text);
        self.context.lock().unwrap().push(turn);
        {
            use std::sync::atomic::Ordering::SeqCst;
            if s.state
                .compare_exchange(held::STREAMING, held::CONFIRMED, SeqCst, SeqCst)
                .is_err()
            {
                // Withdrawn first: the turn's own run.
                s.task.abort();
                self.cancel_prerender();
                self.remember_hand_over(&s);
                if let Some((text, _)) = &mut self.handed_for {
                    // The run finds the turn as the context holds it (a note may follow
                    // the words).
                    if let Some(Message::User(last)) = self.context.lock().unwrap().messages.last()
                    {
                        *text = last.clone();
                    }
                }
                self.run(out).await;
                return;
            }
        }
        let turn = self.turns.current();
        // What was held, then the rest as it comes; the call's task ends the channel.
        let Speculation { events, task, .. } = s;
        let reply = self.consume(events, &turn, out).await;
        task.abort();
        let Some(reply) = reply else {
            return;
        };
        // The run loop continues on the results, when a call asks for one (not after a
        // `hang_up` or a `hold`, whose result nothing follows).
        if self.after_reply(reply, &turn, out).await {
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
            Frame::SpeculationConfirmed(text) => self.confirm_speculation(text, out).await,
            Frame::SpeculationCancelled => self.drop_speculation(),
            Frame::ReopenTurn(text) => self.take_back(&text),
            Frame::Interruption => {
                self.drop_speculation();
                self.cancel_prerender();
                out.forward(Frame::Interruption, direction).await;
            }
            Frame::End | Frame::Cancel => {
                self.drop_speculation();
                self.cancel_prerender();
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
/// `ttfb`, at `rate` tokens a second when one is set (`paced`). A line is the n-th user
/// turn of the context (system notes aside). A reply
/// whose stream was dropped before its end (a speculation withdrawn or thrown away) is
/// served again, as a real model regenerates its answer on the confirmed turn.
pub struct ScriptedModel {
    lines: Vec<Vec<ScriptedReply>>,
    served: Arc<Mutex<Vec<usize>>>,
    ttfb: std::time::Duration,
    /// Output tokens a second; None streams the reply at once.
    rate: Option<f64>,
    name: String,
    pub calls: Mutex<usize>,
}

/// A word of English is about this many tokens.
const TOKENS_PER_WORD: f64 = 1.3;

/// What a tool call costs the model to write: its name and framing, then its arguments
/// at about four characters a token.
fn call_tokens(call: &ToolCall) -> f64 {
    let args = serde_json::to_string(&call.arguments).map_or(0, |s| s.len());
    12.0 + args as f64 / 4.0
}

#[derive(Debug, Clone, Default)]
pub struct ScriptedReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// This reply's first byte comes after this instead of the model's `ttfb`.
    pub ttfb: Option<std::time::Duration>,
}

impl ScriptedReply {
    /// A slow first byte for this reply alone: past the grace, to meet the close.
    pub fn after(mut self, ttfb: std::time::Duration) -> Self {
        self.ttfb = Some(ttfb);
        self
    }
}

impl ScriptedModel {
    pub fn new(lines: Vec<Vec<ScriptedReply>>, ttfb: std::time::Duration) -> Arc<Self> {
        let n = lines.len();
        Arc::new(Self {
            lines,
            served: Arc::new(Mutex::new(vec![0; n])),
            ttfb,
            rate: None,
            name: "scripted".into(),
            calls: Mutex::new(0),
        })
    }

    /// The same script from a model with a first-token delay and a token rate: a word
    /// costs [`TOKENS_PER_WORD`] tokens and a tool call its arguments' length, so a reply
    /// takes as long to write as a real model takes. `name` is what the timeline calls it.
    pub fn paced(
        lines: Vec<Vec<ScriptedReply>>,
        ttfb: std::time::Duration,
        tokens_per_sec: Option<f64>,
        name: &str,
    ) -> Arc<Self> {
        let n = lines.len();
        Arc::new(Self {
            lines,
            served: Arc::new(Mutex::new(vec![0; n])),
            ttfb,
            rate: tokens_per_sec.filter(|r| *r > 0.0),
            name: name.into(),
            calls: Mutex::new(0),
        })
    }

    pub fn say(text: &str) -> ScriptedReply {
        ScriptedReply {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn call(text: &str, name: &str, args: serde_json::Map<String, Value>) -> ScriptedReply {
        ScriptedReply {
            text: text.into(),
            tool_calls: vec![ToolCall::new(format!("call_{name}"), name, args)],
            ..Default::default()
        }
    }

    /// Which line of the script the context is on: its user turns, the notes aside. A
    /// turn past the script (a line the pause split in two) is the last line's, and a
    /// line's replies, once spent, end on its last one.
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
        // A line whose replies are spent (the second half of a split line) gets its last
        // reply again, as a real model answers the second half of what it already answered.
        // What the context holds of this turn's answer so far: a turn that was reopened
        // has none of it left, whatever was streamed for its first half, and a real model
        // asked again starts again (its tool call included). A turn past the script is
        // the last line's, spent, as above.
        let users = context
            .messages
            .iter()
            .filter(|m| matches!(m, Message::User(t) if !t.starts_with("[system]")))
            .count();
        let answered = context
            .messages
            .iter()
            .rev()
            .take_while(|m| !matches!(m, Message::User(t) if !t.starts_with("[system]")))
            .filter(|m| matches!(m, Message::Assistant { .. }))
            .count();
        let reply = line
            .and_then(|l| {
                let mut i = *self.served.lock().unwrap().get(l)?;
                if users <= self.lines.len() {
                    i = i.min(answered);
                }
                let replies = self.lines.get(l)?;
                replies.get(i).or_else(|| replies.last()).cloned()
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
        let ttfb = reply.ttfb.unwrap_or(self.ttfb);
        Box::pin(async_stream_words(
            words,
            reply.tool_calls,
            ttfb,
            self.rate,
            done,
        ))
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
/// speculation that never became the answer. With `rate`, each event after the first
/// waits the time its tokens take to write.
fn async_stream_words(
    words: Vec<String>,
    calls: Vec<ToolCall>,
    ttfb: std::time::Duration,
    rate: Option<f64>,
    done: impl Fn() + Send + 'static,
) -> impl futures::Stream<Item = Result<Event, LlmError>> + Send {
    let wait = |tokens: f64| match rate {
        Some(rate) => std::time::Duration::from_secs_f64(tokens / rate),
        None => std::time::Duration::ZERO,
    };
    let mut events: Vec<(std::time::Duration, Event)> = words
        .into_iter()
        .map(|w| (wait(TOKENS_PER_WORD), Event::Text(w)))
        .collect();
    let n_calls = calls.len();
    events.extend(
        calls
            .into_iter()
            .map(|c| (wait(call_tokens(&c)), Event::ToolCall(c))),
    );
    // The first event is the first token: `ttfb` is its whole wait. A call is written
    // before it is seen, so a reply that opens with one keeps the call's own time.
    if let Some((first, ev)) = events.first_mut()
        && !matches!(ev, Event::ToolCall(_))
    {
        *first = std::time::Duration::ZERO;
    }
    let events: Vec<(std::time::Duration, Event)> = events
        .into_iter()
        .chain([(
            std::time::Duration::ZERO,
            Event::Usage(ghost_llm::Usage {
                input_tokens: 100,
                output_tokens: 20,
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(0),
                reasoning_tokens: None,
            }),
        )])
        .chain([(
            std::time::Duration::ZERO,
            Event::Finished(if n_calls > 0 {
                ghost_llm::FinishReason::ToolUse
            } else {
                ghost_llm::FinishReason::End
            }),
        )])
        .collect();
    let total = events.len();
    futures::stream::unfold(
        (events.into_iter(), 0usize, Some(done)),
        move |(mut it, taken, done)| async move {
            if taken == 0 {
                tokio::time::sleep(ttfb).await;
            }
            let (wait, ev) = it.next()?;
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fast_model_is_asked_without_the_thinking_setting() {
        // Haiku 5.5 refuses `between_tools` with a 400, so every turn was handed over.
        let main = Options::default();
        assert_eq!(main.thinking.as_deref(), Some("between_tools"));
        let fast = fast_options(&main);
        assert_eq!(fast.thinking, None);
        assert_eq!(fast.max_tokens, main.max_tokens);
        assert_eq!(fast.cache_system, main.cache_system);
    }
}
