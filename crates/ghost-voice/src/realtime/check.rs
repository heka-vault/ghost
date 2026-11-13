//! The realtime connection's check (ADR 0063, beside ADR 0056's provider check): reachable,
//! the time from asking for a line to its first audio, and whether a written request is
//! handed to the agent through `consult`. A fixed line and a fixed request, a few cents of
//! a cent; nothing the operator said, and no audio of theirs.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{CONSULT, Event, RealtimeProvider, setup};

pub const LINE: &str = "This is Ghost, checking its realtime voice.";
pub const REQUEST: &str = "What is on my calendar tomorrow morning?";
const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub provider: String,
    pub model: String,
    pub reachable: bool,
    pub error: Option<String>,
    pub connect_ms: Option<u64>,
    /// From asking for the line to its first audio.
    pub first_audio_ms: Option<u64>,
    /// Whether a written request was handed to the agent (`consult`); None: not tried.
    pub tools: Option<bool>,
    /// The check's own spend, estimated.
    pub usd: f64,
}

fn ms(d: Duration) -> u64 {
    (d.as_secs_f64() * 1000.0).round() as u64
}

pub async fn check(p: &dyn RealtimeProvider) -> CheckResult {
    let mut r = CheckResult {
        provider: p.name().to_string(),
        model: p.model().to_string(),
        ..CheckResult::default()
    };
    let began = Instant::now();
    let mut c = match tokio::time::timeout(WAIT, p.connect(&setup())).await {
        Ok(Ok(c)) => c,
        Ok(Err(f)) => {
            r.error = Some(f.to_string());
            return r;
        }
        Err(_) => {
            r.error = Some("unreachable: no answer in time".into());
            return r;
        }
    };
    r.reachable = true;
    r.connect_ms = Some(ms(began.elapsed()));
    let prices = p.prices();
    let mut spend = |u: &super::Usage| {
        if let Some(p) = &prices {
            r.usd += u.usd(p);
        }
    };
    // The line, said exactly: its first audio.
    let asked = Instant::now();
    c.link.say(LINE, "check");
    let deadline = Instant::now() + WAIT;
    let mut first_audio = None;
    let mut failed = None;
    while let Ok(Some(e)) = tokio::time::timeout_at(deadline.into(), c.events.recv()).await {
        match e {
            Event::Audio { .. } if first_audio.is_none() => first_audio = Some(asked.elapsed()),
            Event::Done { usage, .. } => {
                if let Some(u) = usage {
                    spend(&u);
                }
                break;
            }
            Event::Failed(f) => {
                failed = Some(f.to_string());
                break;
            }
            _ => {}
        }
    }
    r.first_audio_ms = first_audio.map(ms);
    if let Some(f) = failed {
        r.error = Some(f);
        c.link.close();
        return r;
    }
    // A request: handed to the agent.
    c.link.ask(REQUEST);
    let deadline = Instant::now() + WAIT;
    let mut consulted = false;
    while let Ok(Some(e)) = tokio::time::timeout_at(deadline.into(), c.events.recv()).await {
        match e {
            Event::Call { name, call_id, .. } => {
                consulted = name == CONSULT;
                c.link.answer(&call_id, super::stage::NOTHING);
            }
            Event::Done { usage, .. } => {
                if let Some(u) = usage {
                    spend(&u);
                }
                break;
            }
            Event::Failed(f) => {
                r.error = Some(f.to_string());
                break;
            }
            _ => {}
        }
    }
    r.tools = Some(consulted);
    c.link.close();
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime::fake::FakeRealtime;
    use crate::realtime::openai::OpenAiRealtime;
    use zeroize::Zeroizing;

    #[tokio::test]
    async fn the_check_times_the_first_audio_and_sees_a_request_handed_on() {
        let f = FakeRealtime::start("sk-fake").await;
        let p = OpenAiRealtime::new(
            f.url.clone(),
            Zeroizing::new("sk-fake".into()),
            "gpt-realtime-2.1-mini",
            "marin",
            500,
            Some(crate::realtime::settings::OPENAI.models[0].prices),
        );
        let r = check(&p).await;
        assert!(r.reachable, "{r:?}");
        assert!(r.first_audio_ms.is_some(), "{r:?}");
        assert_eq!(r.tools, Some(true), "{r:?}");
        assert!(r.usd > 0.0);
        let bad = OpenAiRealtime::new(
            f.url.clone(),
            Zeroizing::new("sk-no".into()),
            "m",
            "marin",
            500,
            None,
        );
        let r = check(&bad).await;
        assert!(!r.reachable);
        assert!(r.error.unwrap().starts_with("refused"));
    }
}
