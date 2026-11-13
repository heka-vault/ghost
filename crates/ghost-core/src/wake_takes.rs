//! Kept wake takes (ADR 0064): the sound of each time Ghost thought it heard its name, kept
//! on this computer so the wake detector can be retrained on the operator's own voice
//! through their own microphone.
//!
//! On by default, one switch to turn it off (`Choice::keep`). A take is kept when the
//! trained detector's hit is decided: woken (`Outcome::Accepted`) or turned down by
//! Parakeet's check (`Outcome::Refused`). Nothing else passive is kept: no audio between
//! hits, no transcript of the room. Each take is
//!
//! - `<id>.wav`: the stream the detector heard, from `PRE_ACCEPTED_SECS` (an accepted wake:
//!   long enough to hold an earlier attempt the detector missed) or `PRE_REFUSED_SECS`
//!   before the frame that scored to `POST_SECS` after it;
//! - `<id>.array.wav`, when the array's own stream is captured beside the echo-cancelled
//!   one: the same moment from it;
//! - `<id>.json`: the score, the model, the input, the time, the detector's score every
//!   80 ms over the take, what Parakeet read for it, and the operator's label.
//!
//! Files are 0600 in a 0700 folder under Ghost's data folder, capped by count and bytes,
//! the oldest removed first. Nothing here sends anything anywhere: the control API lists
//! the takes (no audio), labels and deletes them, and plays one through Ghost's own speaker
//! on the desktop's request; no tool of any app, agent or outside client reaches them.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::time::iso;
use crate::waiting::Items;

/// The store setting the choice is kept under.
pub const KEY: &str = "wake_takes";
/// The take files' folder under Ghost's data folder.
pub const FOLDER: &str = "wake/takes";
pub const RATE: u32 = 16_000;
/// Kept before the frame that scored, for a wake Ghost accepted: 12 s holds an attempt the
/// detector missed just before (the operator says it again, louder).
pub const PRE_ACCEPTED_SECS: f64 = 12.0;
/// Kept before the frame that scored, for a hit Parakeet turned down.
pub const PRE_REFUSED_SECS: f64 = 4.0;
/// Kept after it: the end of "ghost" and a breath.
pub const POST_SECS: f64 = 1.5;
/// The caps: about 1.3 MB for an accepted take with the array beside it, 0.5 MB refused.
pub const MAX_TAKES: usize = 2000;
pub const MAX_BYTES: u64 = 1 << 30;
/// What Parakeet read, at most this many characters.
pub const MAX_READING: usize = 400;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The operator's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Keep wake takes. On unless the operator turned it off.
    pub keep: bool,
    /// The retraining counts an accepted wake with no label as the operator calling Ghost.
    #[serde(default)]
    pub unlabelled_accepted_are_me: bool,
}

impl Default for Choice {
    fn default() -> Self {
        Self {
            keep: true,
            unlabelled_accepted_are_me: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Ghost woke.
    Accepted,
    /// The detector scored it, Parakeet's check turned it down.
    Refused,
}

/// The operator's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    /// Me calling Ghost.
    Me,
    /// Not a wake.
    Not,
    /// Me calling Ghost, and I had called it just before and it missed (an accepted take
    /// holds the 12 s before: the retraining looks there for the missed attempt).
    MeMissedBefore,
}

/// A take's metadata: everything but the audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeMeta {
    pub id: String,
    #[serde(with = "crate::time::json")]
    pub at: DateTime<Utc>,
    pub outcome: Outcome,
    pub model: String,
    pub score: f32,
    pub threshold: f32,
    /// The input the detector heard (`ghost.mic`, a device's name).
    pub input: String,
    /// The array's own stream kept beside it, by its node's name.
    #[serde(default)]
    pub array: Option<String>,
    #[serde(default)]
    pub array_channels: u16,
    /// Seconds of audio, and where in it the frame that scored ended.
    pub secs: f64,
    pub hit_at: f64,
    /// The detector's score every 80 ms, the last ending at `scores_end` seconds.
    #[serde(default)]
    pub scores: Vec<f32>,
    #[serde(default)]
    pub scores_end: f64,
    /// What Parakeet read for it, when it read it.
    #[serde(default)]
    pub reading: Option<String>,
    #[serde(default)]
    pub label: Option<Label>,
    /// Bytes on disk, every file of the take.
    #[serde(default)]
    pub bytes: u64,
}

/// A take as the wake listener hands it over.
#[derive(Debug, Clone, Default)]
pub struct NewTake {
    pub audio: Vec<i16>,
    /// Sample index into `audio` where the frame that scored ended.
    pub hit: usize,
    pub accepted: bool,
    pub model: String,
    pub score: f32,
    pub threshold: f32,
    pub input: String,
    pub reading: Option<String>,
    /// Scores every 80 ms; the last one's window ended at sample `scores_end` of `audio`.
    pub scores: Vec<f32>,
    pub scores_end: usize,
    /// The array's stream over the same moment, interleaved, with its channel count.
    pub array: Option<(String, u16, Vec<i16>)>,
}

/// How much is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub takes: usize,
    pub bytes: u64,
}

pub struct Takes {
    dir: PathBuf,
    choice: Mutex<Choice>,
    items: OnceLock<Items>,
    max_takes: usize,
    max_bytes: u64,
    /// One writer at a time: keep, label, delete.
    write: Mutex<()>,
}

impl Takes {
    pub fn new(data_dir: &Path) -> Self {
        Self::with_caps(data_dir.join(FOLDER), MAX_TAKES, MAX_BYTES)
    }

    pub fn with_caps(dir: PathBuf, max_takes: usize, max_bytes: u64) -> Self {
        Self {
            dir,
            choice: Mutex::new(Choice::default()),
            items: OnceLock::new(),
            max_takes,
            max_bytes,
            write: Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads the choice back from the store and keeps it there from now on.
    pub fn attach(&self, items: Items) -> crate::waiting::Result<Choice> {
        let stored = items
            .setting(KEY)?
            .and_then(|v| serde_json::from_str::<Choice>(&v).ok())
            .unwrap_or_default();
        let _ = self.items.set(items);
        *lock(&self.choice) = stored;
        Ok(stored)
    }

    pub fn choice(&self) -> Choice {
        *lock(&self.choice)
    }

    pub fn set_choice(&self, c: Choice) -> crate::waiting::Result<Choice> {
        if let Some(items) = self.items.get() {
            items.set_setting(KEY, &serde_json::to_string(&c).unwrap_or_default())?;
        }
        *lock(&self.choice) = c;
        Ok(c)
    }

    /// Whether a take would be kept now: the listener asks before it copies any audio.
    pub fn wants(&self) -> bool {
        self.choice().keep
    }

    fn ensure_dir(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))
    }

    /// Keep a take, when the choice is on; then prune to the caps, oldest first.
    pub fn keep(&self, t: NewTake) -> io::Result<Option<TakeMeta>> {
        self.keep_at(t, Utc::now())
    }

    pub fn keep_at(&self, t: NewTake, at: DateTime<Utc>) -> io::Result<Option<TakeMeta>> {
        if !self.wants() || t.audio.is_empty() {
            return Ok(None);
        }
        let _g = lock(&self.write);
        self.ensure_dir()?;
        let outcome = if t.accepted {
            Outcome::Accepted
        } else {
            Outcome::Refused
        };
        let mut id = format!(
            "{}-{}",
            at.format("%Y%m%dT%H%M%S%3f"),
            match outcome {
                Outcome::Accepted => "accepted",
                Outcome::Refused => "refused",
            }
        );
        let mut n = 1;
        while self.dir.join(format!("{id}.json")).exists() {
            n += 1;
            id = format!("{id}-{n}");
        }
        let rate = f64::from(RATE);
        let mut bytes = write_wav(&self.dir.join(format!("{id}.wav")), &t.audio, 1)?;
        let (array, array_channels) = match &t.array {
            Some((name, ch, pcm)) if !pcm.is_empty() && *ch > 0 => {
                bytes += write_wav(&self.dir.join(format!("{id}.array.wav")), pcm, *ch)?;
                (Some(name.clone()), *ch)
            }
            _ => (None, 0),
        };
        let reading = t
            .reading
            .map(|r| r.chars().take(MAX_READING).collect::<String>());
        let mut meta = TakeMeta {
            id: id.clone(),
            at,
            outcome,
            model: t.model,
            score: t.score,
            threshold: t.threshold,
            input: t.input,
            array,
            array_channels,
            secs: t.audio.len() as f64 / rate,
            hit_at: t.hit as f64 / rate,
            scores: t.scores,
            scores_end: t.scores_end as f64 / rate,
            reading,
            label: None,
            bytes,
        };
        let body = serde_json::to_vec_pretty(&meta).map_err(io::Error::other)?;
        meta.bytes += body.len() as u64;
        let body = serde_json::to_vec_pretty(&meta).map_err(io::Error::other)?;
        write_private(&self.dir.join(format!("{id}.json")), &body)?;
        self.prune()?;
        Ok(Some(meta))
    }

    /// Every take, newest first. A take whose metadata does not read is left out.
    pub fn list(&self) -> Vec<TakeMeta> {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<TakeMeta> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| fs::read(&p).ok())
            .filter_map(|b| serde_json::from_slice::<TakeMeta>(&b).ok())
            .collect();
        out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
        out
    }

    pub fn usage(&self) -> Usage {
        let list = self.list();
        Usage {
            takes: list.len(),
            bytes: list.iter().map(|t| t.bytes).sum(),
        }
    }

    pub fn get(&self, id: &str) -> io::Result<TakeMeta> {
        let p = self.path(id, "json")?;
        let b = fs::read(p)?;
        serde_json::from_slice(&b).map_err(io::Error::other)
    }

    fn path(&self, id: &str, ext: &str) -> io::Result<PathBuf> {
        let ok = !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such take"));
        }
        Ok(self.dir.join(format!("{id}.{ext}")))
    }

    pub fn label(&self, id: &str, label: Option<Label>) -> io::Result<TakeMeta> {
        let _g = lock(&self.write);
        let mut m = self.get(id)?;
        m.label = label;
        let body = serde_json::to_vec_pretty(&m).map_err(io::Error::other)?;
        write_private(&self.path(id, "json")?, &body)?;
        Ok(m)
    }

    /// The take's audio for playing: the stream the detector heard, or the array's first
    /// channel.
    pub fn audio(&self, id: &str, array: bool) -> io::Result<Vec<i16>> {
        let p = if array {
            self.path(id, "array.wav")?
        } else {
            self.path(id, "wav")?
        };
        let (ch, pcm) = read_wav(&p)?;
        Ok(if ch > 1 {
            pcm.chunks(usize::from(ch)).map(|f| f[0]).collect()
        } else {
            pcm
        })
    }

    fn remove(&self, id: &str) -> io::Result<bool> {
        let mut any = false;
        for ext in ["wav", "array.wav", "json"] {
            match fs::remove_file(self.path(id, ext)?) {
                Ok(()) => any = true,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(any)
    }

    pub fn delete(&self, id: &str) -> io::Result<bool> {
        let _g = lock(&self.write);
        self.remove(id)
    }

    /// Every file in the takes' folder, and the folder: what is left is nothing.
    pub fn delete_all(&self) -> io::Result<usize> {
        let _g = lock(&self.write);
        let n = self.list().len();
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Oldest first, until both caps hold. Files with no metadata (a write cut off) go too.
    fn prune(&self) -> io::Result<()> {
        let mut list = self.list();
        list.reverse(); // oldest first
        let mut bytes: u64 = list.iter().map(|t| t.bytes).sum();
        let mut count = list.len();
        for t in &list {
            if count <= self.max_takes && bytes <= self.max_bytes {
                break;
            }
            self.remove(&t.id)?;
            count -= 1;
            bytes = bytes.saturating_sub(t.bytes);
        }
        let known: std::collections::HashSet<String> =
            self.list().into_iter().map(|t| t.id).collect();
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for e in rd.filter_map(|e| e.ok()) {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.split('.').next().unwrap_or("").to_string();
                if !known.contains(&id) && name.ends_with(".wav") {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        Ok(())
    }

    /// The choice and the time, for a line in the log: numbers only.
    pub fn describe(m: &TakeMeta) -> String {
        format!(
            "wake take kept: {:?} {} {:.2} at {}, {:.1} s, {} bytes",
            m.outcome,
            m.model,
            m.score,
            iso(&m.at),
            m.secs,
            m.bytes
        )
    }
}

fn write_private(path: &Path, body: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(body)?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, path)
}

/// A 16-bit PCM WAV at 16 kHz; returns its size.
pub fn write_wav(path: &Path, pcm: &[i16], channels: u16) -> io::Result<u64> {
    let data_len = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2 * u32::from(channels)).to_le_bytes());
    b.extend_from_slice(&(2 * channels).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    write_private(path, &b)?;
    Ok(b.len() as u64)
}

/// A WAV this module wrote: (channels, samples).
pub fn read_wav(path: &Path) -> io::Result<(u16, Vec<i16>)> {
    let b = fs::read(path)?;
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a take's WAV");
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(bad());
    }
    let channels = u16::from_le_bytes([b[22], b[23]]);
    let bits = u16::from_le_bytes([b[34], b[35]]);
    if bits != 16 || channels == 0 {
        return Err(bad());
    }
    let pcm = b[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c))
        .collect();
    Ok((channels, pcm))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::store::Store;

    fn take(accepted: bool, secs: f64) -> NewTake {
        let n = (secs * f64::from(RATE)) as usize;
        NewTake {
            audio: (0..n).map(|i| ((i % 100) as i16) * 10).collect(),
            hit: n.saturating_sub(24_000),
            accepted,
            model: "hey_ghost".into(),
            score: 0.45,
            threshold: 0.6,
            input: "ghost.mic".into(),
            reading: Some("A ghost.".into()),
            scores: vec![0.1, 0.3, 0.45],
            scores_end: n.saturating_sub(24_000),
            array: None,
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn on_by_default_and_kept_private() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Takes::new(tmp.path());
        assert!(t.choice().keep);
        let m = t.keep(take(true, 3.0)).unwrap().unwrap();
        assert_eq!(m.outcome, Outcome::Accepted);
        assert_eq!(m.reading.as_deref(), Some("A ghost."));
        let dir = tmp.path().join(FOLDER);
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        for f in files(&dir) {
            let mode = fs::metadata(dir.join(&f)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{f}");
        }
        assert_eq!(t.list().len(), 1);
        assert_eq!(t.usage().bytes, m.bytes);
        let total: u64 = files(&dir)
            .iter()
            .map(|f| fs::metadata(dir.join(f)).unwrap().len())
            .sum();
        assert_eq!(total, m.bytes);
    }

    #[test]
    fn off_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Takes::new(tmp.path());
        t.set_choice(Choice {
            keep: false,
            ..Choice::default()
        })
        .unwrap();
        assert!(!t.wants());
        assert!(t.keep(take(true, 3.0)).unwrap().is_none());
        assert!(t.keep(take(false, 3.0)).unwrap().is_none());
        assert!(!tmp.path().join(FOLDER).exists());
    }

    #[test]
    fn the_choice_holds_over_a_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let items = Items::new(Arc::new(Store::in_memory().unwrap()));
        let t = Takes::new(tmp.path());
        assert!(t.attach(items.clone()).unwrap().keep);
        t.set_choice(Choice {
            keep: false,
            unlabelled_accepted_are_me: true,
        })
        .unwrap();
        let again = Takes::new(tmp.path());
        let c = again.attach(items).unwrap();
        assert!(!c.keep);
        assert!(c.unlabelled_accepted_are_me);
    }

    #[test]
    fn the_caps_remove_the_oldest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Takes::with_caps(tmp.path().join("t"), 3, u64::MAX);
        let t0 = Utc::now();
        let mut ids = Vec::new();
        for i in 0..5 {
            let at = t0 + chrono::Duration::seconds(i);
            ids.push(t.keep_at(take(i % 2 == 0, 1.0), at).unwrap().unwrap().id);
        }
        let left: Vec<String> = t.list().into_iter().map(|m| m.id).collect();
        assert_eq!(left, vec![ids[4].clone(), ids[3].clone(), ids[2].clone()]);
        // bytes: one take's worth
        let one = t.list()[0].bytes;
        let small = Takes::with_caps(tmp.path().join("s"), 100, one + one / 2);
        for i in 0..3 {
            small
                .keep_at(take(true, 1.0), t0 + chrono::Duration::seconds(i))
                .unwrap();
        }
        assert_eq!(small.list().len(), 1);
        assert!(small.usage().bytes <= one + one / 2);
    }

    #[test]
    fn both_streams_and_the_label_and_deleting() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Takes::new(tmp.path());
        let mut nt = take(false, 2.0);
        nt.array = Some(("alsa_input.array".into(), 2, vec![7i16; 64_000]));
        let m = t.keep(nt).unwrap().unwrap();
        assert_eq!(m.outcome, Outcome::Refused);
        assert_eq!(m.array_channels, 2);
        assert_eq!(t.audio(&m.id, true).unwrap().len(), 32_000);
        assert_eq!(t.audio(&m.id, false).unwrap().len(), 32_000);
        let l = t.label(&m.id, Some(Label::Me)).unwrap();
        assert_eq!(l.label, Some(Label::Me));
        assert_eq!(t.get(&m.id).unwrap().label, Some(Label::Me));
        assert!(t.delete(&m.id).unwrap());
        assert!(files(&tmp.path().join(FOLDER)).is_empty());
        assert!(t.label("../../etc/passwd", None).is_err());
    }

    #[test]
    fn delete_all_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Takes::new(tmp.path());
        for _ in 0..3 {
            t.keep(take(true, 1.0)).unwrap();
        }
        assert_eq!(t.delete_all().unwrap(), 3);
        assert!(!tmp.path().join(FOLDER).exists());
        assert_eq!(t.usage(), Usage::default());
        // and it keeps working after
        t.keep(take(true, 1.0)).unwrap();
        assert_eq!(t.list().len(), 1);
    }

    #[test]
    fn a_wav_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.wav");
        write_wav(&p, &[1, -2, 3, -4], 2).unwrap();
        assert_eq!(read_wav(&p).unwrap(), (2, vec![1, -2, 3, -4]));
    }
}
