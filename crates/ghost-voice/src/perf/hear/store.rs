//! Where the Hearing test keeps what it records (ADR 0042): `~/.local/share/ghost/hear/`.
//!
//! - `plan.json`: the operator's take list, once they edit it; the plan Ghost ships until then.
//! - `sessions/<date>[-n]/`: the takes, `<id>-<kind>.aec.wav` (`ghost.mic`, mono) and
//!   `<id>-<kind>.raw.wav` (the array under it), and `takes.jsonl`, the manifest the harness
//!   reads, as the terminal recorder wrote it.
//! - `runs/<stamp>/`: a score run's `results.json` (numbers only) and `report.md`.
//!
//! Nothing here holds what Ghost heard: a take's check lives on the screen only.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::Results;
use super::plan::{self, Block, Channel, Entry, MANIFEST, Plan, PlanTake};
use super::take::{Check, Ending, RATE};

pub const PLAN_FILE: &str = "plan.json";
pub const SESSIONS: &str = "sessions";
pub const RUNS: &str = "runs";
pub const RESULTS: &str = "results.json";
pub const REPORT: &str = "report.md";
/// A run's own facts beside its results: which session and channel, for the list.
pub const RUN_FILE: &str = "run.json";

/// A take just recorded, and its check. `raw`: the array's interleaved audio and its
/// channel count.
#[derive(Debug, Clone, Copy)]
pub struct Recorded<'a> {
    pub block: &'a Block,
    pub take: &'a PlanTake,
    pub aec: &'a [i16],
    pub raw: Option<(&'a [i16], u16)>,
    pub check: &'a Check,
    pub ending: Ending,
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// A session, for the list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionSummary {
    pub name: String,
    pub recorded: usize,
    pub skipped: usize,
    /// Takes with a raw-array file.
    pub raw: usize,
    /// Takes recorded with a warning that asks for a redo.
    pub flagged: usize,
    /// Seconds of audio.
    pub seconds: f64,
    /// The newest take's time.
    pub updated: Option<String>,
}

/// A score run, for the list: `run.json`.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct RunInfo {
    pub id: String,
    pub session: String,
    /// `aec` or `raw`.
    pub channel: String,
    pub created: String,
    pub commit: String,
    pub takes: usize,
    /// Word error rate of the reads, by mode; None where nothing was spoken.
    #[serde(default)]
    pub wer: std::collections::BTreeMap<String, Option<f64>>,
    #[serde(default)]
    pub note: String,
}

/// A name the store made, checked before it becomes a path: a session's or a run's.
fn safe(name: &str) -> anyhow::Result<&str> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    anyhow::ensure!(ok, "{name:?} is not a session or run name");
    Ok(name)
}

pub fn take_files(take: &PlanTake) -> (String, String) {
    let stem = format!("{}-{}", take.id, take.kind.name());
    (format!("{stem}.aec.wav"), format!("{stem}.raw.wav"))
}

fn write_wav(path: &Path, pcm: &[i16], channels: u16) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let tmp = path.with_extension("wav.part");
    {
        let mut w = hound::WavWriter::create(&tmp, spec)?;
        for &s in pcm {
            w.write_sample(s)?;
        }
        w.finalize()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<data>/hear`, under Ghost's data directory.
    pub fn under(data_dir: &Path) -> Self {
        Self::new(data_dir.join("hear"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join(SESSIONS)
    }

    // --- the plan --------------------------------------------------------------------------

    /// The operator's plan, or the one Ghost ships; and whether it is the operator's.
    pub fn plan(&self) -> anyhow::Result<(Plan, bool)> {
        let path = self.root.join(PLAN_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let p: Plan = serde_json::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                Ok((p, true))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((plan::plan()?, false)),
            Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
        }
    }

    /// Save the operator's plan, checked first; the error is the operator's to read.
    pub fn save_plan(&self, p: &Plan) -> Result<(), String> {
        plan::validate(p)?;
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let path = self.root.join(PLAN_FILE);
        let tmp = path.with_extension("json.part");
        let text = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text + "\n").map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Back to the plan Ghost ships.
    pub fn reset_plan(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(self.root.join(PLAN_FILE)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    // --- sessions --------------------------------------------------------------------------

    pub fn session_dir(&self, name: &str) -> anyhow::Result<PathBuf> {
        Ok(self.sessions_dir().join(safe(name)?))
    }

    /// A new session, named for the day: `2026-11-06`, then `2026-11-06-2`.
    pub fn new_session(&self, today: chrono::NaiveDate) -> anyhow::Result<String> {
        let base = today.format("%Y-%m-%d").to_string();
        std::fs::create_dir_all(self.sessions_dir())?;
        for n in 1..1000 {
            let name = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let dir = self.sessions_dir().join(&name);
            if std::fs::create_dir(&dir).is_ok() {
                return Ok(name);
            }
        }
        anyhow::bail!("no free session name for {base}")
    }

    /// The newest session from `today`, to carry on.
    pub fn todays_session(&self, today: chrono::NaiveDate) -> Option<String> {
        let day = today.format("%Y-%m-%d").to_string();
        self.sessions()
            .into_iter()
            .map(|s| s.name)
            .filter(|n| n == &day || n.starts_with(&format!("{day}-")))
            .max_by_key(|n| {
                n.strip_prefix(&day)
                    .and_then(|r| r.strip_prefix('-'))
                    .and_then(|r| r.parse::<u32>().ok())
                    .unwrap_or(1)
            })
    }

    /// The manifest's entries, a take's last line winning.
    pub fn entries(&self, name: &str) -> anyhow::Result<Vec<Entry>> {
        let dir = self.session_dir(name)?;
        anyhow::ensure!(dir.is_dir(), "no session {name}");
        match std::fs::read_to_string(dir.join(MANIFEST)) {
            Ok(text) => Ok(plan::read_manifest(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn append(&self, name: &str, e: &Entry) -> anyhow::Result<()> {
        use std::io::Write;
        let path = self.session_dir(name)?.join(MANIFEST);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{}", serde_json::to_string(e)?)?;
        Ok(())
    }

    fn entry(block: &Block, take: &PlanTake) -> Entry {
        Entry {
            id: take.id.clone(),
            file: None,
            raw: None,
            block: block.block.clone(),
            condition: block.condition.clone(),
            kind: take.kind,
            text: take.text.clone(),
            tags: take.tags.clone(),
            terms: take.terms.clone(),
            skipped: false,
            seconds: None,
            rms: None,
            peak: None,
            speech: None,
            ending: None,
            warnings: Vec::new(),
            time: Some(now()),
        }
    }

    /// Keep a take: its audio, and its line in the manifest. A redo replaces the files;
    /// the manifest's last line wins.
    pub fn save_take(&self, name: &str, t: &Recorded) -> anyhow::Result<Entry> {
        let Recorded {
            block,
            take,
            aec,
            raw,
            check,
            ending,
        } = *t;
        let dir = self.session_dir(name)?;
        std::fs::create_dir_all(&dir)?;
        let (aec_name, raw_name) = take_files(take);
        write_wav(&dir.join(&aec_name), aec, 1)?;
        let raw_file = match raw {
            Some((pcm, channels)) if !pcm.is_empty() => {
                write_wav(&dir.join(&raw_name), pcm, channels)?;
                Some(raw_name)
            }
            _ => {
                let _ = std::fs::remove_file(dir.join(&raw_name)); // a redo keeps no old one
                None
            }
        };
        let mut e = Self::entry(block, take);
        e.file = Some(aec_name);
        e.raw = raw_file;
        e.seconds = Some((check.seconds * 100.0).round() / 100.0);
        e.rms = Some((check.rms * 10.0).round() / 10.0);
        e.peak = Some((check.peak * 10.0).round() / 10.0);
        e.speech = check
            .speech
            .map(|(a, b)| ((a * 100.0).round() / 100.0, (b * 100.0).round() / 100.0));
        e.ending = Some(
            match ending {
                Ending::Hold => "hold",
                Ending::Quiet => "quiet",
                Ending::Timed { .. } => "timed",
            }
            .into(),
        );
        e.warnings = check
            .warnings
            .iter()
            .map(|w| {
                serde_json::to_value(w)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(String::from))
                    .unwrap_or_default()
            })
            .collect();
        self.append(name, &e)?;
        Ok(e)
    }

    /// Skip a take: a manifest line, no audio.
    pub fn skip(&self, name: &str, block: &Block, take: &PlanTake) -> anyhow::Result<Entry> {
        let dir = self.session_dir(name)?;
        let (aec, raw) = take_files(take);
        let _ = std::fs::remove_file(dir.join(aec));
        let _ = std::fs::remove_file(dir.join(raw));
        let mut e = Self::entry(block, take);
        e.skipped = true;
        self.append(name, &e)?;
        Ok(e)
    }

    /// A take's file on one channel, if it was recorded.
    pub fn take_file(
        &self,
        name: &str,
        id: &str,
        channel: Channel,
    ) -> anyhow::Result<Option<PathBuf>> {
        let dir = self.session_dir(name)?;
        let Some(e) = self.entries(name)?.into_iter().find(|e| e.id == id) else {
            return Ok(None);
        };
        let file = match channel {
            Channel::Aec => e.file,
            Channel::Raw => e.raw,
        };
        Ok(file.map(|f| dir.join(f)).filter(|p| p.is_file()))
    }

    pub fn sessions(&self) -> Vec<SessionSummary> {
        let Ok(dir) = std::fs::read_dir(self.sessions_dir()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = dir
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| safe(n).is_ok())
            .collect();
        names.sort();
        names.reverse();
        names
            .into_iter()
            .map(|name| {
                let entries = self.entries(&name).unwrap_or_default();
                let recorded: Vec<&Entry> = entries.iter().filter(|e| !e.skipped).collect();
                SessionSummary {
                    recorded: recorded.len(),
                    skipped: entries.len() - recorded.len(),
                    raw: recorded.iter().filter(|e| e.raw.is_some()).count(),
                    flagged: recorded
                        .iter()
                        .filter(|e| {
                            e.warnings.iter().any(|w| {
                                matches!(
                                    w.as_str(),
                                    "no_speech"
                                        | "started_early"
                                        | "cut_off"
                                        | "clipping"
                                        | "speech_in_silence"
                                )
                            })
                        })
                        .count(),
                    seconds: recorded.iter().filter_map(|e| e.seconds).sum(),
                    updated: entries.iter().filter_map(|e| e.time.clone()).max(),
                    name,
                }
            })
            .collect()
    }

    pub fn delete_session(&self, name: &str) -> anyhow::Result<()> {
        let dir = self.session_dir(name)?;
        anyhow::ensure!(dir.is_dir(), "no session {name}");
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// The newest session with a take in it.
    pub fn latest(&self) -> Option<PathBuf> {
        plan::latest_session(&self.sessions_dir())
    }

    // --- runs ------------------------------------------------------------------------------

    pub fn run_dir(&self, id: &str) -> anyhow::Result<PathBuf> {
        Ok(self.root.join(RUNS).join(safe(id)?))
    }

    /// Keep a run: its results, its report and its facts.
    pub fn save_run(&self, info: &RunInfo, results: &Results, report: &str) -> anyhow::Result<()> {
        let dir = self.run_dir(&info.id)?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(RESULTS), serde_json::to_string_pretty(results)?)?;
        std::fs::write(dir.join(REPORT), report)?;
        std::fs::write(dir.join(RUN_FILE), serde_json::to_string_pretty(info)?)?;
        Ok(())
    }

    /// A free run id for now: `20261106-0915`, then `-2`.
    pub fn new_run_id(&self, at: chrono::NaiveDateTime) -> anyhow::Result<String> {
        let base = at.format("%Y%m%d-%H%M%S").to_string();
        for n in 1..1000 {
            let id = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            if !self.run_dir(&id)?.exists() {
                return Ok(id);
            }
        }
        anyhow::bail!("no free run id for {base}")
    }

    pub fn runs(&self) -> Vec<RunInfo> {
        let Ok(dir) = std::fs::read_dir(self.root.join(RUNS)) else {
            return Vec::new();
        };
        let mut runs: Vec<RunInfo> = dir
            .filter_map(|e| e.ok())
            .filter_map(|e| std::fs::read_to_string(e.path().join(RUN_FILE)).ok())
            .filter_map(|t| serde_json::from_str(&t).ok())
            .collect();
        runs.sort_by(|a, b| b.id.cmp(&a.id));
        runs
    }

    pub fn run(&self, id: &str) -> anyhow::Result<(RunInfo, Results)> {
        let dir = self.run_dir(id)?;
        let info: RunInfo = serde_json::from_str(
            &std::fs::read_to_string(dir.join(RUN_FILE))
                .map_err(|_| anyhow::anyhow!("no run {id}"))?,
        )?;
        Ok((info, super::read_results(&dir.join(RESULTS))?))
    }

    pub fn set_run_note(&self, id: &str, note: &str) -> anyhow::Result<RunInfo> {
        let (mut info, _) = self.run(id)?;
        info.note = note.chars().take(500).collect();
        std::fs::write(
            self.run_dir(id)?.join(RUN_FILE),
            serde_json::to_string_pretty(&info)?,
        )?;
        Ok(info)
    }

    pub fn delete_run(&self, id: &str) -> anyhow::Result<()> {
        let dir = self.run_dir(id)?;
        anyhow::ensure!(dir.is_dir(), "no run {id}");
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf::hear::plan::Kind;
    use crate::perf::hear::take::check;

    fn plan_take(id: &str) -> (Block, PlanTake) {
        let p = plan::plan().unwrap();
        let (b, t) = plan::takes(&p).find(|(_, t)| t.id == id).unwrap();
        (b.clone(), t.clone())
    }

    #[test]
    fn the_shipped_plan_until_the_operator_saves_one_and_a_bad_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let (p, mine) = store.plan().unwrap();
        assert!(!mine);
        let mut edited = p.clone();
        edited.blocks[0].takes[0].text = "What needs me today?".into();
        store.save_plan(&edited).unwrap();
        assert_eq!(store.plan().unwrap(), (edited.clone(), true));
        let mut bad = edited.clone();
        bad.blocks[0].takes[1].id = bad.blocks[0].takes[0].id.clone();
        assert!(store.save_plan(&bad).unwrap_err().contains("twice"));
        store.reset_plan().unwrap();
        assert!(!store.plan().unwrap().1);
    }

    #[test]
    fn a_take_is_kept_with_its_manifest_line_and_a_redo_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        let name = store.new_session(day).unwrap();
        assert_eq!(name, "2026-11-06");
        assert_eq!(store.todays_session(day).as_deref(), Some("2026-11-06"));
        assert_eq!(store.new_session(day).unwrap(), "2026-11-06-2");
        let (b, t) = plan_take("A02");
        let pcm = vec![1000i16; 16_000];
        let c = check(
            Kind::Speech,
            &t.text,
            &pcm,
            Some((0.6, 0.95)),
            "read me issue 403",
        );
        let raw: Vec<i16> = vec![5; 32_000];
        store
            .save_take(
                &name,
                &Recorded {
                    block: &b,
                    take: &t,
                    aec: &pcm,
                    raw: Some((&raw, 2)),
                    check: &c,
                    ending: Ending::Hold,
                },
            )
            .unwrap();
        let e = &store.entries(&name).unwrap()[0];
        assert_eq!(e.file.as_deref(), Some("A02-speech.aec.wav"));
        assert_eq!(e.raw.as_deref(), Some("A02-speech.raw.wav"));
        assert_eq!(e.ending.as_deref(), Some("hold"));
        assert_eq!(e.text, t.text);
        // the manifest holds the line asked for, never what was heard
        let manifest =
            std::fs::read_to_string(store.session_dir(&name).unwrap().join(MANIFEST)).unwrap();
        assert!(!manifest.contains("403"), "{manifest}");
        // the harness reads it, either channel
        let (items, missing) =
            plan::recorded(&store.session_dir(&name).unwrap(), Channel::Raw, &[]).unwrap();
        assert_eq!((items.len(), missing.len()), (1, 0));
        assert_eq!(items[0].pcm.len(), 16_000);

        // a redo without the raw array drops the old raw file
        store
            .save_take(
                &name,
                &Recorded {
                    block: &b,
                    take: &t,
                    aec: &pcm,
                    raw: None,
                    check: &c,
                    ending: Ending::Hold,
                },
            )
            .unwrap();
        assert_eq!(store.entries(&name).unwrap()[0].raw, None);
        assert!(
            store
                .take_file(&name, "A02", Channel::Raw)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .take_file(&name, "A02", Channel::Aec)
                .unwrap()
                .is_some()
        );

        let s = &store.sessions()[1];
        assert_eq!(
            (s.name.as_str(), s.recorded, s.skipped),
            ("2026-11-06", 1, 0)
        );
        store.skip(&name, &b, &t).unwrap();
        assert_eq!(store.sessions()[1].skipped, 1);
        assert!(
            store
                .take_file(&name, "A02", Channel::Aec)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn opening_carries_on_todays_newest_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        assert_eq!(store.todays_session(day), None);
        for n in 1..=10 {
            store.new_session(day).unwrap();
            let want = if n == 1 {
                "2026-11-06".to_string()
            } else {
                format!("2026-11-06-{n}")
            };
            assert_eq!(store.todays_session(day), Some(want));
        }
        assert_eq!(store.todays_session(day.succ_opt().unwrap()), None);
    }

    #[test]
    fn names_from_outside_never_leave_the_store() {
        let store = Store::new("/tmp/nowhere");
        for bad in ["../x", "a/b", "", ".hidden", "a b"] {
            assert!(store.session_dir(bad).is_err(), "{bad}");
            assert!(store.run_dir(bad).is_err(), "{bad}");
        }
        assert!(store.session_dir("2026-11-06-2").is_ok());
    }

    #[test]
    fn runs_list_newest_first_with_their_facts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let at = chrono::NaiveDate::from_ymd_opt(2026, 11, 6)
            .unwrap()
            .and_hms_opt(9, 15, 0)
            .unwrap();
        let results = Results {
            meta: Default::default(),
            takes: Vec::new(),
        };
        for _ in 0..2 {
            let id = store.new_run_id(at).unwrap();
            let info = RunInfo {
                id: id.clone(),
                session: "2026-11-06".into(),
                channel: "aec".into(),
                created: "now".into(),
                commit: "abc".into(),
                takes: 0,
                wer: Default::default(),
                note: String::new(),
            };
            store.save_run(&info, &results, "# report").unwrap();
        }
        let runs = store.runs();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "20261106-091500-2");
        store
            .set_run_note(&runs[0].id, "before the turn re-read")
            .unwrap();
        assert_eq!(
            store.run(&runs[0].id).unwrap().0.note,
            "before the turn re-read"
        );
        store.delete_run(&runs[1].id).unwrap();
        assert_eq!(store.runs().len(), 1);
    }
}
