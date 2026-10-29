//! The measurement session (`README.md`, `scripts/rtc/measure.py`): what the `rtc-probe`
//! example and the aiortc integration tests run on top of a [`Media`] once the offer is
//! answered. Markers go out through the sink at real-time pace, the bursts coming in are
//! timed, the RTVI handshake is answered (`client-ready` gets `bot-ready` and one
//! `server-message`), and `disconnect-bot` ends the session. Not on the daemon's path.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::watch;
use tracing::info;

use crate::frame::{IN_RATE, OUT_RATE, frame_samples};
use crate::marker::{MarkerTone, OnsetDetector};
use crate::media::{CallStats, Link, Media};
use crate::resample::Resampler;
use crate::rtvi::{ClientMessage, Outbound, bot_ready_version};

/// What to do with the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audio {
    /// The marker tone out; the inbound bursts timed.
    Markers,
    /// The inbound audio sent back out (what the operator hears on the real phone), and
    /// the inbound bursts timed.
    Echo,
    /// Silence out.
    Silent,
}

/// What a session saw, as `measure.py` reads it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub backend: &'static str,
    /// Unix seconds when a burst's first sample reached the sink (the desk's send times).
    pub sent: Vec<f64>,
    /// Unix seconds when a burst came out of the decoder (the desk's receive times).
    pub received: Vec<f64>,
    pub client_ready: Vec<f64>,
    pub disconnect_bot: Vec<f64>,
    pub bot_ready_sent: u32,
    /// `(unix seconds, state)` for every link change.
    pub links: Vec<(f64, String)>,
    pub stats: CallStats,
    pub frames: u64,
}

/// Unix seconds now.
pub fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Unix seconds of an `Instant` already past.
pub fn unix_at(at: Instant) -> f64 {
    unix_now() - at.elapsed().as_secs_f64()
}

/// How long a `Gone` link may last before the session ends: a phone that restarts its
/// peer connection (`restart_pc`) closes the old one first.
pub const GONE_GRACE: Duration = Duration::from_secs(5);

/// Run until `stop` says so, the peer has been gone for [`GONE_GRACE`], or the phone says
/// `disconnect-bot`.
pub async fn run(media: Arc<dyn Media>, audio: Audio, mut stop: watch::Receiver<bool>) -> Report {
    let report = Arc::new(Mutex::new(Report {
        backend: crate::BACKEND,
        ..Report::default()
    }));
    let (end_tx, mut end_rx) = watch::channel(false);

    // Inbound: time the bursts; in echo mode send the audio back (16 kHz up to 24 kHz).
    let reader = {
        let media = media.clone();
        let report = report.clone();
        let end_tx = end_tx.clone();
        let mut frames = media.take_frames();
        tokio::spawn(async move {
            let Some(frames) = frames.as_mut() else {
                return;
            };
            let mut detector = OnsetDetector::new(IN_RATE);
            let mut echo = if audio == Audio::Echo {
                Resampler::new(IN_RATE, OUT_RATE).ok()
            } else {
                None
            };
            let sink = media.sink();
            while let Some(frame) = frames.next().await {
                {
                    let mut r = report.lock().expect("report lock");
                    r.frames += 1;
                    if detector.feed(&frame.samples).is_some() {
                        r.received.push(unix_at(frame.at));
                    }
                }
                if let Some(echo) = echo.as_mut() {
                    echo.push(&frame.samples);
                    while let Some(out) = echo.pull_frame() {
                        sink.try_send(&out);
                    }
                }
            }
            let _ = end_tx.send(true);
        })
    };

    // Outbound: the marker tone at real-time pace, 20 ms per tick, only while the link is
    // up (what sits in the sink before the peer connects would be measured as latency),
    // the sink cleared each time the peer (re)connects.
    let feeder = {
        let media = media.clone();
        let report = report.clone();
        tokio::spawn(async move {
            if audio != Audio::Markers {
                return;
            }
            let sink = media.sink();
            let link = media.link();
            let mut tone = MarkerTone::new(OUT_RATE);
            let mut ticks = tokio::time::interval(Duration::from_millis(20));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
            let mut was_up = false;
            loop {
                ticks.tick().await;
                let up = *link.borrow() == Link::Connected;
                if !up {
                    was_up = false;
                    continue;
                }
                if !was_up {
                    sink.clear();
                    was_up = true;
                }
                let (pcm, onset) = tone.frame(frame_samples(OUT_RATE));
                if onset.is_some() {
                    report.lock().expect("report lock").sent.push(unix_now());
                }
                if sink.send(&pcm).await.is_err() {
                    return;
                }
            }
        })
    };

    // The RTVI handshake.
    let talker = {
        let media = media.clone();
        let report = report.clone();
        let end_tx = end_tx.clone();
        let mut messages = media.take_messages();
        tokio::spawn(async move {
            let Some(messages) = messages.as_mut() else {
                return;
            };
            while let Some(message) = messages.next().await {
                match message {
                    ClientMessage::ClientReady { id, version } => {
                        info!("probe: client-ready");
                        let ready = Outbound::BotReady {
                            id,
                            version: bot_ready_version(version.as_deref()),
                        };
                        let status = Outbound::server_message(serde_json::json!({
                            "type": "status",
                            "status": {"state": "listening", "session_origin": "phone"}
                        }))
                        .expect("a status event");
                        let mut r = report.lock().expect("report lock");
                        r.client_ready.push(unix_now());
                        if media.send(&ready).is_ok() && media.send(&status).is_ok() {
                            r.bot_ready_sent += 1;
                        }
                    }
                    ClientMessage::DisconnectBot { .. } => {
                        info!("probe: disconnect-bot");
                        report
                            .lock()
                            .expect("report lock")
                            .disconnect_bot
                            .push(unix_now());
                        let _ = end_tx.send(true);
                    }
                }
            }
        })
    };

    // The link.
    let mut link = media.link();
    {
        let state = *link.borrow();
        report
            .lock()
            .expect("report lock")
            .links
            .push((unix_now(), format!("{state:?}")));
    }
    let mut gone_since: Option<Instant> = None;
    loop {
        let grace = async {
            match gone_since {
                Some(t) => tokio::time::sleep_until((t + GONE_GRACE).into()).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            changed = link.changed() => {
                let state = *link.borrow_and_update();
                info!("probe: link {state:?}");
                report.lock().expect("report lock").links.push((unix_now(), format!("{state:?}")));
                if changed.is_err() {
                    break;
                }
                gone_since = if state == Link::Gone { Some(Instant::now()) } else { None };
            }
            _ = grace => {
                info!("probe: the peer has been gone for {GONE_GRACE:?}");
                break;
            }
            _ = end_rx.changed() => {
                if *end_rx.borrow() {
                    break;
                }
            }
            _ = stop.changed() => {
                if *stop.borrow() {
                    break;
                }
            }
        }
    }
    feeder.abort();
    reader.abort();
    talker.abort();
    let stats = media.stats();
    let mut r = report.lock().expect("report lock").clone();
    r.stats = stats;
    r
}
