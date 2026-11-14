//! What Claude Code writes on its standard output in `--output-format stream-json`, read
//! into what a turn needs, and what Ghost writes on its standard input.
//!
//! Every line is the program's JSON; Ghost keeps the shape of an event and the stream's
//! text, and never a line whole. An error is kept by kind: the words the program wrote
//! about it are not read, logged or reported (ADR 0067), only which kind it named.

use serde_json::{Value, json};

use crate::model::{LlmError, ProgramFailure, Usage};

/// The prefix the program gives the tools of the app named `ghost` in `--mcp-config`.
pub const TOOL_PREFIX: &str = "mcp__ghost__";

/// One line of the program's output, as a turn reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum Wire {
    /// The answer to the `initialize` control request: the program is ready.
    Ready {
        models: Vec<(String, String)>,
    },
    /// A model reply began (`message_start`), with its input's count.
    MessageStart(Usage),
    /// A text or thinking block began: nothing to do.
    BlockStart,
    /// A tool call block began; the name as the model wrote it, prefix and all.
    ToolStarting(String),
    Text(String),
    Thought(String),
    /// `message_delta`: why the reply stopped, and its output count.
    MessageDelta {
        stop: Option<String>,
        output: u64,
    },
    /// `message_stop`.
    MessageStop,
    /// The program will try the request again: what the provider said, by status.
    Retry {
        status: Option<u16>,
        kind: String,
        attempt: u32,
        delay_ms: u64,
    },
    /// The plan's limit, as the program reports it ahead of an error.
    LimitReached,
    /// A reply the program made up to say an error (`model: "<synthetic>"`), by kind.
    SyntheticError(String),
    /// The turn ended (`result`).
    Result {
        error: bool,
        status: Option<u16>,
        usage: Option<Usage>,
    },
    /// Anything else: the init and status notes, the echoed tool results, a control
    /// answer to an interrupt.
    Other,
}

fn usage_of(u: &Value) -> Usage {
    let n = |k: &str| u[k].as_u64();
    Usage {
        input_tokens: n("input_tokens").unwrap_or(0),
        output_tokens: n("output_tokens").unwrap_or(0),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_write_tokens: n("cache_creation_input_tokens"),
        reasoning_tokens: u["output_tokens_details"]["thinking_tokens"].as_u64(),
    }
}

/// Read one line.
pub fn read(line: &Value) -> Wire {
    match line["type"].as_str().unwrap_or("") {
        "control_response" => {
            let r = &line["response"];
            if r["request_id"] == INIT_ID {
                let models = r["response"]["models"]
                    .as_array()
                    .map(|ms| {
                        ms.iter()
                            .filter_map(|m| {
                                let v = m["value"].as_str()?;
                                let name = m["displayName"].as_str().unwrap_or(v);
                                Some((v.to_string(), name.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return Wire::Ready { models };
            }
            Wire::Other
        }
        "stream_event" => {
            let e = &line["event"];
            match e["type"].as_str().unwrap_or("") {
                "message_start" => Wire::MessageStart(usage_of(&e["message"]["usage"])),
                "content_block_start" => {
                    let b = &e["content_block"];
                    if b["type"] == "tool_use" {
                        Wire::ToolStarting(b["name"].as_str().unwrap_or("").to_string())
                    } else {
                        Wire::BlockStart
                    }
                }
                "content_block_delta" => {
                    let d = &e["delta"];
                    match d["type"].as_str().unwrap_or("") {
                        "text_delta" => Wire::Text(d["text"].as_str().unwrap_or("").into()),
                        "thinking_delta" => {
                            Wire::Thought(d["thinking"].as_str().unwrap_or("").into())
                        }
                        _ => Wire::Other,
                    }
                }
                "message_delta" => Wire::MessageDelta {
                    stop: e["delta"]["stop_reason"].as_str().map(str::to_string),
                    output: e["usage"]["output_tokens"].as_u64().unwrap_or(0),
                },
                "message_stop" => Wire::MessageStop,
                _ => Wire::Other,
            }
        }
        "assistant" => {
            let m = &line["message"];
            match line["error"].as_str() {
                Some(kind) => Wire::SyntheticError(kind.to_string()),
                None if m["model"] == "<synthetic>" => Wire::SyntheticError("unknown".into()),
                None => Wire::Other,
            }
        }
        "system" if line["subtype"] == "api_retry" => Wire::Retry {
            status: line["error_status"].as_u64().map(|s| s as u16),
            kind: line["error"].as_str().unwrap_or("").to_string(),
            attempt: line["attempt"].as_u64().unwrap_or(1) as u32,
            delay_ms: line["retry_delay_ms"].as_u64().unwrap_or(0),
        },
        "rate_limit_event" if line["rate_limit_info"]["status"] == "rejected" => Wire::LimitReached,
        "result" => Wire::Result {
            error: line["is_error"].as_bool().unwrap_or(false),
            status: line["api_error_status"].as_u64().map(|s| s as u16),
            usage: line.get("usage").map(usage_of),
        },
        _ => Wire::Other,
    }
}

/// The error a kind the program named stands for. A refused credential is the sign-in;
/// a rate limit on a plan is the plan's limit; the rest by status where there is one.
pub fn error_of(kind: &str, status: Option<u16>) -> LlmError {
    let k = kind.to_lowercase();
    if k.contains("authentication") || matches!(status, Some(401 | 403)) {
        return LlmError::Program(ProgramFailure::SignedOut);
    }
    if k.contains("rate_limit") || k.contains("billing") || k.contains("usage_limit") {
        return LlmError::Program(ProgramFailure::Limit);
    }
    match status {
        Some(s) if s >= 400 => LlmError::Status {
            status: s,
            detail: String::new(),
        },
        _ => LlmError::Program(ProgramFailure::Failed),
    }
}

/// What to do with a retry the program announced: wait for it (one try within two
/// seconds, as ADR 0056 waits on a provider), or stop the turn with this error.
pub fn retry_verdict(
    status: Option<u16>,
    kind: &str,
    attempt: u32,
    delay_ms: u64,
) -> Option<LlmError> {
    let k = kind.to_lowercase();
    if k.contains("authentication") || matches!(status, Some(401 | 403)) {
        return Some(LlmError::Program(ProgramFailure::SignedOut));
    }
    if attempt <= 1 && delay_ms <= crate::model::MOST_WAIT.as_millis() as u64 {
        return None;
    }
    Some(match status {
        Some(s) => LlmError::Status {
            status: s,
            detail: String::new(),
        },
        None if k.contains("rate_limit") => LlmError::Status {
            status: 429,
            detail: String::new(),
        },
        None => LlmError::Program(ProgramFailure::Failed),
    })
}

/// The `initialize` control request's id: its answer says the program is ready.
pub const INIT_ID: &str = "ghost-init";

/// The `initialize` control request: the program connects its apps (Ghost's relay) and
/// answers with its models; no model is asked.
pub fn initialize() -> String {
    json!({"type": "control_request", "request_id": INIT_ID, "request": {"subtype": "initialize"}})
        .to_string()
}

/// Stop the reply in progress (the Agent SDK's `interrupt()`): the program ends the turn
/// with a `result` and keeps running.
pub fn interrupt(n: u64) -> String {
    json!({"type": "control_request", "request_id": format!("ghost-interrupt-{n}"),
           "request": {"subtype": "interrupt"}})
    .to_string()
}

/// One turn of the operator's, as the program takes it on standard input.
pub fn user(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text},
           "parent_tool_use_id": null, "session_id": ""})
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<Value> {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn a_recorded_text_turn_reads_as_text_then_its_end() {
        let got: Vec<Wire> = lines(include_str!("../../tests/fixtures/claude-code/text.jsonl"))
            .iter()
            .map(read)
            .filter(|w| *w != Wire::Other && *w != Wire::BlockStart)
            .collect();
        assert!(
            matches!(got[0], Wire::Ready { ref models } if models.iter().any(|(v, _)| v == "haiku"))
        );
        assert!(matches!(got[1], Wire::MessageStart(_)));
        let text: String = got
            .iter()
            .filter_map(|w| match w {
                Wire::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello there, friendly test assistant!");
        assert!(got.contains(&Wire::MessageStop));
        assert!(matches!(
            got.last(),
            Some(Wire::Result { error: false, .. })
        ));
    }

    #[test]
    fn a_recorded_tool_turn_names_the_tool_with_its_prefix() {
        let got: Vec<Wire> = lines(include_str!("../../tests/fixtures/claude-code/tool.jsonl"))
            .iter()
            .map(read)
            .collect();
        assert!(got.contains(&Wire::ToolStarting("mcp__ghost__lookup".into())));
        assert!(got.contains(&Wire::MessageDelta {
            stop: Some("tool_use".into()),
            output: 57
        }));
    }

    #[test]
    fn a_signed_out_program_reads_as_its_sign_in_never_its_words() {
        let got: Vec<Wire> = lines(include_str!(
            "../../tests/fixtures/claude-code/signed-out.jsonl"
        ))
        .iter()
        .map(read)
        .collect();
        let kind = got
            .iter()
            .find_map(|w| match w {
                Wire::SyntheticError(k) => Some(k.clone()),
                _ => None,
            })
            .expect("the synthetic reply");
        assert_eq!(kind, "authentication_failed");
        let e = error_of(&kind, None);
        assert_eq!(e, LlmError::Program(ProgramFailure::SignedOut));
        assert!(!e.to_string().contains("OAuth"));
        assert!(matches!(got.last(), Some(Wire::Result { error: true, .. })));
    }

    #[test]
    fn a_refused_credential_stops_at_the_first_retry_and_a_short_wait_is_waited() {
        let first: Value = lines(include_str!(
            "../../tests/fixtures/claude-code/retry-401.jsonl"
        ))
        .into_iter()
        .find(|l| l["subtype"] == "api_retry")
        .unwrap();
        let Wire::Retry {
            status,
            kind,
            attempt,
            delay_ms,
        } = read(&first)
        else {
            panic!("a retry");
        };
        assert_eq!(
            retry_verdict(status, &kind, attempt, delay_ms),
            Some(LlmError::Program(ProgramFailure::SignedOut))
        );
        // Overloaded, a first short wait: the program's own retry is the one more try.
        assert_eq!(retry_verdict(Some(529), "overloaded", 1, 500), None);
        // A second, or a long one: the turn says so instead.
        assert_eq!(
            retry_verdict(Some(529), "overloaded", 2, 1000),
            Some(LlmError::Status {
                status: 529,
                detail: String::new()
            })
        );
        assert_eq!(
            retry_verdict(Some(429), "rate_limit", 1, 30_000),
            Some(LlmError::Status {
                status: 429,
                detail: String::new()
            })
        );
    }

    #[test]
    fn a_plans_limit_is_said_as_the_plans_limit() {
        // Built from the Agent SDK's documented `SDKAssistantMessageError` kinds; not
        // recorded (the workstation's plan was not at its limit).
        assert_eq!(
            error_of("rate_limit", Some(429)),
            LlmError::Program(ProgramFailure::Limit)
        );
        assert_eq!(
            read(&json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected"}})),
            Wire::LimitReached
        );
        assert_eq!(
            read(&json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}})),
            Wire::Other
        );
    }

    #[test]
    fn what_ghost_writes_is_one_json_line_each() {
        for l in [initialize(), interrupt(3), user("hi\nthere")] {
            assert!(!l.contains('\n'));
            let v: Value = serde_json::from_str(&l).unwrap();
            assert!(v["type"].is_string());
        }
        let v: Value = serde_json::from_str(&interrupt(3)).unwrap();
        assert_eq!(v["request"]["subtype"], "interrupt");
    }
}
