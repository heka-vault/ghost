//! A real model shaped to stand for real use (`--pad-tokens`, `--first-byte-ms`).
//!
//! The harness's prompt is about 4,000 tokens of system and tools; the operator's is about
//! 15,000 to 18,000 (four apps, their schemas, Ghost's own app; `cache_read` in his
//! `latency.jsonl`). And the local subscription proxy answers Haiku 5.5's first byte about
//! a second sooner than OpenRouter does (2026-11-12). Neither is something the bench would
//! see on its own, so a run may ask for both:
//!
//! - **Padding**: a fixed synthetic text appended to the system prompt, about `pad_tokens`
//!   tokens of it (four characters a token). It sits inside the cached prefix, as the real
//!   prompt's tools and paragraphs do, so a turn pays what a larger prefix costs to read
//!   from the cache and a warm-up writes it. It is the same text on every call and in every
//!   run, so it caches like the real prompt and never varies a result by itself. It is
//!   reference material the model is told it does not need: it must not change what the
//!   model does, only what it reads.
//! - **A first-byte delay**: every streamed call waits `first_byte` before it is sent, so
//!   its first byte comes that much later, as a slow provider's does. Both models get it
//!   (a slow route is slow for the fast model and the main one alike); the warm-up does not,
//!   since nobody waits on it.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use ghost_llm::{Context, Event, LlmError, Options, Tools};

use crate::llm::ModelPort;

/// Characters per token, roughly, for English prose to Claude's tokenizer.
const CHARS_PER_TOKEN: usize = 4;

/// About `tokens` tokens of fixed, synthetic reference text: numbered entries of a made-up
/// style guide, varied enough not to compress to nothing and the same on every call.
pub fn padding(tokens: usize) -> String {
    if tokens == 0 {
        return String::new();
    }
    const SUBJECTS: [&str; 8] = [
        "a status line",
        "a release note",
        "a meeting summary",
        "a changelog entry",
        "an incident review",
        "a design memo",
        "a test plan",
        "a weekly digest",
    ];
    const RULES: [&str; 6] = [
        "names the owner in its first sentence and keeps the date in ISO form",
        "keeps each paragraph under five sentences and puts the decision before the reasons",
        "spells out every acronym the first time and links the ticket it came from",
        "lists open questions last, each with the person who can answer it",
        "uses the past tense for what happened and the present for what holds now",
        "says what was measured, how, and on which machine, before any conclusion",
    ];
    let mut out = String::from(
        "\n\n## Background reference (not needed for any request)\n\nThe entries below are \
         a fixed house style guide kept here for reference. No request needs them; never \
         quote them, act on them or mention them.\n",
    );
    let want = tokens * CHARS_PER_TOKEN;
    let mut i = 0usize;
    while out.len() < want {
        let line = format!(
            "\n{}. Entry {}: {} {}; reviewed in cycle {} of section {}.",
            i + 1,
            i * 7 % 1000 + 100,
            SUBJECTS[i % SUBJECTS.len()],
            RULES[(i / SUBJECTS.len()) % RULES.len()],
            i % 13 + 1,
            (b'A' + (i % 26) as u8) as char,
        );
        out.push_str(&line);
        i += 1;
    }
    out
}

/// `inner` with its system prompt padded and its first byte held back.
pub struct Shaped {
    inner: Arc<dyn ModelPort>,
    pad: Arc<String>,
    first_byte: Duration,
}

impl Shaped {
    /// None when there is nothing to shape: `inner` as it is.
    pub fn wrap(
        inner: Arc<dyn ModelPort>,
        pad_tokens: usize,
        first_byte: Duration,
    ) -> Arc<dyn ModelPort> {
        if pad_tokens == 0 && first_byte.is_zero() {
            return inner;
        }
        Arc::new(Self {
            inner,
            pad: Arc::new(padding(pad_tokens)),
            first_byte,
        })
    }

    fn padded(&self, mut context: Context) -> Context {
        context.system.push_str(&self.pad);
        context
    }
}

impl ModelPort for Shaped {
    fn stream(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        let context = self.padded(context);
        if self.first_byte.is_zero() {
            return self.inner.stream(context, tools, options);
        }
        let (inner, tools, options, wait) = (
            self.inner.clone(),
            tools.clone(),
            options.clone(),
            self.first_byte,
        );
        // Sent after the wait, so dropping the stream during it sends nothing.
        futures::stream::once(async move {
            tokio::time::sleep(wait).await;
            inner.stream(context, &tools, &options)
        })
        .flatten()
        .boxed()
    }

    fn warm_cache(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> BoxFuture<'static, Result<Option<ghost_llm::Usage>, LlmError>> {
        self.inner.warm_cache(self.padded(context), tools, options)
    }

    fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> BoxFuture<'_, Result<String, LlmError>> {
        self.inner.complete(system, user, max_tokens)
    }

    fn model_name(&self) -> String {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn padding_is_about_the_tokens_asked_and_the_same_every_time() {
        assert!(padding(0).is_empty());
        let p = padding(11_000);
        let tokens = p.len() / CHARS_PER_TOKEN;
        assert!((11_000..11_100).contains(&tokens), "{tokens}");
        assert_eq!(p, padding(11_000));
        assert!(p.contains("not needed for any request"));
    }

    /// Records the system prompt it was sent, and answers at once.
    struct Seen(std::sync::Mutex<Vec<String>>);

    impl ModelPort for Seen {
        fn stream(
            &self,
            context: Context,
            _: &Tools,
            _: &Options,
        ) -> BoxStream<'static, Result<Event, LlmError>> {
            self.0.lock().unwrap().push(context.system);
            futures::stream::iter([Ok(Event::Text("Hi.".into()))]).boxed()
        }
        fn complete(
            &self,
            _: String,
            _: String,
            _: u32,
        ) -> BoxFuture<'_, Result<String, LlmError>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn model_name(&self) -> String {
            "seen".into()
        }
    }

    #[tokio::test]
    async fn the_first_byte_waits_and_the_prompt_is_padded() {
        let seen = Arc::new(Seen(Default::default()));
        let shaped = Shaped::wrap(seen.clone(), 100, Duration::from_millis(120));
        let ctx = Context {
            system: "You are Ghost.".into(),
            ..Context::default()
        };
        let began = Instant::now();
        let mut s = shaped.stream(ctx, &Tools::default(), &Options::default());
        assert!(
            seen.0.lock().unwrap().is_empty(),
            "nothing sent before the wait"
        );
        assert!(s.next().await.is_some());
        assert!(
            began.elapsed() >= Duration::from_millis(120),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(
            seen.0.lock().unwrap()[0],
            format!("You are Ghost.{}", padding(100))
        );
        // Nothing to shape: the model as it was.
        let plain = Shaped::wrap(seen.clone(), 0, Duration::ZERO);
        assert_eq!(
            Arc::as_ptr(&plain) as *const u8,
            Arc::as_ptr(&seen) as *const u8
        );
    }
}
