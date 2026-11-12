//! `ghostd llm test`: the provider check of the saved connection, run by the daemon that
//! holds the vault (ADR 0056). The password is read by the daemon at the point of use and
//! never comes through here; this prints what the check found, in plain words, or the
//! JSON with `--json`.

use std::io::Write;

use serde_json::Value;

pub const USAGE: &str = "usage: ghostd llm test [--json]";

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

/// `ghostd llm …`. 0 when the check ran and every model answered, 1 when one didn't,
/// 2 for a usage error or no daemon.
pub async fn llm_cli(
    args: &[String],
    client: &ghost_api::client::GhostClient,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    if args.first().map(String::as_str) != Some("test")
        || args.iter().skip(1).any(|a| a != "--json")
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
    if json {
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&check).unwrap_or_default()
        );
    } else {
        for line in report(&check) {
            let _ = writeln!(out, "{line}");
        }
    }
    let all = check["models"]
        .as_array()
        .is_some_and(|ms| !ms.is_empty() && ms.iter().all(|m| m["reachable"] == true));
    if all { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_report_reads_in_plain_words() {
        let check = json!({
            "where": "cliproxy (anthropic, http://proxy:8317)",
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
