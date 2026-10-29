//! `rtc-probe`: answers one offer over plain HTTP and runs the measurement session
//! (`ghost_rtc::probe`). No signing here (the daemon's phone listener signs; this is for
//! `scripts/rtc/measure.py` and for one hand-run session with the real phone on the LAN or
//! the tunnel, once the listener in `ghostd` can hand the offer over). Never binds the real
//! phone listener's addresses by itself: you pass `--listen`.
//!
//! ```bash
//! cargo run --release -p ghost-rtc --features str0m --example rtc-probe -- --listen 127.0.0.1:0
//! ```
//!
//! Routes, the protocol's shapes without the signatures: `POST /offer`
//! `{sdp, type, pc_id?, restart_pc?}` → `{sdp, type: "answer", pc_id}`; `PATCH /offer`
//! `{pc_id, candidates: [{candidate, sdp_mid, sdp_mline_index}]}` → `{ok}`; `POST /end`.
//! Options: `--echo` (the inbound audio sent back, for a phone in hand), `--silent`,
//! `--seconds N` (end after N; default until the peer leaves), `--report FILE` (the
//! session's JSON, with this process's CPU time), `--bind IP` (the media socket; the
//! listen address's IP by default).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use ghost_rtc::probe::{self, Audio};
use ghost_rtc::{Media, MediaConfig, MediaError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;
use tracing::info;

struct Args {
    listen: SocketAddr,
    bind: Option<IpAddr>,
    audio: Audio,
    seconds: Option<f64>,
    report: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        listen: "127.0.0.1:0".parse().unwrap(),
        bind: None,
        audio: Audio::Markers,
        seconds: None,
        report: None,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(a) = argv.next() {
        match a.as_str() {
            "--listen" => {
                args.listen = argv
                    .next()
                    .expect("--listen HOST:PORT")
                    .parse()
                    .expect("an address")
            }
            "--bind" => args.bind = Some(argv.next().expect("--bind IP").parse().expect("an IP")),
            "--echo" => args.audio = Audio::Echo,
            "--silent" => args.audio = Audio::Silent,
            "--seconds" => {
                args.seconds = Some(argv.next().expect("--seconds N").parse().expect("a number"))
            }
            "--report" => args.report = argv.next(),
            "--help" | "-h" => {
                eprintln!(
                    "rtc-probe --listen HOST:PORT [--bind IP] [--echo|--silent] [--seconds N] [--report FILE]"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    args
}

#[derive(Deserialize)]
struct OfferRequest {
    sdp: String,
    #[serde(rename = "type")]
    kind: String,
    pc_id: Option<String>,
    restart_pc: Option<bool>,
}

#[derive(Deserialize)]
struct CandidateInit {
    candidate: String,
}

#[derive(Deserialize)]
struct PatchRequest {
    pc_id: String,
    candidates: Vec<CandidateInit>,
}

#[derive(Serialize)]
struct Answered {
    at: f64,
    restart: bool,
    fingerprint: String,
    took_ms: f64,
}

struct App {
    media: Arc<dyn Media>,
    pc_id: String,
    stop: watch::Sender<bool>,
    answers: std::sync::Mutex<Vec<Answered>>,
}

fn error(status: StatusCode, code: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": code})))
}

async fn offer(
    State(app): State<Arc<App>>,
    Json(req): Json<OfferRequest>,
) -> (StatusCode, Json<Value>) {
    if req.kind != "offer" {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "bad_request");
    }
    if let Some(id) = &req.pc_id
        && *id != app.pc_id
    {
        return error(StatusCode::NOT_FOUND, "not_found");
    }
    let restart = req.restart_pc.unwrap_or(false);
    let started = Instant::now();
    match app.media.answer(&req.sdp, restart).await {
        Ok(sdp) => {
            let fingerprint = app
                .media
                .fingerprint()
                .map(|f| f.to_string())
                .unwrap_or_default();
            info!("probe: answered (restart {restart}); fingerprint {fingerprint}");
            app.answers.lock().unwrap().push(Answered {
                at: probe::unix_now(),
                restart,
                fingerprint,
                took_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
            (
                StatusCode::OK,
                Json(json!({"sdp": sdp, "type": "answer", "pc_id": app.pc_id})),
            )
        }
        Err(MediaError::BadOffer(why)) => {
            info!("probe: offer rejected: {why}");
            error(StatusCode::BAD_REQUEST, "bad_offer")
        }
        Err(e) => {
            info!("probe: offer failed: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

async fn candidates(
    State(app): State<Arc<App>>,
    Json(req): Json<PatchRequest>,
) -> (StatusCode, Json<Value>) {
    if req.pc_id != app.pc_id {
        return error(StatusCode::NOT_FOUND, "not_found");
    }
    for c in req.candidates {
        if app.media.add_remote_candidate(&c.candidate).await.is_err() {
            return error(StatusCode::BAD_REQUEST, "bad_candidate");
        }
    }
    (StatusCode::OK, Json(json!({"ok": true})))
}

async fn end(State(app): State<Arc<App>>) -> Json<Value> {
    let _ = app.stop.send(true);
    Json(json!({"ok": true}))
}

/// This process's CPU time so far, every thread: user plus system from `/proc/self/stat`,
/// in clock ticks (100 a second on Linux).
fn cpu_seconds() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after_comm = text.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / 100.0)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = parse_args();
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let addr = listener.local_addr()?;
    let config = MediaConfig {
        bind: args.bind.unwrap_or(addr.ip()),
        ..MediaConfig::default()
    };
    let media: Arc<dyn Media> = Arc::from(ghost_rtc::new_media(config)?);
    let (stop_tx, stop_rx) = watch::channel(false);
    let app = Arc::new(App {
        media: media.clone(),
        pc_id: format!("probe-{}", std::process::id()),
        stop: stop_tx.clone(),
        answers: std::sync::Mutex::new(Vec::new()),
    });
    let router = Router::new()
        .route("/offer", post(offer).patch(candidates))
        .route("/end", post(end))
        .with_state(app.clone());
    println!(
        "rtc-probe: {} listening on http://{addr} (POST /offer)",
        ghost_rtc::BACKEND
    );
    use std::io::Write;
    std::io::stdout().flush()?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("rtc-probe: http: {e}");
        }
    });
    let wall = Instant::now();
    let cpu0 = cpu_seconds().unwrap_or(0.0);

    // The stats line every 5 s.
    let ticker = {
        let media = media.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_secs(5));
            ticks.tick().await;
            loop {
                ticks.tick().await;
                let s = media.stats();
                info!(
                    "probe: in {} pkts, lost {}, concealed {}, jitter {:.2} ms, out {} pkts, dropped frames {}, path {}",
                    s.packets_received,
                    s.packets_lost,
                    s.concealed,
                    s.jitter_ms,
                    s.packets_sent,
                    s.frames_dropped,
                    s.path.as_deref().unwrap_or("-")
                );
            }
        })
    };
    if let Some(seconds) = args.seconds {
        let stop = stop_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
            let _ = stop.send(true);
        });
    }
    let report = probe::run(media.clone(), args.audio, stop_rx).await;
    ticker.abort();
    media.close().await;
    let cpu = cpu_seconds().unwrap_or(0.0) - cpu0;
    let wall = wall.elapsed().as_secs_f64();
    let mut out = serde_json::to_value(&report)?;
    out["answers"] = serde_json::to_value(&*app.answers.lock().unwrap())?;
    out["cpu_seconds"] = json!(cpu);
    out["wall_seconds"] = json!(wall);
    out["cpu_percent"] = json!(if wall > 0.0 { cpu / wall * 100.0 } else { 0.0 });
    info!(
        "probe: done: {} bursts sent, {} heard, client-ready x{}, cpu {:.1}% of one core over {:.0} s",
        report.sent.len(),
        report.received.len(),
        report.client_ready.len(),
        cpu / wall.max(0.001) * 100.0,
        wall
    );
    match args.report {
        Some(path) => std::fs::write(path, serde_json::to_string_pretty(&out)?)?,
        None => println!("{out}"),
    }
    Ok(())
}
