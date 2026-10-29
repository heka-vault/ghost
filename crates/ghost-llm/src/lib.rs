//! The model client Ghost uses everywhere, over the one connection it shares across
//! sessions (ADR 0037; the port of `ghost_voice.llm` with the context and tool shapes
//! Pipecat's adapters gave it).
//!
//! [`Model`] is the interface: the session's streamed reply ([`Model::stream`]), a one-shot
//! completion (a watch's model step, [`Model::complete`]), and a probe that times the first
//! token (the desktop's Test connection, [`Model::probe`]). Where a call goes and what key
//! it sends is the connection's [`Target`](ghost_core::connection::Target): the Messages
//! API for `anthropic`, or OpenAI-compatible Chat Completions (OpenAI, Ollama, LM Studio, a
//! URL of the operator's). [`Context`] is the conversation as Ghost keeps it, converted to
//! each protocol's wire shape and back; [`Tools`] are the schemas offered to the model,
//! bounded as the voice package bounded them.
//!
//! Both protocols carry the same fixes the Anthropic path earned:
//!
//! - Every tool call in a reply is delivered. The Messages API path keeps every `tool_use`
//!   block of a streamed reply, and the Chat Completions path accumulates every index of
//!   the `tool_calls` deltas, whole when a server (Ollama) sends a reply's calls in one
//!   chunk. The prompt asks the model to make independent lookups together in one reply;
//!   a client that kept one call lost the others without a word.
//! - A reply that hits the output cap (`LLM_MAX_TOKENS`, tool calls included) is not
//!   dropped silently. The Messages API ends it with `stop_reason: max_tokens` and Chat
//!   Completions with `finish_reason: length`, or the stream ends inside a call's
//!   arguments. On 2026-10-24 four comments the operator had just confirmed were never
//!   posted, and Ghost went quiet. Now the tool calls of a cut reply are not delivered
//!   (half a batch is worse than none); the model is told, once, to redo it in smaller
//!   steps, and a second cut is said aloud instead. Either way the turn is flagged
//!   `cut_off` ([`Event::CutOff`]).
//!
//! The Messages API path puts a prompt-cache breakpoint on the system prompt. Pipecat marks
//! the last two user messages, which caches a conversation as it grows; but a new session
//! starts with new words, so nothing it wrote before is a prefix of the next session's
//! request, and the tool schemas plus the system prompt (about 3k tokens) were written
//! again at every session's first call. With the marker that prefix is read back instead.
//! OpenAI caches a long prefix by itself.
//!
//! [`shared`] keeps one client per connection, so a session doesn't open a new connection
//! (and an HTTPS handshake) on its first call.
//!
//! A key is sent in a header and nowhere else: never in a `Debug`, never in an error. A
//! non-2xx body is never logged whole: an error carries its first line, redacted.

pub mod context;
pub mod model;
mod reply;
pub mod sse;
pub mod tools;

pub use context::{
    AnthropicContext, Context, ContextError, Message, ToolCall, with_system_breakpoint,
};
pub use model::{
    ANTHROPIC_BETA, CUT_OFF_LINE, CUT_OFF_NOTE, DEFAULT_THINKING, Event, FinishReason,
    LLM_MAX_TOKENS, LlmError, Model, Options, Probe, Usage, prefill_supported, shared,
};
pub use tools::{
    BRING_UP_TOOL, CHANGE_TOOL, HANG_UP_TOOL, HOLD_TOOL, OfferAll, Offered, Offering, Offers,
    SWITCH_TOOL, SessionTools, ToolSchema, Tools, tool_name_ok,
};
