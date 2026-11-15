//! Debug mode (ADR 0074): `GHOST_DEBUG_AUDIO` for an hour, from the operator's own switch.
//!
//! The switch turns on exactly what `GHOST_DEBUG_AUDIO=1` turns on, through the same gate
//! (`ghost_voice::debug::enable`): what Ghost hears and its audio decisions, logged to the
//! daemon's own log (the journal or the terminal). Nothing else is logged, and nothing goes
//! anywhere else, with one bounded exception: while the switch is on, the same lines are kept
//! in a 0600 file under Ghost's data folder ([`CAPTURE_FILE`]) so the person can add them to a
//! diagnostic report; the file is deleted when debug mode turns off, and at every start.
//!
//! It is held in this process's memory only: never written to `settings.env` or the store,
//! so a restart of Ghost always turns it off. It turns itself off [`MINUTES`] after it was
//! turned on, by a timer here, not by the page. While it is on, the status carries its end
//! (the desktop's marker) and the waiting list one note saying so.
//!
//! The operator's only: the control API's `PUT /debug-audio` and `ghostd debug-audio`. No
//! tool of Ghost's own app, run, outside client, phone route, app or model reaches it. It
//! opens nothing: with listening off (ADR 0057) no capture is open, and it opens none.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use ghost_api::DaemonState;
use ghost_api::developer::{DEBUG_AUDIO_MINUTES as MINUTES, DebugAudioPort, DebugAudioView};
use ghost_api::models::DebugAudioMark;
use tracing_subscriber::layer::Context;

/// The tracing target of `ghost_voice::debug::audio`: the only lines debug mode adds.
pub const TARGET: &str = "ghost_voice::debug";
/// The capture's name in Ghost's data folder.
pub const CAPTURE_FILE: &str = "debug-audio.log";
/// The most the capture keeps; past it, one line says so and nothing more is written.
pub const CAPTURE_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// The waiting list's key for debug mode's note.
pub const NOTE_KEY: &str = "debug:audio";

/// The capture: a file while debug mode's switch is on, nothing otherwise. Cloned into the
/// subscriber as a layer; the switch opens and closes it.
#[derive(Clone, Default)]
pub struct Capture {
    file: Arc<Mutex<Option<Open>>>,
}

struct Open {
    path: PathBuf,
    file: std::fs::File,
    written: u64,
    full: bool,
}

impl Capture {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Open>> {
        self.file.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start keeping lines in `path`, 0600, emptied first.
    pub fn open(&self, path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        *self.lock() = Some(Open {
            path: path.to_path_buf(),
            file,
            written: 0,
            full: false,
        });
        Ok(())
    }

    /// Stop, and delete the file.
    pub fn close(&self) {
        if let Some(open) = self.lock().take() {
            drop(open.file);
            let _ = std::fs::remove_file(&open.path);
        }
    }

    /// The bytes kept now.
    pub fn bytes(&self) -> u64 {
        self.lock().as_ref().map_or(0, |o| o.written)
    }

    /// What the file holds now, if it is open.
    pub fn read(&self) -> Option<String> {
        let path = self.lock().as_ref().map(|o| o.path.clone())?;
        std::fs::read_to_string(path).ok()
    }

    fn write(&self, line: &str) {
        let mut g = self.lock();
        let Some(open) = g.as_mut() else { return };
        if open.full {
            return;
        }
        let bytes = line.len() as u64 + 1;
        if open.written + bytes > CAPTURE_MAX_BYTES {
            open.full = true;
            let _ = writeln!(open.file, "[the debug log is full; nothing more is kept]");
            return;
        }
        if writeln!(open.file, "{line}").is_ok() {
            open.written += bytes;
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        if self.lock().is_none() {
            return;
        }
        let mut v = crate::daemon::MessageVisitor::default();
        event.record(&mut v);
        let line = format!(
            "{} {}",
            Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            ghost_core::redact::redact_text(&v.0)
        );
        self.write(&line);
    }
}

/// Delete a capture left by a Ghost that stopped while debug mode was on.
pub fn sweep(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join(CAPTURE_FILE));
}

struct On {
    until: DateTime<Utc>,
    /// Bumped at every change, so a timer set for an earlier switch does nothing.
    generation: u64,
}

/// The switch.
pub struct DebugAudio {
    /// Ghost was started with `GHOST_DEBUG_AUDIO=1`: on for this process whatever the switch.
    from_environment: bool,
    on: Mutex<Option<On>>,
    generation: Mutex<u64>,
    capture: Capture,
    capture_path: PathBuf,
    state: Arc<DaemonState>,
    /// The waiting list read again (debug mode's note came or went).
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl DebugAudio {
    pub fn new(
        from_environment: bool,
        capture: Capture,
        data_dir: &Path,
        state: Arc<DaemonState>,
    ) -> Arc<Self> {
        sweep(data_dir);
        let me = Arc::new(Self {
            from_environment,
            on: Mutex::new(None),
            generation: Mutex::new(0),
            capture,
            capture_path: data_dir.join(CAPTURE_FILE),
            state,
            on_change: Mutex::new(None),
        });
        me.publish();
        me
    }

    pub fn set_on_change(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.on_change.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    fn until(&self) -> Option<DateTime<Utc>> {
        self.on
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|o| o.until)
    }

    pub fn is_on(&self) -> bool {
        self.from_environment || self.until().is_some()
    }

    /// What the debug capture holds now (the diagnostic report's box).
    pub fn captured(&self) -> Option<String> {
        self.until().and(self.capture.read())
    }

    /// Debug mode's note on the waiting list, while the switch is on.
    pub fn note(&self) -> Option<String> {
        let until = self.until()?;
        let local = until.with_timezone(&chrono::Local).format("%H:%M");
        Some(format!(
            "Debug mode is on: Ghost is logging what it hears and its audio decisions on this computer until {local}. Turn it off on Settings › Developer."
        ))
    }

    fn publish(&self) {
        ghost_voice::debug::enable(self.is_on());
        let mark = if let Some(until) = self.until() {
            Some(DebugAudioMark {
                until: Some(until.to_rfc3339()),
            })
        } else if self.from_environment {
            Some(DebugAudioMark { until: None })
        } else {
            None
        };
        self.state.set_debug_audio(mark);
        let f = self
            .on_change
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(f) = f {
            f();
        }
    }

    /// On for [`MINUTES`] from now (again, if it was on), or off now.
    pub fn switch(self: &Arc<Self>, on: bool) -> DebugAudioView {
        self.switch_at(on, Utc::now(), Duration::minutes(MINUTES))
    }

    pub(crate) fn switch_at(
        self: &Arc<Self>,
        on: bool,
        now: DateTime<Utc>,
        length: Duration,
    ) -> DebugAudioView {
        let generation = {
            let mut g = self.generation.lock().unwrap_or_else(|e| e.into_inner());
            *g += 1;
            *g
        };
        if on {
            let until = now + length;
            *self.on.lock().unwrap_or_else(|e| e.into_inner()) = Some(On { until, generation });
            if let Err(e) = self.capture.open(&self.capture_path) {
                tracing::warn!("debug mode: the log for a diagnostic report can't be kept: {e}");
            }
            tracing::warn!(
                "debug mode is on until {}: transcripts and audio decisions are being logged",
                until.to_rfc3339()
            );
            self.publish();
            // The daemon's own timer: the page may be closed, the window gone.
            if let Ok(h) = tokio::runtime::Handle::try_current() {
                let weak = Arc::downgrade(self);
                let wait = (until - now).to_std().unwrap_or_default();
                h.spawn(async move {
                    tokio::time::sleep(wait).await;
                    if let Some(me) = weak.upgrade() {
                        me.expire(generation);
                    }
                });
            }
        } else {
            self.off();
        }
        self.view()
    }

    /// The timer for `generation` ran out.
    pub(crate) fn expire(&self, generation: u64) {
        let current = self
            .on
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|o| o.generation);
        if current == Some(generation) {
            tracing::info!("debug mode: an hour is up; it is off");
            self.off();
        }
    }

    fn off(&self) {
        let was = self.on.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.capture.close();
        if was.is_some() {
            tracing::info!("debug mode is off");
        }
        self.publish();
    }

    pub fn view(&self) -> DebugAudioView {
        DebugAudioView {
            on: self.is_on(),
            until: self.until().map(|u| u.to_rfc3339()),
            from_environment: self.from_environment,
            log_bytes: self.capture.bytes(),
        }
    }
}

/// The control API's side.
pub struct DebugAudioDesk(pub Arc<DebugAudio>);

impl DebugAudioPort for DebugAudioDesk {
    fn view(&self) -> DebugAudioView {
        self.0.view()
    }

    fn set(&self, on: bool) -> DebugAudioView {
        self.0.switch(on)
    }
}

/// `ghostd debug-audio on|off|status`, through the running daemon.
pub async fn cli(
    args: &[String],
    client: &ghost_api::GhostClient,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let usage = "ghostd debug-audio on|off|status";
    let reply = match args.first().map(String::as_str) {
        None | Some("status") => client.debug_audio(None).await,
        Some(w @ ("on" | "off")) => client.debug_audio(Some(w == "on")).await,
        Some(other) => {
            let _ = writeln!(err, "unknown: ghostd debug-audio {other}; {usage}");
            return 2;
        }
    };
    match reply {
        Ok(v) => {
            let on = v["on"].as_bool().unwrap_or(false);
            let line = match (on, v["until"].as_str(), v["from_environment"].as_bool()) {
                (false, _, _) => "debug mode: off".to_string(),
                (true, Some(u), _) => format!(
                    "debug mode: on until {u}: Ghost is logging what it hears and its audio decisions to its log"
                ),
                (true, None, Some(true)) => {
                    "debug mode: on, because Ghost was started with GHOST_DEBUG_AUDIO=1".to_string()
                }
                (true, None, _) => "debug mode: on".to_string(),
            };
            let _ = writeln!(out, "{line}");
            0
        }
        Err(e) => {
            let _ = writeln!(err, "ghostd debug-audio: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_api::models::Build;
    use tracing_subscriber::layer::SubscriberExt;

    fn state() -> Arc<DaemonState> {
        Arc::new(DaemonState::with_build("test", "file", Build::default()))
    }

    // `ghost_voice::debug`'s gate is one flag for the process: these tests share it, so they
    // run one at a time.
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn it_turns_itself_off_when_its_time_is_up() {
        let _one = ONE.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let st = state();
        let d = DebugAudio::new(false, Capture::new(), tmp.path(), st.clone());
        assert!(!d.is_on() && st.debug_audio().is_none());
        let v = d.switch_at(true, Utc::now(), Duration::milliseconds(80));
        assert!(v.on && v.until.is_some());
        assert!(ghost_voice::debug::enabled());
        assert!(st.debug_audio().unwrap().until.is_some());
        assert!(d.note().unwrap().contains("Debug mode is on"));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!d.is_on(), "the daemon's timer turned it off");
        assert!(!ghost_voice::debug::enabled());
        assert!(st.debug_audio().is_none());
        assert!(d.note().is_none());
    }

    #[tokio::test]
    async fn a_timer_from_an_earlier_switch_does_not_end_a_later_one() {
        let _one = ONE.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let d = DebugAudio::new(false, Capture::new(), tmp.path(), state());
        d.switch_at(true, Utc::now(), Duration::milliseconds(50));
        d.switch_at(true, Utc::now(), Duration::minutes(60));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(d.is_on());
        d.switch(false);
        assert!(!d.is_on() && !ghost_voice::debug::enabled());
    }

    #[test]
    fn a_restart_turns_it_off_and_nothing_persists_it() {
        let _one = ONE.blocking_lock();
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("settings.env");
        std::fs::write(&settings, "GHOST_WAKE_PHRASES=hey ghost\n").unwrap();
        let before = std::fs::read_to_string(&settings).unwrap();
        let d = DebugAudio::new(false, Capture::new(), tmp.path(), state());
        d.switch(true);
        assert!(tmp.path().join(CAPTURE_FILE).exists());
        // Nothing Ghost reads at its start says it was on.
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        // A new process: a new switch over the same folder, off, the capture swept.
        drop(d);
        ghost_voice::debug::enable(false);
        let again = DebugAudio::new(false, Capture::new(), tmp.path(), state());
        assert!(!again.is_on() && !ghost_voice::debug::enabled());
        assert!(!tmp.path().join(CAPTURE_FILE).exists());
    }

    #[test]
    fn started_with_the_variable_it_stays_on_and_the_switch_says_so() {
        let _one = ONE.blocking_lock();
        let tmp = tempfile::tempdir().unwrap();
        let st = state();
        let d = DebugAudio::new(true, Capture::new(), tmp.path(), st.clone());
        assert!(d.view().on && d.view().from_environment && d.view().until.is_none());
        assert_eq!(st.debug_audio().unwrap().until, None);
        d.switch(false);
        assert!(
            ghost_voice::debug::enabled(),
            "the variable is the operator's by hand"
        );
        ghost_voice::debug::enable(false);
    }

    #[test]
    fn the_capture_keeps_debug_lines_only_bounded_0600_and_deleted_when_off() {
        use std::os::unix::fs::PermissionsExt;
        let _one = ONE.blocking_lock();
        let tmp = tempfile::tempdir().unwrap();
        let cap = Capture::new();
        let sub = tracing_subscriber::registry().with(cap.clone());
        let d = DebugAudio::new(false, cap.clone(), tmp.path(), state());
        tracing::subscriber::with_default(sub, || {
            tracing::info!(target: "ghost_voice::debug", "[audio] heard: before it was on");
            d.switch(true);
            tracing::info!(target: "ghost_voice::debug", "[audio] heard: a synthetic phrase");
            tracing::info!("api: listening on a socket");
        });
        let path = tmp.path().join(CAPTURE_FILE);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let text = d.captured().unwrap();
        assert!(text.contains("a synthetic phrase"));
        assert!(!text.contains("before it was on"));
        assert!(!text.contains("api: listening"));
        // Bounded.
        let long = "x".repeat(64 * 1024);
        for _ in 0..80 {
            cap.write(&long);
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= CAPTURE_MAX_BYTES + 128);
        d.switch(false);
        assert!(!path.exists());
        assert!(d.captured().is_none());
    }

    #[test]
    fn with_listening_off_it_opens_nothing() {
        // It is a log flag: the switch has no route to a capture stream. Listening off stays
        // off and the status says nothing else changed.
        let _one = ONE.blocking_lock();
        let tmp = tempfile::tempdir().unwrap();
        let st = state();
        st.set_not_listening(Some(ghost_api::models::NotListening { until: None }));
        let d = DebugAudio::new(false, Capture::new(), tmp.path(), st.clone());
        d.switch(true);
        assert!(st.not_listening().is_some());
        let src = include_str!("debug_audio.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        for word in [
            "capture_stream",
            "open_input",
            "ghost_audio::",
            "Listening::",
        ] {
            assert!(!body.contains(word), "{word}");
        }
        d.switch(false);
    }
}
