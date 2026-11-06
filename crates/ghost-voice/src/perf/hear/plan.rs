//! The hear test set (`docs/benchmarks/hear-plan.json`) and the takes recorded from it.
//!
//! The plan is the list of lines, block by block; `scripts/record-hear-takes.py` records
//! the operator saying them into a session directory with a `takes.jsonl` manifest, and
//! the same plan rendered by Kokoro is the set the harness runs without the operator.
//! Every take carries its own reference text and terms in the manifest, so a session
//! recorded under an older plan is still scored on what was asked then.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const PLAN_JSON: &str = include_str!("../../../../../docs/benchmarks/hear-plan.json");
pub const MANIFEST: &str = "takes.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A request, scored by its word error rate.
    Speech,
    /// A word or two Ghost must not drop: scored the same, and its empty reads apart.
    Reply,
    /// Nothing said: every word heard is made up.
    Silence,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Speech => "speech",
            Kind::Reply => "reply",
            Kind::Silence => "silence",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanTake {
    pub id: String,
    pub kind: Kind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Block {
    pub block: String,
    pub condition: String,
    #[serde(default)]
    pub away: bool,
    #[serde(default)]
    pub music: bool,
    pub takes: Vec<PlanTake>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Plan {
    pub version: u32,
    pub blocks: Vec<Block>,
}

pub fn plan() -> anyhow::Result<Plan> {
    Ok(serde_json::from_str(PLAN_JSON)?)
}

/// One take to score: where it came from, what was said, and its audio.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub block: String,
    pub condition: String,
    pub kind: Kind,
    pub text: String,
    pub tags: Vec<String>,
    pub terms: Vec<String>,
    /// `take` for a recording, `kokoro:<voice>` for a rendered line.
    pub source: String,
    /// 16 kHz mono int16.
    pub pcm: Vec<i16>,
}

impl Item {
    /// The key a baseline is paired on.
    pub fn key(&self) -> String {
        format!("{}|{}", self.id, self.source)
    }

    pub fn seconds(&self) -> f64 {
        self.pcm.len() as f64 / 16_000.0
    }
}

/// A line of `takes.jsonl` (the recorder's manifest).
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub raw: Option<String>,
    pub block: String,
    pub condition: String,
    pub kind: Kind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default)]
    pub skipped: bool,
}

/// The manifest's entries, the last line for a take winning; a torn line is skipped.
pub fn read_manifest(text: &str) -> Vec<Entry> {
    let mut by_id: indexmap::IndexMap<String, Entry> = indexmap::IndexMap::new();
    for line in text.lines() {
        if let Ok(e) = serde_json::from_str::<Entry>(line) {
            by_id.shift_remove(&e.id);
            by_id.insert(e.id.clone(), e);
        }
    }
    let mut out: Vec<Entry> = by_id.into_values().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Which microphone channel of a take is scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// `ghost.mic`, what Ghost hears today.
    Aec,
    /// The array's own first channel under it (issue #52 item 2).
    Raw,
}

impl Channel {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "aec" | "ghost.mic" => Ok(Channel::Aec),
            "raw" => Ok(Channel::Raw),
            other => Err(format!("--channel is aec or raw, not {other:?}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::Aec => "ghost.mic",
            Channel::Raw => "raw array, channel 1",
        }
    }
}

/// A WAV as 16 kHz mono int16: the first channel of a stereo file.
pub fn read_mono(path: &Path) -> anyhow::Result<Vec<i16>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.sample_rate == 16_000 && spec.bits_per_sample == 16,
        "{}: {} Hz {}-bit, not 16 kHz 16-bit",
        path.display(),
        spec.sample_rate,
        spec.bits_per_sample
    );
    let channels = usize::from(spec.channels.max(1));
    let samples: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>()?;
    Ok(samples.into_iter().step_by(channels).collect())
}

/// A recorded session's takes, in id order, from `dir/takes.jsonl`.
pub fn recorded(
    dir: &Path,
    channel: Channel,
    blocks: &[String],
) -> anyhow::Result<(Vec<Item>, Vec<String>)> {
    let text = std::fs::read_to_string(dir.join(MANIFEST))
        .map_err(|e| anyhow::anyhow!("{}: {e}", dir.join(MANIFEST).display()))?;
    let mut items = Vec::new();
    let mut missing = Vec::new();
    for e in read_manifest(&text) {
        if e.skipped || (!blocks.is_empty() && !blocks.contains(&e.block)) {
            continue;
        }
        let file = match channel {
            Channel::Aec => e.file.clone(),
            Channel::Raw => e.raw.clone(),
        };
        let Some(file) = file else {
            missing.push(e.id.clone());
            continue;
        };
        let pcm = read_mono(&dir.join(&file))?;
        items.push(Item {
            id: e.id,
            block: e.block,
            condition: e.condition,
            kind: e.kind,
            text: e.text,
            tags: e.tags,
            terms: e.terms,
            source: "take".into(),
            pcm,
        });
    }
    Ok((items, missing))
}

/// The newest session under `root` (`.scratch/hear-takes/<date>`), for `--takes latest`.
pub fn latest_session(root: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(MANIFEST).is_file())
        .collect();
    dirs.sort();
    dirs.pop()
}

/// The plan's takes Kokoro can stand in for: the desk blocks. Distance and music are the
/// room's, so those blocks are left to the recordings.
pub fn renderable(plan: &Plan, blocks: &[String]) -> Vec<(Block, PlanTake)> {
    plan.blocks
        .iter()
        .filter(|b| !b.away && !b.music)
        .filter(|b| blocks.is_empty() || blocks.contains(&b.block))
        .flat_map(|b| b.takes.iter().map(move |t| (b.clone(), t.clone())))
        .collect()
}

/// Deterministic noise at `dbfs`, `secs` long: the room under a rendered line, and a
/// rendered silence take. Never digital zero, which no microphone gives.
pub fn room_noise(secs: f64, dbfs: f64, seed: u32) -> Vec<i16> {
    let n = (secs * 16_000.0) as usize;
    let amplitude = 10f64.powf(dbfs / 20.0) * 3f64.sqrt() * 32767.0;
    let mut state = seed.max(1);
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let unit = f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0;
            (unit * amplitude) as i16
        })
        .collect()
}

/// A rendered line with the room around it: `lead` seconds before, `tail` after.
pub fn in_room(pcm: &[i16], lead: f64, tail: f64, dbfs: f64, seed: u32) -> Vec<i16> {
    let before = room_noise(lead, dbfs, seed);
    let after = room_noise(tail, dbfs, seed.wrapping_add(1));
    let under = room_noise(
        pcm.len() as f64 / 16_000.0 + 0.01,
        dbfs,
        seed.wrapping_add(2),
    );
    let mut out = before;
    out.extend(
        pcm.iter()
            .zip(under.iter().chain(std::iter::repeat(&0)))
            .map(|(&s, &n)| s.saturating_add(n)),
    );
    out.extend(after);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_plan_parses_with_unique_ids() {
        let p = plan().unwrap();
        assert_eq!(p.version, 1);
        let mut ids: Vec<&str> = p
            .blocks
            .iter()
            .flat_map(|b| b.takes.iter().map(|t| t.id.as_str()))
            .collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "take ids are unique");
        for b in &p.blocks {
            for t in &b.takes {
                assert!(
                    t.id.starts_with(&b.block),
                    "{} is in block {}",
                    t.id,
                    b.block
                );
                match t.kind {
                    Kind::Silence => assert!(t.text.is_empty(), "{} says nothing", t.id),
                    _ => assert!(!t.text.is_empty(), "{} has a line", t.id),
                }
                for term in &t.terms {
                    assert!(
                        super::super::score::normalize(&t.text)
                            .windows(super::super::score::normalize(term).len())
                            .any(|w| w == &super::super::score::normalize(term)[..]),
                        "{}: term {term:?} is in its line",
                        t.id
                    );
                }
            }
        }
    }

    #[test]
    fn the_plan_covers_what_issue_60_measures() {
        let p = plan().unwrap();
        let takes: Vec<&PlanTake> = p.blocks.iter().flat_map(|b| &b.takes).collect();
        let tagged = |tag: &str| {
            takes
                .iter()
                .filter(|t| t.tags.iter().any(|x| x == tag))
                .count()
        };
        assert!(tagged("pause") >= 8, "pauses mid-sentence (#52 item 1)");
        assert!(tagged("numbers") >= 8);
        assert!(
            takes.iter().filter(|t| t.kind == Kind::Reply).count() >= 8,
            "short replies"
        );
        assert!(
            takes.iter().filter(|t| t.kind == Kind::Silence).count() >= 10,
            "silence"
        );
        assert!(tagged("fillers") >= 3, "fillers (#55)");
    }

    #[test]
    fn the_manifest_keeps_the_last_line_per_take_and_skips_torn_ones() {
        let text = concat!(
            r#"{"id":"A01","file":"A01.aec.wav","block":"A","condition":"desk","kind":"speech","text":"old"}"#,
            "\n",
            r#"{"id":"A02","file":null,"block":"A","condition":"desk","kind":"speech","text":"x","skipped":true}"#,
            "\n{\"id\":\"A0",
            "\n",
            r#"{"id":"A01","file":"A01.aec.wav","raw":"A01.raw.wav","block":"A","condition":"desk","kind":"speech","text":"new","terms":["GitHub"]}"#,
            "\n"
        );
        let e = read_manifest(text);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].id, "A01");
        assert_eq!(e[0].text, "new");
        assert_eq!(e[0].raw.as_deref(), Some("A01.raw.wav"));
        assert!(e[1].skipped);
    }

    #[test]
    fn rendered_takes_leave_out_distance_and_music() {
        let p = plan().unwrap();
        let r = renderable(&p, &[]);
        assert!(r.iter().all(|(b, _)| !b.away && !b.music));
        assert!(r.iter().any(|(b, _)| b.block == "B"));
        let only_d = renderable(&p, &["D".to_string()]);
        assert!(only_d.iter().all(|(b, _)| b.block == "D"));
    }

    #[test]
    fn room_noise_is_quiet_deterministic_and_never_zero() {
        let a = room_noise(1.0, -60.0, 3);
        assert_eq!(a, room_noise(1.0, -60.0, 3));
        assert_eq!(a.len(), 16_000);
        let rms = (a.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / a.len() as f64).sqrt();
        let dbfs = 20.0 * (rms / 32768.0).log10();
        assert!((dbfs + 60.0).abs() < 1.5, "{dbfs}");
        assert!(a.iter().any(|&s| s != 0));
    }
}
