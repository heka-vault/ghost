//! What Ghost listens on, and whether it is still hearing (ADR 0064).
//!
//! **The plan.** Every machine is one of a few cases, read from PipeWire (`pw-dump`):
//! - [`Case::OnChip`]: the microphone cancels echo on its own chip against what it plays,
//!   and Ghost plays through it (a mic array like the XVF3800, a conference puck, a
//!   speakerphone). Ghost hears it directly, an array's processed channel taken alone.
//! - [`Case::Headset`]: Ghost's voice goes to the operator's ears, not into the room.
//! - [`Case::HostCanceller`]: Ghost's own canceller (`ghost.mic`, `infra/pipewire`) cleans
//!   the microphone against what the speakers play.
//! - [`Case::Unprotected`]: nothing cancels Ghost's voice (a laptop's microphone and
//!   speakers, a webcam with desk speakers and no canceller). Ghost listens anyway and
//!   says it may hear itself; headphones help.
//!
//! The operator's settings win (`GHOST_INPUT_DEVICE`, `GHOST_WAKE_INPUT_DEVICE`); unset,
//! the case chooses. A setting that names a device that is not there falls back toward
//! working (the case's own choice) rather than toward silence, and says so.
//!
//! **The health.** The passive listener's audio passes a [`Meter`] (numbers per 3 s, never
//! the audio), and every check of a weak wake hit reports its level and whether Parakeet
//! read words. [`Health`] turns those into a [`Problem`]: a stream chopped into tiny
//! fragments (a USB link dropping packets, issue #26), or loud stretches that read as
//! nothing again and again (`ghost.mic` gone deaf, issue #58). A problem is a note on the
//! waiting list leading to Settings › Sound; [`Repairer`] restarts the canceller and the
//! listener reopens the microphone, on the operator's press or, when they chose it, by
//! itself while no session is open.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ghost_audio::input::{CANCELLER_SOURCE, InputSpec, cancels_on_chip};
use serde::Serialize;

/// The waiting list's key for the microphone's note (place: Settings › Sound).
pub const NOTE_KEY: &str = "input:degraded";
/// The canceller's user unit (`infra/pipewire/install.sh`).
pub const CANCELLER_UNIT: &str = "ghost-echo-cancel.service";

// --- what the sound stack looks like -------------------------------------------------------

/// One PipeWire node, as `pw-dump` shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub class: String,
    pub description: String,
    pub channels: u16,
    pub props: BTreeMap<String, String>,
    /// PipeWire says it is running (a sink with something playing on it).
    pub running: bool,
}

impl Node {
    fn device(&self) -> Option<&str> {
        self.props.get("device.id").map(String::as_str)
    }

    fn is_source(&self) -> bool {
        self.class == "Audio/Source" || self.class == "Audio/Duplex"
    }

    fn is_sink(&self) -> bool {
        self.class == "Audio/Sink" || self.class == "Audio/Duplex"
    }

    /// Worn by the operator: a headset, headphones, earbuds, a Bluetooth headset profile.
    fn is_worn(&self) -> bool {
        let form = self
            .props
            .get("device.form-factor")
            .map(|f| f.to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(
            form.as_str(),
            "headset" | "headphone" | "handset" | "hands-free" | "earbuds"
        ) {
            return true;
        }
        let text = format!("{} {}", self.name, self.description).to_ascii_lowercase();
        [
            "headset",
            "headphone",
            "earbud",
            "airpods",
            "bluez_input",
            "handsfree",
            "hfp",
        ]
        .iter()
        .any(|w| text.contains(w))
    }

    /// The name the operator recognises.
    pub fn shown(&self) -> &str {
        if self.description.is_empty() {
            &self.name
        } else {
            &self.description
        }
    }
}

/// The sound devices and Ghost's canceller, read once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundFacts {
    pub nodes: Vec<Node>,
    pub default_source: Option<String>,
    pub default_sink: Option<String>,
}

impl SoundFacts {
    pub fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }

    /// The canceller's source, when it is running.
    pub fn canceller(&self) -> Option<&Node> {
        self.node(CANCELLER_SOURCE).filter(|n| n.is_source())
    }

    /// The microphone the canceller cleans (its capture stream's target).
    pub fn canceller_mic(&self) -> Option<&Node> {
        let capture = self.node("ghost.mic.capture")?;
        let target = capture
            .props
            .get("target.object")
            .or_else(|| capture.props.get("node.target"))?;
        self.node(target).filter(|n| n.is_source())
    }

    /// The hardware behind an input: the canceller's microphone, the named node, or the
    /// default source.
    fn hardware(&self, spec: &InputSpec) -> Option<&Node> {
        match spec.node.as_deref() {
            Some(CANCELLER_SOURCE) => self.canceller_mic(),
            Some(name) => self.node(name),
            None => self.default_source.as_deref().and_then(|n| self.node(n)),
        }
    }

    fn sink(&self, output: Option<&str>) -> Option<&Node> {
        output
            .or(self.default_sink.as_deref())
            .and_then(|n| self.node(n))
            .filter(|n| n.is_sink())
    }

    /// Whether an input names something that is there.
    fn present(&self, spec: &InputSpec) -> bool {
        match spec.node.as_deref() {
            None => true,
            Some(CANCELLER_SOURCE) => self.canceller().is_some(),
            Some(name) => self.node(name).is_some_and(Node::is_source),
        }
    }
}

fn text_of(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The facts from `pw-dump`'s JSON: the nodes and the `default` metadata's choices.
pub fn facts_from_pw_dump(json: &str) -> Option<SoundFacts> {
    let all: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let mut facts = SoundFacts::default();
    for o in &all {
        let kind = o.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind.ends_with(":Node") {
            let Some(raw) = o.pointer("/info/props").and_then(|p| p.as_object()) else {
                continue;
            };
            let props: BTreeMap<String, String> = raw
                .iter()
                .filter_map(|(k, v)| text_of(v).map(|v| (k.clone(), v)))
                .collect();
            let Some(name) = props.get("node.name").cloned() else {
                continue;
            };
            facts.nodes.push(Node {
                class: props.get("media.class").cloned().unwrap_or_default(),
                description: props.get("node.description").cloned().unwrap_or_default(),
                channels: props
                    .get("audio.channels")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0),
                name,
                props,
                running: o.pointer("/info/state").and_then(|v| v.as_str()) == Some("running"),
            });
        } else if kind.ends_with(":Metadata")
            && o.pointer("/props/metadata.name").and_then(|n| n.as_str()) == Some("default")
        {
            for m in o
                .get("metadata")
                .and_then(|m| m.as_array())
                .into_iter()
                .flatten()
            {
                let name = m
                    .pointer("/value/name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string);
                match m.get("key").and_then(|k| k.as_str()) {
                    Some("default.audio.source") => facts.default_source = name,
                    Some("default.audio.sink") => facts.default_sink = name,
                    Some("default.configured.audio.source") if facts.default_source.is_none() => {
                        facts.default_source = name
                    }
                    Some("default.configured.audio.sink") if facts.default_sink.is_none() => {
                        facts.default_sink = name
                    }
                    _ => {}
                }
            }
        }
    }
    Some(facts)
}

/// Run a tool of the sound stack with no password in its environment; its stdout, or None.
fn run_tool(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .envs(crate::passwords::child_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read while it runs: a dump larger than the pipe would otherwise never finish.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut buf);
        buf
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    };
    let out = reader.join().ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&out).into_owned())
}

/// The machine's facts now, or None without PipeWire's tools (a test machine, CI).
pub fn live_facts() -> Option<SoundFacts> {
    facts_from_pw_dump(&run_tool("pw-dump", &[], Duration::from_secs(3))?)
}

// --- the plan --------------------------------------------------------------------------------

/// Which kind of microphone path the machine has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Case {
    OnChip,
    Headset,
    HostCanceller,
    Unprotected,
    /// PipeWire could not be read: the settings as given, or the default source.
    Unknown,
}

impl Case {
    pub fn as_str(self) -> &'static str {
        match self {
            Case::OnChip => "on-chip",
            Case::Headset => "headset",
            Case::HostCanceller => "host-canceller",
            Case::Unprotected => "unprotected",
            Case::Unknown => "unknown",
        }
    }
}

/// What Ghost listens on, and why, in the operator's words.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub case: Case,
    /// What a session hears.
    pub session: InputSpec,
    /// What the wake detector hears.
    pub wake: InputSpec,
    /// One or two sentences: the case, and what Ghost did about it.
    pub summary: String,
    /// A better choice than the settings make, if there is one.
    pub advice: Option<String>,
    /// The settings' choice to switch to for that advice (`GHOST_INPUT_DEVICE`, value).
    pub suggest: Vec<(String, String)>,
    /// A setting named a device that is not there, and Ghost fell back.
    pub fell_back: Option<String>,
}

impl Plan {
    /// Whether a desk session's echo filter keeps its warm-up rule
    /// (`ghost_voice::echo::WARMUP_SECS`): a short segment begun right as Ghost's voice
    /// starts is taken for its leak. Kept where a canceller can leak at the onset: Ghost's
    /// own canceller (`ghost.mic`, about half a second, #42), even over a chip that
    /// cancels; an unprotected microphone; a path Ghost could not read. Off where the
    /// microphone is heard directly and either its chip cancels against what Ghost plays
    /// (2026-11-13: nothing over the room's floor in the first half second of 122 onsets)
    /// or Ghost's voice is in the operator's ears, since there the rule only costs the
    /// operator's short words right as Ghost starts (#54).
    pub fn echo_warmup(&self) -> bool {
        self.session.is_canceller() || !matches!(self.case, Case::OnChip | Case::Headset)
    }
}

/// The case's own choice of input, given the hardware.
fn case_of(facts: &SoundFacts, mic: Option<&Node>, output: Option<&str>) -> Case {
    let sink = facts.sink(output);
    let same_device = match (mic.and_then(Node::device), sink.and_then(Node::device)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    match mic {
        Some(m) if cancels_on_chip(&m.props) && same_device => Case::OnChip,
        Some(m) if m.is_worn() || sink.is_some_and(Node::is_worn) => Case::Headset,
        _ if facts.canceller().is_some() && facts.canceller_mic().is_some() => Case::HostCanceller,
        _ => Case::Unprotected,
    }
}

/// An on-chip device's input: its processed channel alone when it has several.
fn on_chip_input(mic: &Node) -> InputSpec {
    InputSpec {
        node: Some(mic.name.clone()),
        channel: (mic.channels != 1).then_some(ghost_audio::input::ON_CHIP_PROCESSED_CHANNEL),
    }
}

/// The plan from the settings and the facts. `output` is `GHOST_OUTPUT_DEVICE`.
pub fn plan(
    session_setting: Option<&str>,
    wake_setting: Option<&str>,
    output: Option<&str>,
    facts: Option<&SoundFacts>,
) -> Plan {
    fn set(s: Option<&str>) -> Option<InputSpec> {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| InputSpec::parse(Some(s)))
    }
    let session_set = set(session_setting);
    let wake_set = set(wake_setting);
    let Some(facts) = facts else {
        let session = session_set.unwrap_or_default();
        return Plan {
            case: Case::Unknown,
            wake: wake_set.unwrap_or_else(|| session.clone()),
            session,
            summary: "Ghost could not read the sound devices, so it listens where the settings \
                      say, or on the default microphone."
                .into(),
            advice: None,
            suggest: Vec::new(),
            fell_back: None,
        };
    };
    // The hardware the machine would use by itself: the default source (the canceller's
    // microphone when the default is the canceller).
    let default_mic = facts
        .hardware(&InputSpec::default())
        .map(|n| {
            if n.name == CANCELLER_SOURCE {
                facts.canceller_mic().unwrap_or(n)
            } else {
                n
            }
        })
        .cloned();
    let auto_case = case_of(facts, default_mic.as_ref(), output);
    // Named, not left to the capture's own rule (which would take ghost.mic whenever it is
    // there): the default source itself, the case having been judged on it.
    let auto = match (auto_case, &default_mic) {
        (Case::OnChip, Some(m)) => on_chip_input(m),
        (Case::HostCanceller, _) => InputSpec::node(CANCELLER_SOURCE),
        _ => facts
            .default_source
            .clone()
            .filter(|n| facts.node(n).is_some())
            .map(InputSpec::node)
            .unwrap_or_default(),
    };
    let mut fell_back = None;
    let mut keep = |spec: Option<InputSpec>, what: &str, otherwise: InputSpec| match spec {
        Some(s) if facts.present(&s) => s,
        Some(s) => {
            fell_back = Some(format!(
                "The {what} in the settings ({s}) is not there, so Ghost listens on {otherwise} \
                 instead."
            ));
            otherwise
        }
        None => otherwise,
    };
    let session = keep(session_set, "microphone", auto.clone());
    let wake = keep(wake_set, "wake microphone", session.clone());

    // The case of what is actually heard: a session's hardware decides it.
    let mic = facts.hardware(&session).cloned();
    let case = if session.is_canceller() {
        // Through the canceller: on-chip hardware under it still counts as on-chip.
        match case_of(facts, facts.canceller_mic(), output) {
            Case::OnChip => Case::OnChip,
            _ => Case::HostCanceller,
        }
    } else {
        match case_of(facts, mic.as_ref(), output) {
            // The canceller is installed but this input isn't it.
            Case::HostCanceller => Case::Unprotected,
            c => c,
        }
    };
    let shown_mic = mic
        .as_ref()
        .map(|n| n.shown().to_string())
        .unwrap_or_else(|| "the default microphone".into());
    let shown_sink = facts
        .sink(output)
        .map(|n| n.shown().to_string())
        .unwrap_or_else(|| "the speakers".into());
    let mut advice = None;
    let mut suggest = Vec::new();
    // A chip cancels only what plays through its own device: another output with sound on
    // it (music from the computer's speakers while the array plays Ghost) reaches the
    // microphone uncancelled. Said, not switched: the operator may move that audio.
    let elsewhere: Vec<String> = if case == Case::OnChip {
        let device = facts
            .sink(output)
            .and_then(Node::device)
            .map(str::to_string);
        facts
            .nodes
            .iter()
            .filter(|n| n.is_sink() && n.running && n.device().map(str::to_string) != device)
            .map(|n| n.shown().to_string())
            .collect()
    } else {
        Vec::new()
    };
    let summary = match case {
        Case::OnChip => {
            let hw = if session.is_canceller() {
                facts.canceller_mic()
            } else {
                mic.as_ref()
            };
            let direct = hw.map(on_chip_input);
            let name = hw
                .map(|n| n.shown().to_string())
                .unwrap_or(shown_mic.clone());
            if let Some(direct) = direct
                && (session != direct || wake != direct)
            {
                advice = Some(format!(
                    "{name} cancels Ghost's voice by itself, so Ghost can listen to it \
                         directly ({direct}), without the host echo canceller: it hears the \
                         wake better that way and nothing drifts over the day."
                ));
                if wake != direct {
                    suggest.push(("GHOST_WAKE_INPUT_DEVICE".into(), direct.to_string()));
                }
                if session != direct {
                    suggest.push(("GHOST_INPUT_DEVICE".into(), direct.to_string()));
                }
            }
            if session.is_canceller() {
                format!(
                    "{name} cancels echo on its own chip, and Ghost plays through it. Ghost \
                     listens through the host echo canceller too, as the settings say."
                )
            } else {
                format!(
                    "{name} cancels echo on its own chip, and Ghost plays through it, so Ghost \
                     listens to it directly: no echo canceller is needed."
                )
            }
        }
        Case::Headset => format!(
            "You're on a headset ({shown_mic}): Ghost's voice doesn't reach the microphone, \
             so no echo canceller is needed."
        ),
        Case::HostCanceller => {
            let under = facts
                .canceller_mic()
                .map(|n| n.shown().to_string())
                .unwrap_or_else(|| "the microphone".into());
            format!(
                "Ghost's echo canceller cleans {under} against what {shown_sink} plays, and \
                 Ghost listens to it (ghost.mic)."
            )
        }
        Case::Unprotected => {
            if facts.canceller().is_some() {
                advice = Some(
                    "Ghost's echo canceller is running (ghost.mic): choose it as the microphone \
                     so Ghost doesn't hear itself."
                        .into(),
                );
                suggest.push(("GHOST_INPUT_DEVICE".into(), CANCELLER_SOURCE.into()));
            } else {
                advice = Some(
                    "Headphones help, or set up Ghost's echo canceller (Settings › Sound, or \
                     Repair) so it cleans the microphone against the speakers."
                        .into(),
                );
            }
            format!(
                "Nothing cancels Ghost's voice here ({shown_mic} with {shown_sink}): Ghost \
                 listens anyway, and may hear itself."
            )
        }
        Case::Unknown => unreachable!("facts were read"),
    };
    let summary = if elsewhere.is_empty() {
        summary
    } else {
        format!(
            "{summary} Sound is also playing on {}, which its chip can't cancel: play it \
             through the same device, or Ghost may hear it.",
            elsewhere.join(", ")
        )
    };
    Plan {
        case,
        session,
        wake,
        summary,
        advice,
        suggest,
        fell_back,
    }
}

// --- the health ------------------------------------------------------------------------------

/// What a few seconds of the passive stream looked like: numbers only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub secs: f64,
    /// Share of samples that are exactly zero.
    pub zero_share: f64,
    /// Median length, in samples, of the runs between zeros.
    pub median_run: usize,
    pub dbfs: f64,
}

/// The length of a window the meter reports, in seconds.
pub const WINDOW_SECS: f64 = 3.0;

/// Counts zeros, runs and energy over each 3 s of 16 kHz audio. Per sample one compare and
/// one multiply; nothing of the audio is kept.
#[derive(Debug, Clone)]
pub struct Meter {
    rate: u32,
    n: usize,
    zeros: usize,
    sumsq: f64,
    run: usize,
    runs: Vec<u32>,
}

impl Meter {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            n: 0,
            zeros: 0,
            sumsq: 0.0,
            run: 0,
            runs: Vec::with_capacity(1024),
        }
    }

    /// Feed a frame; a window's numbers when one completes.
    pub fn push(&mut self, samples: &[i16]) -> Option<Window> {
        for &s in samples {
            if s == 0 {
                self.zeros += 1;
                if self.run > 0 && self.runs.len() < 1 << 16 {
                    self.runs.push(self.run.min(u32::MAX as usize) as u32);
                }
                self.run = 0;
            } else {
                self.run += 1;
                let f = f64::from(s);
                self.sumsq += f * f;
            }
        }
        self.n += samples.len();
        if (self.n as f64) < WINDOW_SECS * f64::from(self.rate) {
            return None;
        }
        if self.run > 0 {
            self.runs.push(self.run.min(u32::MAX as usize) as u32);
        }
        let median_run = if self.runs.is_empty() {
            0
        } else {
            let mid = self.runs.len() / 2;
            *self.runs.select_nth_unstable(mid).1 as usize
        };
        let rms = (self.sumsq / self.n as f64).sqrt();
        let w = Window {
            secs: self.n as f64 / f64::from(self.rate),
            zero_share: self.zeros as f64 / self.n as f64,
            median_run,
            dbfs: 20.0 * (rms / 32768.0).max(1e-9).log10(),
        };
        self.n = 0;
        self.zeros = 0;
        self.sumsq = 0.0;
        self.run = 0;
        self.runs.clear();
        Some(w)
    }
}

/// A window that looks like a link dropping packets (issue #26: ~60% zeros in runs of
/// 20–30, the audio between in ~5-sample pieces). A canceller's gating leaves long runs of
/// zeros and long runs of signal, which this does not take for a drop.
pub fn looks_chopped(w: &Window) -> bool {
    w.zero_share >= 0.3 && w.median_run > 0 && w.median_run <= 32
}

/// How long chopped windows must last before they are a problem, and clean ones before it
/// clears.
pub const DROPOUT_HOLD: Duration = Duration::from_secs(30);
/// A check of a weak wake hit this loud that reads nothing counts toward deafness.
pub const LOUD_DBFS: f32 = -35.0;
/// This many such reads ...
pub const DEAF_READS: usize = 3;
/// ... within this long.
pub const DEAF_SPAN: Duration = Duration::from_secs(20 * 60);
/// An automatic repair at most this often.
pub const AUTO_REPAIR_EVERY: Duration = Duration::from_secs(60 * 60);

/// What is wrong with the input.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Problem {
    /// The stream arrives chopped into tiny pieces with zeros between.
    Dropping { zero_share: f64 },
    /// Loud stretches read as nothing, again and again.
    Deaf { reads: usize },
}

/// The input's health over time, from the meter and the wake checks.
#[derive(Debug, Default)]
pub struct Health {
    chopped_since: Option<Instant>,
    clean_since: Option<Instant>,
    dropping: Option<f64>,
    deaf: VecDeque<Instant>,
    last_repair: Option<Instant>,
    pub repairs: u64,
}

impl Health {
    pub fn window(&mut self, w: &Window, now: Instant) {
        if looks_chopped(w) {
            self.clean_since = None;
            let since = *self.chopped_since.get_or_insert(now);
            if now.duration_since(since) + Duration::from_secs_f64(w.secs) >= DROPOUT_HOLD {
                self.dropping = Some(w.zero_share);
            }
        } else {
            self.chopped_since = None;
            let since = *self.clean_since.get_or_insert(now);
            if now.duration_since(since) + Duration::from_secs_f64(w.secs) >= DROPOUT_HOLD {
                self.dropping = None;
            }
        }
    }

    /// A check of a weak wake hit: its window's level, how many words Parakeet read, and
    /// whether it was the final read (a first read mid-sentence often reads nothing).
    pub fn check(&mut self, level_dbfs: f32, words: usize, final_: bool, now: Instant) {
        if level_dbfs < LOUD_DBFS {
            return;
        }
        if words > 0 {
            // Loud and read: the stream is hearing.
            self.deaf.clear();
        } else if final_ {
            self.deaf.push_back(now);
        }
        while self
            .deaf
            .front()
            .is_some_and(|t| now.duration_since(*t) > DEAF_SPAN)
        {
            self.deaf.pop_front();
        }
    }

    pub fn problem(&self, now: Instant) -> Option<Problem> {
        if let Some(zero_share) = self.dropping {
            return Some(Problem::Dropping { zero_share });
        }
        let recent = self
            .deaf
            .iter()
            .filter(|t| now.duration_since(**t) <= DEAF_SPAN)
            .count();
        (recent >= DEAF_READS).then_some(Problem::Deaf { reads: recent })
    }

    /// After a repair: forget the deafness (the stream is new); a chopped link is the
    /// hardware's and is measured again from scratch.
    pub fn repaired(&mut self, now: Instant) {
        self.deaf.clear();
        self.dropping = None;
        self.chopped_since = None;
        self.clean_since = None;
        self.last_repair = Some(now);
        self.repairs += 1;
    }

    /// An automatic repair is due: deaf (a repair can help; a dropping link it can't), and
    /// none in the last hour.
    pub fn auto_repair_due(&self, now: Instant) -> bool {
        matches!(self.problem(now), Some(Problem::Deaf { .. }))
            && self
                .last_repair
                .is_none_or(|t| now.duration_since(t) >= AUTO_REPAIR_EVERY)
    }
}

/// The note for the waiting list: what is wrong and the one thing that fixes it.
pub fn note(problem: &Problem, plan: &Plan) -> String {
    match problem {
        Problem::Dropping { zero_share } => format!(
            "The microphone is dropping audio: {:.0}% of what it sends is gaps of silence, \
             the way a USB link that loses packets looks. Plug it into a port on the \
             computer itself, not a hub.",
            zero_share * 100.0
        ),
        Problem::Deaf { reads } => {
            let through = if plan.session.is_canceller() || plan.wake.is_canceller() {
                "Repair restarts Ghost's echo canceller and reopens the microphone."
            } else {
                "Repair reopens the microphone."
            };
            let better = plan
                .advice
                .as_deref()
                .filter(|_| plan.case == Case::OnChip)
                .map(|a| format!(" {a}"))
                .unwrap_or_default();
            format!(
                "Ghost has stopped hearing well: {reads} loud stretches in the last 20 minutes \
                 read as no words. {through}{better}"
            )
        }
    }
}

// --- the repair ------------------------------------------------------------------------------

/// What Repair does with Ghost's echo canceller (ADR 0064, ADR 0066).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellerStep {
    /// It runs and Ghost listens through it: restart it.
    Restart,
    /// Ghost should listen through it and it isn't running (gone after a reinstall, its
    /// unit removed), or nothing cancels Ghost's voice here: install it, from the bundle
    /// or the checkout, and start it.
    Install,
    /// Not the canceller's to fix: a chip or a headset already keeps Ghost's voice out
    /// (ADR 0064), or the case can't be read.
    Leave,
}

/// The step for `plan`, with the canceller `running` or not, and whether the operator
/// pressed Repair (`asked`): an automatic repair restarts, never installs.
pub fn canceller_step(plan: &Plan, running: bool, asked: bool) -> CancellerStep {
    let uses = plan.session.is_canceller() || plan.wake.is_canceller();
    match (uses, running, plan.case) {
        (true, true, _) => CancellerStep::Restart,
        (true, false, _) if asked => CancellerStep::Install,
        (false, false, Case::Unprotected) if asked => CancellerStep::Install,
        _ => CancellerStep::Leave,
    }
}

/// What a repair does to the sound stack; the listener's reopening is the daemon's.
pub trait Repairer: Send + Sync {
    /// Restart Ghost's echo canceller and wait for `ghost.mic`. Err says why not.
    fn restart_canceller(&self) -> Result<(), String>;
    /// Install and start it from the installer under `infra_dir` (ADR 0066). Ok says what
    /// happened; Err why not, in words.
    fn install_canceller(&self, infra_dir: &Path) -> Result<String, String>;
    /// Whether its user unit is running.
    fn canceller_running(&self) -> bool;
}

/// The workstation's: `systemctl --user restart` with the graph's rate held while the
/// canceller is gone, so the graph can't move to 48 kHz under the devices and silence the
/// output (issue #58: the restart's own cost).
pub struct LiveRepairer;

/// The forced rate in PipeWire's `settings` metadata ("0" when none).
fn forced_rate() -> Option<String> {
    let out = run_tool("pw-metadata", &["-n", "settings"], Duration::from_secs(3))?;
    Some(
        out.lines()
            .find_map(|l| {
                l.split("key:'clock.force-rate' value:'")
                    .nth(1)
                    .and_then(|v| v.split('\'').next())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "0".into()),
    )
}

fn set_forced_rate(rate: &str) -> bool {
    run_tool(
        "pw-metadata",
        &["-n", "settings", "0", "clock.force-rate", rate],
        Duration::from_secs(3),
    )
    .is_some()
}

impl Repairer for LiveRepairer {
    fn canceller_running(&self) -> bool {
        ghost_api::deps::unit_active(CANCELLER_UNIT)
    }

    fn install_canceller(&self, infra_dir: &Path) -> Result<String, String> {
        let (ok, detail) = tokio::runtime::Handle::current().block_on(ghost_api::echo_cancel::run(
            infra_dir,
            false,
            ghost_api::echo_cancel::probe_sound_server(),
        ));
        if !ok {
            return Err(detail);
        }
        // The installer waits for ghost.mic; a chip that needs none says so and installs
        // nothing.
        if detail.contains("no host echo canceller is needed") {
            return Ok("no echo canceller is needed here".into());
        }
        Ok("set up the echo canceller".into())
    }

    fn restart_canceller(&self) -> Result<(), String> {
        if !ghost_api::deps::unit_active(CANCELLER_UNIT) {
            return Err("Ghost's echo canceller isn't running".into());
        }
        let facts = live_facts();
        let rate = facts
            .as_ref()
            .and_then(|f| f.canceller_mic())
            .and_then(|n| n.props.get("audio.rate").cloned())
            .unwrap_or_else(|| "16000".into());
        let before = forced_rate();
        let pinned = before.as_deref() == Some("0") && set_forced_rate(&rate);
        let restarted = run_tool(
            "systemctl",
            &["--user", "restart", CANCELLER_UNIT],
            Duration::from_secs(15),
        )
        .is_some();
        let mut up = false;
        if restarted {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(10) {
                if live_facts().is_some_and(|f| f.canceller().is_some()) {
                    up = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        if pinned {
            set_forced_rate("0");
        }
        match (restarted, up) {
            (false, _) => Err("the echo canceller did not restart".into()),
            (true, false) => {
                Err("the echo canceller restarted but ghost.mic didn't come back".into())
            }
            (true, true) => Ok(()),
        }
    }
}

// --- xruns, for `ghostd check` ---------------------------------------------------------------

/// `pw-top -b`'s ERR column per node name, from its last snapshot.
pub fn parse_pw_top(text: &str) -> BTreeMap<String, u64> {
    let mut last = BTreeMap::new();
    let mut current = BTreeMap::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first() == Some(&"S") && cols.get(1) == Some(&"ID") {
            if !current.is_empty() {
                last = std::mem::take(&mut current);
            }
            continue;
        }
        // S ID QUANT RATE WAIT BUSY W/Q B/Q ERR [FORMAT...] NAME
        if cols.len() >= 10
            && let Ok(err) = cols[8].parse::<u64>()
        {
            let name = cols
                .last()
                .unwrap_or(&"")
                .trim_start_matches('+')
                .to_string();
            if !name.is_empty() {
                current.insert(name, err);
            }
        }
    }
    if current.is_empty() { last } else { current }
}

/// The xruns of Ghost's canceller nodes and of the microphone under it, now.
pub fn canceller_xruns() -> Option<BTreeMap<String, u64>> {
    let text = run_tool("pw-top", &["-b", "-n", "2"], Duration::from_secs(5))?;
    Some(parse_pw_top(&text))
}

/// Lines for `ghostd check`: the case, the inputs, any advice, the canceller's xruns.
/// Informational: a sound problem never fails the check (`make deploy` runs it).
pub fn check_lines(
    session_setting: Option<&str>,
    wake_setting: Option<&str>,
    output: Option<&str>,
) -> Vec<String> {
    let facts = live_facts();
    let p = plan(session_setting, wake_setting, output, facts.as_ref());
    let mut out = vec![
        format!("microphone: {} ({})", p.summary, p.case.as_str()),
        format!(
            "microphone: a session hears {}; the wake hears {}",
            p.session, p.wake
        ),
    ];
    if let Some(f) = &p.fell_back {
        out.push(format!("microphone: {f}"));
    }
    if let Some(a) = &p.advice {
        out.push(format!("microphone: advice: {a}"));
        for (k, v) in &p.suggest {
            out.push(format!("microphone:   {k}={v}"));
        }
    }
    if facts.as_ref().is_some_and(|f| f.canceller().is_some())
        && let Some(x) = canceller_xruns()
    {
        let mic = facts
            .as_ref()
            .and_then(|f| f.canceller_mic())
            .map(|n| n.name.clone());
        let mut parts: Vec<String> = ["ghost.mic.capture", "ghost.mic", "echo-cancel-sink"]
            .iter()
            .filter_map(|n| x.get(*n).map(|e| format!("{n} {e}")))
            .collect();
        if let Some(e) = mic.as_ref().and_then(|m| x.get(m)) {
            parts.push(format!("its microphone {e}"));
        }
        if !parts.is_empty() {
            out.push(format!(
                "microphone: echo canceller xruns since it started: {}",
                parts.join(", ")
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        name: &str,
        class: &str,
        device: Option<&str>,
        channels: u16,
        extra: &[(&str, &str)],
    ) -> Node {
        let mut props: BTreeMap<String, String> = extra
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        props.insert("node.name".into(), name.into());
        if let Some(d) = device {
            props.insert("device.id".into(), d.into());
        }
        Node {
            name: name.into(),
            class: class.into(),
            description: String::new(),
            channels,
            props,
            running: false,
        }
    }

    const ARRAY_IN: &str =
        "alsa_input.usb-Seeed_Studio_reSpeaker_XVF3800_4-Mic_Array_0001-00.iec958-stereo";
    const ARRAY_OUT: &str =
        "alsa_output.usb-Seeed_Studio_reSpeaker_XVF3800_4-Mic_Array_0001-00.analog-stereo";

    /// The owner's desk: the array is mic and speakers, the canceller over it.
    fn array_desk(canceller: bool) -> SoundFacts {
        let mut nodes = vec![
            node(ARRAY_IN, "Audio/Source", Some("66"), 2, &[]),
            node(ARRAY_OUT, "Audio/Sink", Some("66"), 2, &[]),
            node(
                "alsa_input.usb-046d_HD_Webcam_C615-00.mono-fallback",
                "Audio/Source",
                Some("70"),
                1,
                &[],
            ),
        ];
        if canceller {
            nodes.push(node("ghost.mic", "Audio/Source", None, 1, &[]));
            nodes.push(node(
                "ghost.mic.capture",
                "Stream/Input/Audio",
                None,
                1,
                &[("node.target", ARRAY_IN)],
            ));
        }
        SoundFacts {
            nodes,
            default_source: Some(ARRAY_IN.into()),
            default_sink: Some(ARRAY_OUT.into()),
        }
    }

    const LAPTOP_MIC: &str = "alsa_input.pci-0000_00_1f.3.analog-stereo";

    /// A laptop: its own microphone and speakers on one card, nothing cancelling.
    fn laptop(canceller: bool) -> SoundFacts {
        let mut nodes = vec![
            node(
                "alsa_input.pci-0000_00_1f.3.analog-stereo",
                "Audio/Source",
                Some("50"),
                2,
                &[("device.form-factor", "internal")],
            ),
            node(
                "alsa_output.pci-0000_00_1f.3.analog-stereo",
                "Audio/Sink",
                Some("50"),
                2,
                &[("device.form-factor", "internal")],
            ),
        ];
        if canceller {
            nodes.push(node("ghost.mic", "Audio/Source", None, 1, &[]));
            nodes.push(node(
                "ghost.mic.capture",
                "Stream/Input/Audio",
                None,
                1,
                &[("target.object", "alsa_input.pci-0000_00_1f.3.analog-stereo")],
            ));
        }
        SoundFacts {
            nodes,
            default_source: Some("alsa_input.pci-0000_00_1f.3.analog-stereo".into()),
            default_sink: Some("alsa_output.pci-0000_00_1f.3.analog-stereo".into()),
        }
    }

    fn headset() -> SoundFacts {
        SoundFacts {
            nodes: vec![
                node(
                    "alsa_input.usb-Logitech_USB_Headset-00.mono-fallback",
                    "Audio/Source",
                    Some("80"),
                    1,
                    &[("device.form-factor", "headset")],
                ),
                node(
                    "alsa_output.usb-Logitech_USB_Headset-00.analog-stereo",
                    "Audio/Sink",
                    Some("80"),
                    2,
                    &[("device.form-factor", "headset")],
                ),
                node(
                    "alsa_output.pci-0000_00_1f.3.hdmi-stereo",
                    "Audio/Sink",
                    Some("50"),
                    2,
                    &[],
                ),
            ],
            default_source: Some("alsa_input.usb-Logitech_USB_Headset-00.mono-fallback".into()),
            default_sink: Some("alsa_output.usb-Logitech_USB_Headset-00.analog-stereo".into()),
        }
    }

    /// A conference puck: one channel, cancels on chip, Ghost plays through it.
    fn puck() -> SoundFacts {
        SoundFacts {
            nodes: vec![
                node(
                    "alsa_input.usb-Jabra_SPEAK_510_USB-00.mono-fallback",
                    "Audio/Source",
                    Some("90"),
                    1,
                    &[],
                ),
                node(
                    "alsa_output.usb-Jabra_SPEAK_510_USB-00.analog-stereo",
                    "Audio/Sink",
                    Some("90"),
                    2,
                    &[],
                ),
            ],
            default_source: Some("alsa_input.usb-Jabra_SPEAK_510_USB-00.mono-fallback".into()),
            default_sink: Some("alsa_output.usb-Jabra_SPEAK_510_USB-00.analog-stereo".into()),
        }
    }

    /// A webcam's microphone with desk speakers on another card.
    fn webcam() -> SoundFacts {
        SoundFacts {
            nodes: vec![
                node(
                    "alsa_input.usb-046d_HD_Webcam_C615-00.mono-fallback",
                    "Audio/Source",
                    Some("70"),
                    1,
                    &[],
                ),
                node(
                    "alsa_output.pci-0000_00_1f.3.analog-stereo",
                    "Audio/Sink",
                    Some("50"),
                    2,
                    &[],
                ),
            ],
            default_source: Some("alsa_input.usb-046d_HD_Webcam_C615-00.mono-fallback".into()),
            default_sink: Some("alsa_output.pci-0000_00_1f.3.analog-stereo".into()),
        }
    }

    #[test]
    fn an_array_that_cancels_on_chip_is_heard_directly_on_its_processed_channel() {
        let p = plan(None, None, None, Some(&array_desk(false)));
        assert_eq!(p.case, Case::OnChip);
        assert_eq!(p.session.to_string(), format!("{ARRAY_IN}#0"));
        assert_eq!(p.wake, p.session);
        assert!(p.advice.is_none());
        assert!(
            p.summary.contains("no echo canceller is needed"),
            "{}",
            p.summary
        );
        // Unset with the canceller installed too: still the array, directly.
        let p = plan(None, None, None, Some(&array_desk(true)));
        assert_eq!(p.case, Case::OnChip);
        assert_eq!(p.session.channel, Some(0));
    }

    #[test]
    fn the_owners_settings_win_and_ghost_says_what_would_be_better() {
        // Today: ghost.mic set, over the array.
        let p = plan(Some("ghost.mic"), None, None, Some(&array_desk(true)));
        assert_eq!(p.case, Case::OnChip);
        assert!(p.session.is_canceller() && p.wake.is_canceller());
        assert!(p.advice.as_deref().unwrap().contains("directly"));
        let direct = format!("{ARRAY_IN}#0");
        assert!(
            p.suggest
                .contains(&("GHOST_WAKE_INPUT_DEVICE".to_string(), direct.clone()))
        );
        // The wake on the array, sessions on ghost.mic: the wake has what it needs.
        let p = plan(
            Some("ghost.mic"),
            Some(&direct),
            None,
            Some(&array_desk(true)),
        );
        assert_eq!(p.wake.to_string(), direct);
        assert!(p.session.is_canceller());
        assert_eq!(
            p.suggest,
            vec![("GHOST_INPUT_DEVICE".to_string(), direct.clone())]
        );
    }

    #[test]
    fn a_setting_for_a_device_that_is_gone_falls_back_toward_working() {
        // ghost.mic set, the canceller not running: the array directly, not silence.
        let p = plan(Some("ghost.mic"), None, None, Some(&array_desk(false)));
        assert_eq!(p.session.to_string(), format!("{ARRAY_IN}#0"));
        assert!(p.fell_back.as_deref().unwrap().contains("ghost.mic"));
        // A laptop with ghost.mic set and nothing there: the default microphone.
        let p = plan(Some("ghost.mic"), None, None, Some(&laptop(false)));
        assert_eq!(p.session, InputSpec::node(LAPTOP_MIC));
        assert_eq!(p.case, Case::Unprotected);
        assert!(p.summary.contains("may hear itself"));
        // A wake device that is gone: the session's.
        let p = plan(
            None,
            Some("alsa_input.unplugged#0"),
            None,
            Some(&array_desk(false)),
        );
        assert_eq!(p.wake, p.session);
        assert!(p.fell_back.is_some());
    }

    /// ADR 0064, ADR 0066: Repair sets the canceller up only where it is the right answer.
    #[test]
    fn repair_sets_the_canceller_up_only_where_it_belongs() {
        use CancellerStep::*;
        // A laptop and its speakers, nothing cancelling: the operator's Repair installs it,
        // an automatic repair never does.
        let bare = plan(None, None, None, Some(&laptop(false)));
        assert_eq!(canceller_step(&bare, false, true), Install);
        assert_eq!(canceller_step(&bare, false, false), Leave);
        // Running and listened through: restarted.
        let host = plan(None, None, None, Some(&laptop(true)));
        assert_eq!(canceller_step(&host, true, false), Restart);
        // Its source there but its unit gone (a reinstalled machine): set up again.
        assert_eq!(canceller_step(&host, false, true), Install);
        // A chip that cancels, or a headset: never.
        let chip = plan(None, None, None, Some(&array_desk(false)));
        assert_eq!(chip.case, Case::OnChip);
        assert_eq!(canceller_step(&chip, false, true), Leave);
        let head = plan(None, None, None, Some(&headset()));
        assert_eq!(head.case, Case::Headset);
        assert_eq!(canceller_step(&head, false, true), Leave);
    }

    #[test]
    fn a_laptop_uses_the_canceller_when_it_is_there_and_warns_when_it_is_not() {
        let p = plan(None, None, None, Some(&laptop(true)));
        assert_eq!(p.case, Case::HostCanceller);
        assert!(p.session.is_canceller());
        let p = plan(None, None, None, Some(&laptop(false)));
        assert_eq!(p.case, Case::Unprotected);
        assert_eq!(p.session, InputSpec::node(LAPTOP_MIC));
        assert!(p.advice.as_deref().unwrap().contains("Headphones help"));
        // The canceller there but the operator picked the raw microphone.
        let p = plan(
            Some("alsa_input.pci-0000_00_1f.3.analog-stereo"),
            None,
            None,
            Some(&laptop(true)),
        );
        assert_eq!(p.case, Case::Unprotected);
        assert_eq!(
            p.suggest,
            vec![("GHOST_INPUT_DEVICE".to_string(), "ghost.mic".to_string())]
        );
    }

    #[test]
    fn the_echo_warm_up_guard_is_kept_only_where_a_canceller_can_leak_at_the_onset() {
        // The array heard directly: its chip leaves nothing at Ghost's onset (2026-11-13).
        assert!(!plan(None, None, None, Some(&array_desk(false))).echo_warmup());
        // The same array under ghost.mic: the host canceller is what is heard, and it
        // leaks its first half second (#42).
        let p = plan(Some("ghost.mic"), None, None, Some(&array_desk(true)));
        assert_eq!(p.case, Case::OnChip);
        assert!(p.echo_warmup());
        // Ghost's voice in the operator's ears: nothing plays into the room.
        assert!(!plan(None, None, None, Some(&headset())).echo_warmup());
        assert!(!plan(None, None, None, Some(&puck())).echo_warmup());
        assert!(plan(None, None, None, Some(&laptop(true))).echo_warmup()); // host canceller
        assert!(plan(None, None, None, Some(&laptop(false))).echo_warmup()); // unprotected
        assert!(plan(None, None, None, Some(&webcam())).echo_warmup());
        assert!(plan(None, None, None, None).echo_warmup()); // unknown: as before
    }

    #[test]
    fn a_headset_a_puck_and_a_webcam_each_get_their_case() {
        let p = plan(None, None, None, Some(&headset()));
        assert_eq!(p.case, Case::Headset);
        assert_eq!(
            p.session,
            InputSpec::node("alsa_input.usb-Logitech_USB_Headset-00.mono-fallback")
        );
        let p = plan(None, None, None, Some(&puck()));
        assert_eq!(p.case, Case::OnChip);
        // One channel: the device itself, no channel picked.
        assert_eq!(p.session.channel, None);
        assert!(p.session.node.as_deref().unwrap().contains("Jabra"));
        let p = plan(None, None, None, Some(&webcam()));
        assert_eq!(p.case, Case::Unprotected);
        // The puck's microphone with Ghost on other speakers: its chip has no reference.
        let mut f = puck();
        f.nodes.push(node(
            "alsa_output.pci-0000_00_1f.3.analog-stereo",
            "Audio/Sink",
            Some("50"),
            2,
            &[],
        ));
        let p = plan(
            None,
            None,
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo"),
            Some(&f),
        );
        assert_eq!(p.case, Case::Unprotected);
    }

    #[test]
    fn music_on_another_output_is_named_since_the_chip_cant_cancel_it() {
        let mut f = array_desk(false);
        let p = plan(None, None, None, Some(&f));
        assert!(!p.summary.contains("also playing"));
        // The array playing Ghost is fine; desk speakers on another card playing music aren't.
        f.nodes
            .iter_mut()
            .for_each(|n| n.running = n.name == ARRAY_OUT);
        assert!(
            !plan(None, None, None, Some(&f))
                .summary
                .contains("also playing")
        );
        let mut speakers = node(
            "alsa_output.pci-0000_00_1f.3.analog-stereo",
            "Audio/Sink",
            Some("50"),
            2,
            &[],
        );
        speakers.description = "Desk speakers".into();
        speakers.running = true;
        f.nodes.push(speakers);
        let p = plan(None, None, None, Some(&f));
        assert_eq!(p.case, Case::OnChip);
        assert!(
            p.summary.contains("also playing on Desk speakers"),
            "{}",
            p.summary
        );
    }

    #[test]
    fn without_pipewire_the_settings_are_taken_as_given() {
        let p = plan(Some("ghost.mic"), None, None, None);
        assert_eq!(p.case, Case::Unknown);
        assert!(p.session.is_canceller() && p.wake.is_canceller());
        let p = plan(None, None, None, None);
        assert_eq!(p.session, InputSpec::default());
    }

    #[test]
    fn pw_dump_gives_the_nodes_and_the_defaults() {
        let json = format!(
            r#"[
            {{"id": 74, "type": "PipeWire:Interface:Node", "info": {{"props": {{"node.name": "{ARRAY_IN}", "media.class": "Audio/Source", "audio.channels": 2, "device.id": 66, "node.description": "reSpeaker XVF3800 4-Mic Array Digital Stereo (IEC958)"}}}}}},
            {{"id": 73, "type": "PipeWire:Interface:Node", "info": {{"props": {{"node.name": "{ARRAY_OUT}", "media.class": "Audio/Sink", "audio.channels": 2, "device.id": 66}}}}}},
            {{"id": 40, "type": "PipeWire:Interface:Node", "info": {{"props": {{"node.name": "ghost.mic.capture", "media.class": "Stream/Input/Audio", "node.target": "{ARRAY_IN}"}}}}}},
            {{"id": 41, "type": "PipeWire:Interface:Node", "info": {{"props": {{"node.name": "ghost.mic", "media.class": "Audio/Source", "audio.channels": 1}}}}}},
            {{"id": 45, "type": "PipeWire:Interface:Metadata", "props": {{"metadata.name": "default"}}, "metadata": [
                {{"subject": 0, "key": "default.audio.sink", "type": "Spa:String:JSON", "value": {{"name": "{ARRAY_OUT}"}}}},
                {{"subject": 0, "key": "default.audio.source", "type": "Spa:String:JSON", "value": {{"name": "{ARRAY_IN}"}}}}
            ]}}
        ]"#
        );
        let f = facts_from_pw_dump(&json).unwrap();
        assert_eq!(f.default_source.as_deref(), Some(ARRAY_IN));
        assert_eq!(f.default_sink.as_deref(), Some(ARRAY_OUT));
        assert_eq!(f.node(ARRAY_IN).unwrap().channels, 2);
        assert_eq!(f.canceller_mic().unwrap().name, ARRAY_IN);
        assert_eq!(
            plan(Some("ghost.mic"), None, None, Some(&f)).case,
            Case::OnChip
        );
        assert!(facts_from_pw_dump("not json").is_none());
    }

    #[test]
    fn the_meter_tells_a_chopped_link_from_a_gated_canceller_and_a_quiet_room() {
        let mut m = Meter::new(16_000);
        // Issue #26's stream: 5 samples of signal, 25 zeros, over and over.
        let chopped: Vec<i16> = (0..48_000)
            .map(|i| if i % 30 < 5 { 900 } else { 0 })
            .collect();
        let w = chopped.chunks(320).find_map(|f| m.push(f)).unwrap();
        assert!((w.zero_share - 25.0 / 30.0).abs() < 0.01);
        assert_eq!(w.median_run, 5);
        assert!(looks_chopped(&w));
        // A canceller's gate: a second of zeros, a second of signal.
        let gated: Vec<i16> = (0..48_000)
            .map(|i| if (i / 16_000) % 2 == 0 { 0 } else { 300 })
            .collect();
        let w = gated.chunks(320).find_map(|f| m.push(f)).unwrap();
        assert!(w.zero_share > 0.3);
        assert!(!looks_chopped(&w));
        // A quiet room with a little noise: no zeros to speak of.
        let room: Vec<i16> = (0..48_000).map(|i| ((i * 7919) % 11) as i16 - 5).collect();
        let w = room.chunks(320).find_map(|f| m.push(f)).unwrap();
        assert!(!looks_chopped(&w));
        assert!(w.dbfs < -60.0);
    }

    /// What the meter costs on the passive path (ADR 0018): `cargo test --release -p ghostd
    /// meter_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn meter_cost() {
        let room: Vec<i16> = (0..320)
            .map(|i| ((i * 7919) % 2001) as i16 - 1000)
            .collect();
        let mut m = Meter::new(16_000);
        let frames = 50 * 3600; // an hour of 20 ms frames
        let t = Instant::now();
        let mut windows = 0;
        for _ in 0..frames {
            windows += usize::from(m.push(std::hint::black_box(&room)).is_some());
        }
        let secs = t.elapsed().as_secs_f64();
        println!(
            "meter: an hour of audio in {:.1} ms ({windows} windows), {:.2} us per 20 ms frame",
            secs * 1e3,
            secs * 1e6 / frames as f64
        );
    }

    fn chopped_window() -> Window {
        Window {
            secs: 3.0,
            zero_share: 0.6,
            median_run: 5,
            dbfs: -30.0,
        }
    }

    fn clean_window() -> Window {
        Window {
            secs: 3.0,
            zero_share: 0.004,
            median_run: 4000,
            dbfs: -55.0,
        }
    }

    #[test]
    fn a_chopped_stream_is_a_problem_after_thirty_seconds_and_clears_after_thirty_clean() {
        let t0 = Instant::now();
        let mut h = Health::default();
        for i in 0..9 {
            h.window(&chopped_window(), t0 + Duration::from_secs(3 * i));
        }
        assert_eq!(
            h.problem(t0 + Duration::from_secs(27)),
            None,
            "27 s is not yet 30"
        );
        h.window(&chopped_window(), t0 + Duration::from_secs(27));
        assert!(matches!(
            h.problem(t0 + Duration::from_secs(30)),
            Some(Problem::Dropping { .. })
        ));
        // One clean window doesn't clear it; thirty seconds of them do.
        h.window(&clean_window(), t0 + Duration::from_secs(30));
        assert!(h.problem(t0 + Duration::from_secs(33)).is_some());
        for i in 1..10 {
            h.window(&clean_window(), t0 + Duration::from_secs(30 + 3 * i));
        }
        assert_eq!(h.problem(t0 + Duration::from_secs(60)), None);
        assert!(!h.auto_repair_due(t0), "a repair can't fix a link");
    }

    #[test]
    fn loud_stretches_read_as_nothing_make_ghost_deaf_and_a_reading_clears_it() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut h = Health::default();
        // Quiet windows, first reads mid-sentence: not counted.
        h.check(-50.0, 0, true, at(0));
        h.check(-25.0, 0, false, at(1));
        h.check(-25.0, 0, true, at(60));
        h.check(-23.0, 0, true, at(120));
        assert_eq!(h.problem(at(121)), None);
        h.check(-24.0, 0, true, at(180));
        assert_eq!(h.problem(at(181)), Some(Problem::Deaf { reads: 3 }));
        assert!(h.auto_repair_due(at(181)));
        // Twenty minutes on, the old reads have aged out.
        assert_eq!(h.problem(at(60 + 20 * 60 + 1)), None);
        // A loud stretch that reads words: hearing again.
        h.check(-25.0, 0, true, at(200));
        h.check(-25.0, 0, true, at(201));
        h.check(-25.0, 0, true, at(202));
        assert!(h.problem(at(203)).is_some());
        h.check(-22.0, 4, true, at(204));
        assert_eq!(h.problem(at(205)), None);
    }

    #[test]
    fn a_repair_clears_the_problem_and_an_automatic_one_waits_an_hour() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut h = Health::default();
        for s in 0..3 {
            h.check(-25.0, 0, true, at(s));
        }
        assert!(h.auto_repair_due(at(3)));
        h.repaired(at(3));
        assert_eq!(h.problem(at(4)), None);
        assert_eq!(h.repairs, 1);
        for s in 10..13 {
            h.check(-25.0, 0, true, at(s));
        }
        assert!(h.problem(at(13)).is_some());
        assert!(!h.auto_repair_due(at(13)), "within the hour");
        assert!(h.auto_repair_due(at(3 + 3600)) || h.problem(at(3 + 3600)).is_none());
    }

    #[test]
    fn the_note_says_what_is_wrong_and_the_one_thing_that_fixes_it() {
        let over_array = plan(Some("ghost.mic"), None, None, Some(&array_desk(true)));
        let deaf = note(&Problem::Deaf { reads: 4 }, &over_array);
        assert!(deaf.contains("4 loud stretches"));
        assert!(deaf.contains("restarts Ghost's echo canceller"));
        assert!(deaf.contains("directly"), "{deaf}");
        let direct = plan(None, None, None, Some(&array_desk(false)));
        let deaf = note(&Problem::Deaf { reads: 3 }, &direct);
        assert!(deaf.contains("Repair reopens the microphone."));
        assert!(!deaf.contains("canceller"));
        let drop = note(&Problem::Dropping { zero_share: 0.6 }, &direct);
        assert!(drop.contains("60%") && drop.contains("not a hub"));
    }

    #[test]
    fn pw_top_errors_are_read_from_the_last_snapshot() {
        let text = "\
S   ID  QUANT   RATE    WAIT    BUSY   W/Q   B/Q  ERR FORMAT           NAME
R   74    128  16000 814.4us   3.5us  0.10  0.00  14000    S16LE 2 16000 alsa_input.usb-array
R   41    480  48000  63.5us   3.9us  0.01  0.00  30000     F32P 1 16000  + ghost.mic
S   ID  QUANT   RATE    WAIT    BUSY   W/Q   B/Q  ERR FORMAT           NAME
R   74    128  16000 814.4us   3.5us  0.10  0.00  14206    S16LE 2 16000 alsa_input.usb-array
R   40    480  48000  55.1us   7.9us  0.01  0.00  8455     F32P 1 16000  + ghost.mic.capture
R   41    480  48000  63.5us   3.9us  0.01  0.00  30203     F32P 1 16000  + ghost.mic
S   31      0      0    ---     ---   ---   ---     0                  Freewheel-Driver
";
        let x = parse_pw_top(text);
        assert_eq!(x["ghost.mic"], 30203);
        assert_eq!(x["ghost.mic.capture"], 8455);
        assert_eq!(x["alsa_input.usb-array"], 14206);
    }
}
