//! `ghostd llm test`: the provider check of the saved connection, run by the daemon that
//! holds the vault (ADR 0056). The password is read by the daemon at the point of use and
//! never comes through here; this prints what the check found, in plain words, or the
//! JSON with `--json`.

use std::io::Write;

use serde_json::Value;

pub const USAGE: &str = "usage: ghostd llm test [--realtime] [--json]";

fn secs(ms: &Value) -> String {
    ms.as_f64()
        .map_or("-".to_string(), |ms| format!("{:.2} s", ms / 1000.0))
}

fn finding(v: &Value) -> String {
    let state = match v["state"].as_str().unwrap_or("") {
        "yes" => "yes",
        "no" => "no",
        "not_reported" => "not reported",
        "not_applicable" => "not applicable",
        _ => "not tried",
    };
    match v["detail"].as_str() {
        Some(d) if !d.is_empty() => format!("{state} ({d})"),
        _ => state.to_string(),
    }
}

/// The check, as lines for a terminal.
pub fn report(check: &Value) -> Vec<String> {
    let mut out = vec![format!(
        "Model connection: {}",
        check["where"].as_str().unwrap_or("?")
    )];
    for m in check["models"].as_array().into_iter().flatten() {
        out.push(String::new());
        let role = if m["role"] == "fast" {
            "fast model"
        } else {
            "main model"
        };
        out.push(format!("{role}: {}", m["model"].as_str().unwrap_or("?")));
        if m["reachable"] != true {
            out.push(format!(
                "  not reachable: {}",
                m["error"].as_str().unwrap_or("no answer")
            ));
            continue;
        }
        if let Some(by) = m["answered_by"].as_str() {
            out.push(format!("  answered by:   {by}"));
        }
        out.push(format!(
            "  first byte:    {} median, {} worst",
            secs(&m["first_byte"]["median_ms"]),
            secs(&m["first_byte"]["worst_ms"])
        ));
        out.push(format!(
            "  first word:    {} median, {} worst ({} calls)",
            secs(&m["first_word"]["median_ms"]),
            secs(&m["first_word"]["worst_ms"]),
            m["samples"]
        ));
        out.push(format!("  streaming:     {}", finding(&m["streaming"])));
        out.push(format!("  tool calls:    {}", finding(&m["tools"])));
        out.push(format!("  prompt cache:  {}", finding(&m["caching"])));
        out.push(format!("  thinking:      {}", finding(&m["thinking"])));
        if let Some(e) = m["error"].as_str() {
            out.push(format!("  also:          {e}"));
        }
    }
    if let Some(a) = check["advice"].as_str() {
        out.push(String::new());
        out.push(a.to_string());
    }
    out
}

/// The realtime voice's check (ADR 0063), as lines for a terminal.
pub fn realtime_report(check: &Value) -> Vec<String> {
    let mut out = vec![format!(
        "Realtime voice: {} {}",
        check["provider"].as_str().unwrap_or("?"),
        check["model"].as_str().unwrap_or("?")
    )];
    if check["reachable"] != true {
        out.push(format!(
            "  not reachable: {}",
            check["error"].as_str().unwrap_or("no answer")
        ));
        return out;
    }
    out.push(format!("  connected in:  {}", secs(&check["connect_ms"])));
    out.push(format!(
        "  first audio:   {}",
        secs(&check["first_audio_ms"])
    ));
    let tools = match check["tools"].as_bool() {
        Some(true) => "yes (a request was handed to Ghost's agent)",
        Some(false) => "no (the voice answered a request itself)",
        None => "not tried",
    };
    out.push(format!("  tool calls:    {tools}"));
    if let Some(usd) = check["usd"].as_f64() {
        out.push(format!("  this check:    about ${usd:.4}"));
    }
    if let Some(e) = check["error"].as_str() {
        out.push(format!("  also:          {e}"));
    }
    out
}

/// `ghostd llm …`. 0 when the check ran and every model answered, 1 when one didn't,
/// 2 for a usage error or no daemon. With `--realtime`, or when the realtime voice is
/// chosen, its connection is checked too (ADR 0063).
pub async fn llm_cli(
    args: &[String],
    client: &ghost_api::client::GhostClient,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    let mut realtime = args.iter().any(|a| a == "--realtime");
    if args.first().map(String::as_str) != Some("test")
        || args
            .iter()
            .skip(1)
            .any(|a| a != "--json" && a != "--realtime")
    {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    }
    let check = match client.check_model().await {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(err, "ghostd llm test: {e}");
            return 2;
        }
    };
    if !realtime && let Ok(view) = client.realtime().await {
        realtime = view["mode"] == "realtime";
    }
    let voice = if realtime {
        match client.check_realtime().await {
            Ok(v) => Some(v),
            Err(e) => {
                let _ = writeln!(err, "ghostd llm test: the realtime voice: {e}");
                return 2;
            }
        }
    } else {
        None
    };
    if json {
        let mut all = check.clone();
        if let Some(v) = &voice {
            all["realtime"] = v.clone();
        }
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&all).unwrap_or_default()
        );
    } else {
        for line in report(&check) {
            let _ = writeln!(out, "{line}");
        }
        if let Some(v) = &voice {
            let _ = writeln!(out);
            for line in realtime_report(v) {
                let _ = writeln!(out, "{line}");
            }
        }
    }
    let all = check["models"]
        .as_array()
        .is_some_and(|ms| !ms.is_empty() && ms.iter().all(|m| m["reachable"] == true))
        && voice.as_ref().is_none_or(|v| v["reachable"] == true);
    if all { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_realtime_check_reads_in_plain_words() {
        let lines = realtime_report(&json!({
            "provider": "openai", "model": "gpt-realtime-2.1-mini", "reachable": true,
            "connect_ms": 420, "first_audio_ms": 610, "tools": true, "usd": 0.0012
        }))
        .join("\n");
        assert!(lines.contains("first audio:   0.61 s"), "{lines}");
        assert!(lines.contains("handed to Ghost's agent"), "{lines}");
        let lines = realtime_report(
            &json!({"provider": "openai", "model": "m", "reachable": false,
            "error": "refused: HTTP 401"}),
        )
        .join("\n");
        assert!(
            lines.contains("not reachable: refused: HTTP 401"),
            "{lines}"
        );
    }

    #[test]
    fn a_report_reads_in_plain_words() {
        let check = json!({
            "where": "custom (anthropic, http://proxy:8080)",
            "models": [
                {"role": "main", "model": "claude-sonnet-5-5", "reachable": true,
                 "answered_by": "claude-sonnet-5-5", "samples": 3,
                 "first_byte": {"median_ms": 610, "worst_ms": 900},
                 "first_word": {"median_ms": 820, "worst_ms": 1300},
                 "streaming": {"state": "yes"}, "tools": {"state": "yes"},
                 "caching": {"state": "yes", "detail": "4100 tokens read from the cache"},
                 "thinking": {"state": "yes", "detail": "accepts between_tools"}},
                {"role": "fast", "model": "claude-haiku-5-5", "reachable": false,
                 "error": "The model provider doesn't offer that model. (HTTP 404)"}
            ],
            "advice": null
        });
        let lines = report(&check).join("\n");
        assert!(lines.contains("main model: claude-sonnet-5-5"), "{lines}");
        assert!(
            lines.contains("first word:    0.82 s median, 1.30 s worst (3 calls)"),
            "{lines}"
        );
        assert!(
            lines.contains("prompt cache:  yes (4100 tokens read from the cache)"),
            "{lines}"
        );
        assert!(lines.contains("fast model: claude-haiku-5-5"), "{lines}");
        assert!(
            lines.contains("not reachable: The model provider doesn't offer that model."),
            "{lines}"
        );
    }
}
