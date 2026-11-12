//! `ghostd perf providers` (`make perf-providers`): the provider check the product runs
//! (ADR 0055, `ghost_llm::check`), against every provider named on the command line that
//! the environment has a key for, written as a dated comparison under
//! `docs/benchmarks/perf/`.
//!
//! A provider is `name` or `name:VARIABLE`, the variable holding its key. The key is read
//! from that environment variable only: never from the operator's vault (the command
//! doesn't open it), never printed, never written; the report names the variable. A
//! provider whose variable is unset is skipped, saying which variable to set.
//!
//! - `proxy[:VAR]`: the subscription proxy at `ANTHROPIC_BASE_URL` (`CLIPROXY_BASE_URL`,
//!   then CLIProxy's own address), Anthropic's API; `VAR` defaults to
//!   `ANTHROPIC_AUTH_TOKEN` and may be unset (the proxy may take no key).
//! - `anthropic[:VAR]`: the API direct; `ANTHROPIC_API_KEY`.
//! - `openrouter[:VAR]`: OpenRouter's Anthropic API; `OPENROUTER_API_KEY`.
//! - `openrouter-openai[:VAR]`: OpenRouter's OpenAI-compatible API; `OPENROUTER_API_KEY`.
//! - `openai[:VAR]`: `OPENAI_API_KEY`.
//! - `ollama`, `lmstudio`: on this machine, no key; their models must be named
//!   (`--models`).
//!
//! Each provider's Claude models are checked as a session uses them: the main model with
//! the session's thinking setting on its timed calls, the fast model without.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use ghost_core::connection::{ModelConnection, Protocol, Target, openrouter_model};
use ghost_llm::check::{self, CheckOptions, ModelCheck, State, Verdict};
use ghost_llm::{DEFAULT_THINKING, Model};
use serde_json::json;

pub const USAGE: &str = "usage: ghostd perf providers PROVIDER[:KEY_VARIABLE]... [--models main,fast] \
                         [--samples N] [--out FILE.md]\n  providers: proxy anthropic openrouter \
                         openrouter-openai openai ollama lmstudio";

const OPENROUTER_OPENAI: &str = "https://openrouter.ai/api/v1";
const CLAUDE: [(&str, &str); 2] = [("claude-sonnet-5-5", "main"), ("claude-haiku-5-5", "fast")];
const GPT: [(&str, &str); 2] = [("gpt-5.1", "main"), ("gpt-5-mini", "fast")];

/// One provider as named on the command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    /// The variable its key is read from; None for one that takes none.
    pub key_var: Option<String>,
    /// The key may be missing (a proxy, a local server).
    pub key_optional: bool,
}

/// `proxy`, `anthropic:MY_KEY`, ...
pub fn parse_named(spec: &str) -> Result<Named, String> {
    let (name, var) = match spec.split_once(':') {
        Some((n, v)) => (n, Some(v.to_string())),
        None => (spec, None),
    };
    let (default, optional) = match name {
        "proxy" => (Some("ANTHROPIC_AUTH_TOKEN"), true),
        "anthropic" => (Some("ANTHROPIC_API_KEY"), false),
        "openrouter" | "openrouter-openai" => (Some("OPENROUTER_API_KEY"), false),
        "openai" => (Some("OPENAI_API_KEY"), false),
        "ollama" | "lmstudio" => (None, true),
        other => return Err(format!("unknown provider {other:?}\n{USAGE}")),
    };
    let key_var = var.or_else(|| default.map(String::from));
    if let Some(v) = &key_var
        && !ghost_core::connection::is_password_name(v)
    {
        return Err(format!("{v:?} is not a variable name like MY_MODEL_KEY"));
    }
    Ok(Named {
        name: name.into(),
        key_var,
        key_optional: optional,
    })
}

/// The connection settings a named provider stands for, built from its own provider so
/// nothing of the operator's configured connection leaks in.
fn settings(named: &Named, env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    let mut put = |k: &str, v: &str| {
        out.insert(k.into(), v.into());
    };
    match named.name.as_str() {
        "proxy" => {
            put("GHOST_LLM_PROVIDER", "cliproxy");
            if let Some(b) = env
                .get("ANTHROPIC_BASE_URL")
                .filter(|b| !b.trim().is_empty())
            {
                put("GHOST_LLM_BASE_URL", b);
            } else if let Some(b) = env
                .get("CLIPROXY_BASE_URL")
                .filter(|b| !b.trim().is_empty())
            {
                put("CLIPROXY_BASE_URL", b);
            }
        }
        "openrouter-openai" => {
            put("GHOST_LLM_PROVIDER", "custom");
            put("GHOST_LLM_PROTOCOL", "openai");
            put("GHOST_LLM_BASE_URL", OPENROUTER_OPENAI);
        }
        other => put("GHOST_LLM_PROVIDER", other),
    }
    if let Some(var) = &named.key_var {
        put("GHOST_LLM_PASSWORD", var);
        if let Some(v) = env.get(var) {
            put(var, v);
        }
    }
    out
}

/// The models to check on a provider: `--models`, or its defaults.
fn models(named: &Named, conn: &ModelConnection, wanted: &[String]) -> Vec<(String, String)> {
    let roles = ["main", "fast"];
    let given: Vec<(String, String)> = wanted
        .iter()
        .enumerate()
        .map(|(i, m)| (m.clone(), roles.get(i).unwrap_or(&"main").to_string()))
        .collect();
    let pairs: Vec<(String, String)> = if !given.is_empty() {
        given
    } else if conn.protocol == Protocol::Anthropic || named.name == "openrouter-openai" {
        CLAUDE
            .iter()
            .map(|(m, r)| (m.to_string(), r.to_string()))
            .collect()
    } else if named.name == "openai" {
        GPT.iter()
            .map(|(m, r)| (m.to_string(), r.to_string()))
            .collect()
    } else {
        Vec::new()
    };
    pairs
        .into_iter()
        .map(|(m, r)| {
            if named.name == "openrouter-openai" {
                (openrouter_model(&m), r)
            } else {
                (m, r)
            }
        })
        .collect()
}

/// Where a provider was reached, for a report that may be committed: a public
/// provider's address, but a local one only by where its address came from (a private
/// address stays on the machine).
fn shown_address(named: &Named, target: &Target) -> String {
    match named.name.as_str() {
        "proxy" => "ANTHROPIC_BASE_URL (on the operator's network)".into(),
        "ollama" | "lmstudio" => "this machine".into(),
        _ => ghost_llm::capabilities::address(&target.base_url),
    }
}

/// One row of the comparison.
#[derive(Clone, Debug)]
pub struct Row {
    pub provider: String,
    pub key_var: Option<String>,
    pub address: String,
    pub check: ModelCheck,
}

fn secs(x: Option<f64>) -> String {
    x.map_or("-".into(), |s| format!("{s:.2}"))
}

fn mark(v: &Verdict) -> &'static str {
    match v.state {
        State::Yes => "yes",
        State::No => "no",
        State::NotReported => "not reported",
        State::NotApplicable => "n/a",
        State::Untested => "-",
    }
}

/// The comparison as markdown.
pub fn markdown(rows: &[Row], skipped: &[String], samples: usize, date: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Model providers compared, {date}\n");
    let _ = writeln!(
        s,
        "`make perf-providers`: the provider check Ghost runs from Settings › Model and \
         `ghostd llm test` (ADR 0055), with the same fixed synthetic prompts, against each \
         provider the environment had a key for. {samples} timed calls per model; first \
         byte and first word are seconds from the call, median / worst. Keys came from the \
         variables named, never from Ghost's vault, and are not in this file.\n"
    );
    let _ = writeln!(
        s,
        "| provider | key from | model | role | first byte | first word | streams | tools | cache | thinking |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|");
    for r in rows {
        let c = &r.check;
        let key = r.key_var.as_deref().unwrap_or("none");
        if !c.reachable {
            let _ = writeln!(
                s,
                "| {} | {key} | {} | {} | not reachable | | | | | |",
                r.provider, c.model, c.role
            );
            continue;
        }
        let _ = writeln!(
            s,
            "| {} | {key} | {} | {} | {} / {} | {} / {} | {} | {} | {} | {} |",
            r.provider,
            c.model,
            c.role,
            secs(c.first_byte.median),
            secs(c.first_byte.worst),
            secs(c.first_word.median),
            secs(c.first_word.worst),
            mark(&c.streaming),
            mark(&c.tools),
            mark(&c.caching),
            mark(&c.thinking),
        );
    }
    let notes: Vec<String> = rows
        .iter()
        .flat_map(|r| {
            let c = &r.check;
            let mut out = Vec::new();
            if let Some(e) = &c.error {
                out.push(format!("- {} {}: {e}", r.provider, c.model));
            }
            for (what, v) in [
                ("streams", &c.streaming),
                ("tools", &c.tools),
                ("cache", &c.caching),
                ("thinking", &c.thinking),
            ] {
                if let Some(d) = &v.detail {
                    out.push(format!("- {} {} {what}: {d}", r.provider, c.model));
                }
            }
            out
        })
        .collect();
    if !notes.is_empty() {
        let _ = writeln!(s, "\n## What it found\n");
        for n in notes {
            let _ = writeln!(s, "{n}");
        }
    }
    if !skipped.is_empty() {
        let _ = writeln!(s, "\n## Not measured\n");
        for n in skipped {
            let _ = writeln!(s, "- {n}");
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for r in rows {
        if !seen.contains(&r.provider.as_str()) {
            if seen.is_empty() {
                let _ = writeln!(s, "\n## Addresses\n");
            }
            seen.push(&r.provider);
            let _ = writeln!(s, "- {}: {}", r.provider, r.address);
        }
    }
    s
}

/// Run the comparison. `env` is the process environment; nothing else is read.
pub async fn run(
    named: &[Named],
    wanted: &[String],
    samples: usize,
    env: &HashMap<String, String>,
    log: &mut dyn std::io::Write,
) -> (Vec<Row>, Vec<String>) {
    let mut rows = Vec::new();
    let mut skipped = Vec::new();
    for n in named {
        let set = settings(n, env);
        let has_key = n
            .key_var
            .as_ref()
            .is_some_and(|v| env.get(v).is_some_and(|k| !k.is_empty()));
        if !has_key && !n.key_optional {
            let var = n.key_var.as_deref().unwrap_or("?");
            let _ = writeln!(log, "  skipping {}: set {var} to measure it", n.name);
            skipped.push(format!("{}: set {var} to measure it", n.name));
            continue;
        }
        let conn = ModelConnection::from_env(&set);
        let list = models(n, &conn, wanted);
        if list.is_empty() {
            skipped.push(format!("{}: name its models with --models", n.name));
            continue;
        }
        for (model, role) in list {
            let mut c = conn.clone();
            c.model = model.clone();
            let target: Target = c.target(&set);
            let opts = if role == "fast" {
                CheckOptions::fast(Some(DEFAULT_THINKING.into()))
            } else {
                CheckOptions::main(Some(DEFAULT_THINKING.into()))
            };
            let opts = CheckOptions { samples, ..opts };
            let _ = writeln!(log, "  {} {} ({role})", n.name, target.model);
            let found = match Model::new(target.clone()) {
                Ok(m) => check::check(&m, &opts).await,
                Err(e) => ModelCheck::failed(&role, &target.model, e.to_string()),
            };
            rows.push(Row {
                provider: n.name.clone(),
                key_var: n.key_var.clone().filter(|_| has_key),
                address: shown_address(n, &target),
                check: found,
            });
        }
    }
    (rows, skipped)
}

/// `ghostd perf providers …`.
pub fn cli(argv: &[String], root: &std::path::Path) -> i32 {
    let mut named = Vec::new();
    let mut wanted: Vec<String> = Vec::new();
    let mut samples = 5usize;
    let mut out: Option<PathBuf> = None;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--models" => {
                wanted = it
                    .next()
                    .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
                    .unwrap_or_default()
            }
            "--samples" => samples = it.next().and_then(|v| v.parse().ok()).unwrap_or(samples),
            "--out" => out = it.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            spec => match parse_named(spec) {
                Ok(n) => named.push(n),
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            },
        }
    }
    if named.is_empty() {
        eprintln!("{USAGE}");
        return 2;
    }
    let env: HashMap<String, String> = std::env::vars().collect();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    println!("perf providers: {samples} timed calls per model");
    let (rows, skipped) = rt.block_on(run(&named, &wanted, samples, &env, &mut std::io::stdout()));
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let md = markdown(&rows, &skipped, samples, &date);
    let path = out.unwrap_or_else(|| {
        root.join("docs/benchmarks/perf")
            .join(format!("{date}-providers.md"))
    });
    let json_path = path.with_extension("json");
    let data = json!({
        "date": date,
        "samples": samples,
        "rows": rows.iter().map(|r| json!({
            "provider": r.provider, "key_from": r.key_var, "address": r.address,
            "check": r.check,
        })).collect::<Vec<_>>(),
        "skipped": skipped,
    });
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, &md).and_then(|_| {
        std::fs::write(
            &json_path,
            serde_json::to_string_pretty(&data).unwrap_or_default() + "\n",
        )
    }) {
        eprintln!("{}: {e}", path.display());
        return 2;
    }
    print!("\n{md}");
    println!("\nwritten: {}", path.display());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_names_where_its_key_comes_from() {
        let p = parse_named("proxy").unwrap();
        assert_eq!(p.key_var.as_deref(), Some("ANTHROPIC_AUTH_TOKEN"));
        assert!(p.key_optional);
        let a = parse_named("anthropic:MY_KEY").unwrap();
        assert_eq!(a.key_var.as_deref(), Some("MY_KEY"));
        assert!(!a.key_optional);
        assert!(parse_named("ollama").unwrap().key_var.is_none());
        assert!(parse_named("bogus").is_err());
        assert!(parse_named("anthropic:not a name").is_err());
    }

    #[test]
    fn a_provider_without_its_key_is_skipped_and_says_which_variable() {
        let env: HashMap<String, String> = HashMap::new();
        let named = [parse_named("openrouter").unwrap()];
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut log = Vec::new();
        let (rows, skipped) = rt.block_on(run(&named, &[], 1, &env, &mut log));
        assert!(rows.is_empty());
        assert_eq!(
            skipped,
            vec!["openrouter: set OPENROUTER_API_KEY to measure it"]
        );
    }

    #[test]
    fn the_connection_is_the_providers_own_and_the_key_only_by_name() {
        let env: HashMap<String, String> = [
            ("ANTHROPIC_BASE_URL", "http://proxy.lan:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "sk-proxy-0123456789"),
            ("GHOST_LLM_PROVIDER", "openai"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let n = parse_named("proxy").unwrap();
        let set = settings(&n, &env);
        let conn = ModelConnection::from_env(&set);
        assert_eq!(conn.provider, "cliproxy");
        assert_eq!(conn.base_url, "http://proxy.lan:8317");
        assert_eq!(&*conn.target(&set).api_key, "sk-proxy-0123456789");
        let or = parse_named("openrouter-openai").unwrap();
        let conn = ModelConnection::from_env(&settings(&or, &env));
        assert_eq!(conn.protocol, Protocol::Openai);
        let list = models(&or, &conn, &[]);
        assert_eq!(list[0].0, "anthropic/claude-sonnet-5.5");
    }

    #[test]
    fn the_report_names_variables_never_values() {
        let row = Row {
            provider: "proxy".into(),
            key_var: Some("ANTHROPIC_AUTH_TOKEN".into()),
            address: "http://proxy.lan:8317".into(),
            check: ModelCheck::failed("main", "claude-sonnet-5-5", "nothing answered"),
        };
        let md = markdown(
            &[row],
            &["openrouter: set OPENROUTER_API_KEY to measure it".into()],
            5,
            "2026-11-12",
        );
        assert!(
            md.contains(
                "| proxy | ANTHROPIC_AUTH_TOKEN | claude-sonnet-5-5 | main | not reachable"
            )
        );
        assert!(md.contains("set OPENROUTER_API_KEY"));
    }
}
