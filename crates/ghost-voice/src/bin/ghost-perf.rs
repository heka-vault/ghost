//! `ghost-perf`: the latency harness (`make perf`), ADR 0018. See `ghost_voice::perf`.

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(ghost_voice::perf::cli::main(&args));
}
