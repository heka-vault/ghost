//! The provider check (ADR 0055): what the operator's connection does with each thing a
//! spoken turn needs, asked with a fixed synthetic prompt and nothing of the operator's.
//!
//! For one model of the connection: whether it answers at all (or the error, in plain
//! words), the time to the first byte and to the first word over a few short streamed
//! calls (median and worst), whether the reply streams or arrives all at once, whether a
//! tool call comes back as one, whether a second identical request reads its prefix from
//! the cache, and whether the thinking setting a session sends is accepted. What it finds
//! is the route's [`Capabilities`], which the daemon keeps and seeds at start so every
//! request is shaped to it.
//!
//! The check sends each request exactly as it is shaped (`Model::measure`, no retry): a
//! refusal is the finding. Where the provider doesn't report a thing (a local server's
//! cache) the answer is "not reported", never a guess.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::capabilities::Capabilities;
use crate::context::Context;
use crate::model::{Measured, Model, Options, REASONING_EFFORT, spoken_failure};
use crate::tools::{ToolSchema, Tools};
use crate::{LLM_MAX_TOKENS, LlmError};
use ghost_core::connection::Protocol;

/// Short calls timed per model.
pub const SAMPLES: usize = 3;
/// Asked for the timing: a reply of a few words in several pieces.
pub const LATENCY_PROMPT: &str =
    "Count from one to ten in words, separated by spaces. Write nothing else.";
pub const TOOL_NAME: &str = "clock_read";
pub const TOOL_PROMPT: &str = "What time is it? Call the clock_read tool to find out.";
pub const CACHE_PROMPT: &str = "Reply with the single word: ready.";
/// About this long and a spoken reply starts late enough to notice: the model's first
/// word, before Ghost says anything (ADR 0018's budget, 2026-11-12's evidence).
pub const SLOW_FIRST_WORD_SECS: f64 = 1.5;

/// The fixed system prompt of the cache probe: about 4,500 tokens, past every current
/// model's smallest cacheable prefix (512 tokens on the 5.x models, 4,096 on Haiku 4.5).
pub fn cache_text() -> String {
    let mut s = String::from(
        "You are the subject of Ghost's provider check. This text is fixed and synthetic: \
         it holds no one's words and changes only when Ghost's code does.\n",
    );
    for i in 1..=140 {
        s.push_str(&format!(
            "Entry {i}: a line of reference text the check sends twice, so the second \
             request can read the same prefix from the provider's prompt cache.\n"
        ));
    }
    s
}

/// One finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Yes,
    No,
    /// The provider gave no figure to tell (a local server's cache, usually).
    NotReported,
    /// Nothing to check: no thinking setting is sent on this route.
    NotApplicable,
    /// Not tried: the model didn't answer at all.
    Untested,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub state: State,
    /// In plain words, when there is more to say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Verdict {
    fn of(state: State, detail: impl Into<String>) -> Self {
        let d = detail.into();
        Self {
            state,
            detail: (!d.is_empty()).then_some(d),
        }
    }

    fn untested() -> Self {
        Self::of(State::Untested, "")
    }

    pub fn yes(&self) -> Option<bool> {
        match self.state {
            State::Yes => Some(true),
            State::No => Some(false),
            _ => None,
        }
    }
}

/// Median and worst, in seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub median: Option<f64>,
    pub worst: Option<f64>,
}

impl Timing {
    pub fn of(xs: &[f64]) -> Self {
        if xs.is_empty() {
            return Self::default();
        }
        let mut v = xs.to_vec();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        let median = if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        };
        Self {
            median: Some(median),
            worst: v.last().copied(),
        }
    }
}

/// What the check found for one model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCheck {
    /// `main` or `fast` (ADR 0050).
    pub role: String,
    /// As the provider is asked for it.
    pub model: String,
    /// As the provider said it answered, when it says.
    #[serde(default)]
    pub answered_by: Option<String>,
    pub reachable: bool,
    /// Why it didn't answer, or what failed after it did, in plain words.
    #[serde(default)]
    pub error: Option<String>,
    pub first_byte: Timing,
    pub first_word: Timing,
    /// Timed calls that answered.
    pub samples: usize,
    pub streaming: Verdict,
    pub tools: Verdict,
    pub caching: Verdict,
    pub thinking: Verdict,
    /// The thinking setting tried (`between_tools`, `reasoning_effort`), if any.
    #[serde(default)]
    pub thinking_mode: Option<String>,
}

impl ModelCheck {
    /// A model that couldn't be asked at all: `error` says why.
    pub fn failed(role: &str, model: &str, error: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            model: model.into(),
            answered_by: None,
            reachable: false,
            error: Some(error.into()),
            first_byte: Timing::default(),
            first_word: Timing::default(),
            samples: 0,
            streaming: Verdict::untested(),
            tools: Verdict::untested(),
            caching: Verdict::untested(),
            thinking: Verdict::untested(),
            thinking_mode: None,
        }
    }

    /// What a session's requests should take from this.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            rejects_thinking: (self.thinking.state == State::No)
                .then(|| self.thinking_mode.clone())
                .flatten(),
            rejects_cache_markers: self.caching.state == State::No
                && self
                    .caching
                    .detail
                    .as_deref()
                    .is_some_and(|d| d.contains("cache_control")),
            caches: self.caching.yes(),
            tools: self.tools.yes(),
            streams: self.streaming.yes(),
        }
    }

    /// The first word's median is past what a spoken reply can hide.
    pub fn slow(&self) -> bool {
        self.first_word
            .median
            .is_some_and(|m| m > SLOW_FIRST_WORD_SECS)
    }
}

/// What the check sends for one model.
#[derive(Clone, Debug)]
pub struct CheckOptions {
    /// `main` or `fast`.
    pub role: String,
    pub samples: usize,
    /// The thinking setting a session sends on the Messages API (`GHOST_LLM_THINKING`).
    pub thinking: Option<String>,
    /// Whether the timed calls carry it, as the main model's turns do; the fast model's
    /// don't (`ghost_voice::llm::fast_options`).
    pub timed_with_thinking: bool,
}

impl CheckOptions {
    pub fn main(thinking: Option<String>) -> Self {
        Self {
            role: "main".into(),
            samples: SAMPLES,
            thinking,
            timed_with_thinking: true,
        }
    }

    pub fn fast(thinking: Option<String>) -> Self {
        Self {
            role: "fast".into(),
            samples: SAMPLES,
            thinking,
            timed_with_thinking: false,
        }
    }
}

/// An error, for the operator: the plain sentence, then what the provider said.
pub fn plain(e: &LlmError) -> String {
    let sentence = spoken_failure(e);
    let sentence = sentence
        .strip_suffix(" Try again in a moment.")
        .unwrap_or(&sentence);
    format!("{sentence} ({e})")
}

fn refused_thinking(e: &LlmError) -> bool {
    matches!(e, LlmError::Status { status: 400 | 422, detail }
        if { let d = detail.to_lowercase(); d.contains("thinking") || d.contains("between_tools") || d.contains("reasoning") })
}

fn options(max_tokens: u32, thinking: Option<String>, cache_system: bool) -> Options {
    Options {
        max_tokens,
        thinking,
        cache_system,
    }
}

/// Check one model of the connection. Never fails: what went wrong is in the result.
pub async fn check(model: &Model, opts: &CheckOptions) -> ModelCheck {
    let target = model.target();
    let protocol = target.protocol;
    let mut out = ModelCheck::failed(&opts.role, &target.model, "");
    out.error = None;
    let mut caps = Capabilities::default();
    let plain_ctx = Context::new("").user(LATENCY_PROMPT);
    let none = Tools::default();

    // 1. Does it answer, and does it take the thinking setting a session sends?
    let thinking = match protocol {
        Protocol::Anthropic => opts.thinking.clone(),
        Protocol::Openai => (!target.reasoning.effort.is_empty()).then(|| REASONING_EFFORT.into()),
    };
    out.thinking_mode = thinking.clone();
    let first = model
        .measure_shaped(
            &plain_ctx,
            &none,
            &options(1024, thinking.clone(), false),
            &caps,
        )
        .await;
    match (&first.error, &thinking) {
        (None, Some(mode)) => {
            out.thinking = Verdict::of(State::Yes, format!("accepts {mode}"));
        }
        (None, None) => {
            out.thinking = Verdict::of(State::NotApplicable, "no thinking setting is sent");
        }
        (Some(e), Some(mode)) if refused_thinking(e) => {
            out.thinking = Verdict::of(
                State::No,
                format!("refuses {mode}; Ghost sends its requests without it ({e})"),
            );
            caps.rejects_thinking = Some(mode.clone());
        }
        (Some(e), _) => {
            out.error = Some(plain(e));
            return out;
        }
    }
    out.reachable = true;
    note_model(&mut out, &first);

    // 2. Timed: the first byte and the first word, as a turn sends them.
    let timed_thinking = if opts.timed_with_thinking && protocol == Protocol::Anthropic {
        thinking.filter(|t| caps.takes_thinking(t))
    } else {
        None
    };
    let mut bytes = Vec::new();
    let mut words = Vec::new();
    let mut pieces = Vec::new();
    for _ in 0..opts.samples.max(1) {
        let m = model
            .measure_shaped(
                &plain_ctx,
                &none,
                &options(LLM_MAX_TOKENS, timed_thinking.clone(), false),
                &caps,
            )
            .await;
        if let Some(e) = &m.error {
            out.error.get_or_insert_with(|| plain(e));
            continue;
        }
        note_model(&mut out, &m);
        bytes.extend(m.first_byte);
        words.extend(m.first_word);
        if m.first_word.is_some() {
            pieces.push(m.text_chunks);
        }
    }
    out.samples = words.len();
    out.first_byte = Timing::of(&bytes);
    out.first_word = Timing::of(&words);
    out.streaming = if pieces.is_empty() {
        Verdict::of(State::Untested, "no reply had words in it")
    } else if pieces.iter().any(|&n| n >= 2) {
        Verdict::of(State::Yes, "")
    } else {
        Verdict::of(
            State::No,
            "every reply arrived all at once, so Ghost can't start speaking until the whole reply is in",
        )
    };

    // 3. A tool call comes back as one.
    let tool = Tools::new(vec![ToolSchema::new(
        TOOL_NAME,
        "Reads the current time. Takes no arguments.",
        json!({}),
        Vec::new(),
    )]);
    let ctx =
        Context::new("You are a test of tool calling. Answer by calling a tool.").user(TOOL_PROMPT);
    let m = model
        .measure_shaped(&ctx, &tool, &options(LLM_MAX_TOKENS, None, false), &caps)
        .await;
    out.tools = match (&m.error, m.calls.iter().find(|c| c.name == TOOL_NAME)) {
        (Some(e), _) => Verdict::of(State::No, plain(e)),
        (None, Some(_)) => Verdict::of(State::Yes, ""),
        (None, None) if !m.calls.is_empty() => {
            Verdict::of(State::No, "it called a tool that wasn't offered")
        }
        (None, None) => Verdict::of(
            State::No,
            "it answered in words instead of calling the tool; apps and Ghost's own tools won't work well",
        ),
    };

    // 4. A second identical request reads its prefix from the cache.
    let ctx = Context::new(cache_text()).user(CACHE_PROMPT);
    let opts_cache = options(16, None, true);
    let a = model.measure_shaped(&ctx, &none, &opts_cache, &caps).await;
    out.caching = match &a.error {
        Some(e) => {
            let d = plain(e);
            if d.contains("cache_control") {
                Verdict::of(State::No, d)
            } else {
                Verdict::of(State::Untested, d)
            }
        }
        None => {
            let b = model.measure_shaped(&ctx, &none, &opts_cache, &caps).await;
            match (&b.error, b.usage.as_ref().and_then(|u| u.cache_read_tokens)) {
                (Some(e), _) => Verdict::of(State::Untested, plain(e)),
                (None, Some(n)) if n > 0 => {
                    Verdict::of(State::Yes, format!("{n} tokens read from the cache"))
                }
                (None, Some(_)) => Verdict::of(
                    State::No,
                    "a second identical request didn't read from the cache; every turn pays for the whole prompt",
                ),
                (None, None) => Verdict::of(State::NotReported, "the provider doesn't report it"),
            }
        }
    };
    out
}

/// Another route the operator has tried, as the advice names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Tried {
    /// "claude-haiku-5-5 via cliproxy".
    pub label: String,
    /// Its first word's median, in seconds.
    pub first_word: f64,
}

/// How much faster another tried route must be to be named.
const NAMED_IF_FASTER_BY: f64 = 0.3;

/// What would help, in plain words, when the connection is too slow for a good spoken
/// reply: the model that answers a spoken turn first (the fast model when there is one) took
/// longer than [`SLOW_FIRST_WORD_SECS`] to its first word. `live` is the same measure
/// from the operator's own recent turns (median seconds, how many). None when it is quick
/// enough. Ghost never changes the connection itself: this only says what would help.
pub fn advice(
    models: &[ModelCheck],
    has_fast: bool,
    live: Option<(f64, usize)>,
    others: &[Tried],
) -> Option<String> {
    let main = models.iter().find(|m| m.role == "main");
    let fast = models.iter().find(|m| m.role == "fast");
    let first = fast.filter(|f| f.reachable).or(main);
    let checked = first
        .filter(|f| f.reachable)
        .and_then(|f| f.first_word.median);
    let name = first.map_or("the model", |f| f.model.as_str());
    let (secs, said) = match (live, checked) {
        (Some((l, n)), _) if l > SLOW_FIRST_WORD_SECS => (
            l,
            format!("Your last {n} spoken replies waited {l:.1} s for the model's first word."),
        ),
        (_, Some(c)) if c > SLOW_FIRST_WORD_SECS => (
            c,
            format!(
                "Spoken replies will start slowly: {name} took {c:.1} s to its first word on this connection."
            ),
        ),
        _ => return None,
    };
    let mut helps = Vec::new();
    match fast {
        None if !has_fast => helps.push(
            "A fast model on this connection (a small one, like a Haiku) answers small talk sooner."
                .to_string(),
        ),
        Some(f) if f.slow() => helps.push(format!("The fast model, {}, is slow here too.", f.model)),
        _ => {}
    }
    if let Some(best) = others
        .iter()
        .filter(|o| o.first_word + NAMED_IF_FASTER_BY < secs)
        .min_by(|a, b| a.first_word.total_cmp(&b.first_word))
    {
        helps.push(format!(
            "Of the connections you've tested, {} answered in {:.1} s.",
            best.label, best.first_word
        ));
    }
    if first.is_some_and(|f| f.streaming.state == State::No) {
        helps.push("This connection sends each reply all at once.".into());
    }
    helps.push("Ghost won't change the connection for you: it's in Settings, under Model.".into());
    Some(format!("{said} {}", helps.join(" ")))
}

fn note_model(out: &mut ModelCheck, m: &Measured) {
    if out.answered_by.is_none() && !m.model.is_empty() {
        out.answered_by = Some(m.model.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick(role: &str, model: &str, first_word: f64) -> ModelCheck {
        ModelCheck {
            role: role.into(),
            model: model.into(),
            answered_by: None,
            reachable: true,
            error: None,
            first_byte: Timing::default(),
            first_word: Timing::of(&[first_word]),
            samples: 1,
            streaming: Verdict::of(State::Yes, ""),
            tools: Verdict::of(State::Yes, ""),
            caching: Verdict::of(State::Yes, ""),
            thinking: Verdict::of(State::NotApplicable, ""),
            thinking_mode: None,
        }
    }

    #[test]
    fn a_quick_connection_needs_no_advice() {
        assert_eq!(advice(&[quick("main", "s", 0.8)], false, None, &[]), None);
        // The fast model answers first: a slow main model behind a quick one is fine.
        let both = [quick("main", "s", 2.0), quick("fast", "h", 0.6)];
        assert_eq!(advice(&both, true, None, &[]), None);
    }

    #[test]
    fn a_slow_connection_says_what_would_help_and_never_switches() {
        let tried = [
            Tried {
                label: "claude-haiku-5-5 via cliproxy".into(),
                first_word: 0.7,
            },
            Tried {
                label: "x via ollama".into(),
                first_word: 2.0,
            },
        ];
        let a = advice(
            &[quick("main", "anthropic/claude-sonnet-5.5", 2.1)],
            false,
            None,
            &tried,
        )
        .unwrap();
        assert!(a.contains("2.1 s"), "{a}");
        assert!(a.contains("A fast model"), "{a}");
        assert!(
            a.contains("claude-haiku-5-5 via cliproxy answered in 0.7 s"),
            "{a}"
        );
        assert!(!a.contains("ollama"), "{a}");
        assert!(a.contains("won't change the connection"), "{a}");
        let both = [quick("main", "s", 2.0), quick("fast", "h", 1.9)];
        let a = advice(&both, true, None, &[]).unwrap();
        assert!(a.contains("The fast model, h, is slow here too."), "{a}");
    }

    #[test]
    fn the_operators_own_turns_say_it_too() {
        let a = advice(&[quick("main", "s", 0.9)], false, Some((1.8, 10)), &[]).unwrap();
        assert!(
            a.starts_with("Your last 10 spoken replies waited 1.8 s"),
            "{a}"
        );
        // No check yet: the operator's turns alone say it.
        assert!(advice(&[], true, Some((2.0, 6)), &[]).is_some());
        assert_eq!(
            advice(&[quick("main", "s", 0.9)], false, Some((1.2, 10)), &[]),
            None
        );
    }

    #[test]
    fn a_median_and_the_worst() {
        let t = Timing::of(&[0.9, 0.5, 2.0]);
        assert_eq!(t.median, Some(0.9));
        assert_eq!(t.worst, Some(2.0));
        assert_eq!(Timing::of(&[1.0, 2.0]).median, Some(1.5));
        assert_eq!(Timing::of(&[]), Timing::default());
    }

    #[test]
    fn the_cache_text_is_long_enough_and_fixed() {
        let t = cache_text();
        assert!(t.len() > 19_000, "{} chars", t.len());
        assert_eq!(t, cache_text());
    }

    #[test]
    fn a_refused_thinking_mode_becomes_the_routes() {
        let mut c = ModelCheck {
            role: "fast".into(),
            model: "m".into(),
            answered_by: None,
            reachable: true,
            error: None,
            first_byte: Timing::default(),
            first_word: Timing::of(&[2.0]),
            samples: 1,
            streaming: Verdict::of(State::Yes, ""),
            tools: Verdict::of(State::Yes, ""),
            caching: Verdict::of(State::NotReported, ""),
            thinking: Verdict::of(State::No, "refuses between_tools"),
            thinking_mode: Some("between_tools".into()),
        };
        let caps = c.capabilities();
        assert_eq!(caps.rejects_thinking.as_deref(), Some("between_tools"));
        assert_eq!(caps.caches, None);
        assert!(c.slow());
        c.first_word = Timing::of(&[0.8]);
        assert!(!c.slow());
    }
}
