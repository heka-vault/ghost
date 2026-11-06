//! `ghost-perf hear`: how well Ghost hears the operator, measured (issue #60, #52).
//!
//! Each take, a recording of the operator from `make record-hear` or a line of the plan
//! rendered by Kokoro, goes through the real speech-in stage (`stt::ParakeetStt`, the real
//! Silero and Parakeet, the session's VAD settings), fed as fast as the models go on the
//! audio's own clock. It is heard three ways:
//!
//! - `stage`: what the stage hands the aggregator, every final of the take joined with a
//!   space as `turns` joins them. Today that is each VAD segment read alone.
//! - `turn`: the turn's audio, from the first segment's pre-roll to the last segment's
//!   end, read once at the close: the whole-turn re-read of #52 item 1, before it exists.
//! - `file`: the whole take read once with no VAD: what Parakeet can do with the audio.
//!
//! Every read is scored against what was said (`score`): word error rate, errors by
//! class (numbers, the take's terms, function words, other words), empty reads of a
//! reply, words made up in a silent take, and what each read cost at the close. The
//! report compares the three ways, and with `--baseline` the same takes in an earlier
//! run, by a paired bootstrap, and states the pass rule (`report`). Results hold numbers,
//! never transcripts; `--show-text` prints the transcripts to the terminal only.
//!
//! The takes are recorded on purpose and kept under `.scratch/hear-takes/` (git-ignored);
//! nothing here reads Ghost's passive audio.

pub mod plan;
pub mod report;
pub mod score;
pub mod store;
pub mod take;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use ghost_infer::{Silero, VadAnalyzer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::plan::{Channel, Item, Kind};
use self::score::Class;
use super::audio::Voice;
use super::harness::{RunConfig, make_settings};
use crate::config::VoiceSettings;
use crate::frames::{Audio, Direction, Frame, Metric, Out, Stage};
use crate::pipeline::Models;
use crate::stt::{PRE_ROLL_SECS, ParakeetStt, SAMPLE_RATE, TRAILING_SECS, recognize};

/// The ways a take is heard, in report order.
pub const MODES: [&str; 3] = ["stage", "turn", "file"];
/// 20 ms, as the desk's capture delivers it.
const FRAME: usize = 320;
/// The room after the take, so a segment running to the end still closes.
const TAIL_SECS: f64 = 2.0;
/// The room under a rendered line.
const ROOM_DBFS: f64 = -60.0;

#[derive(Debug, Clone)]
pub struct HearArgs {
    /// A recorded session; None renders the plan with Kokoro.
    pub takes: Option<PathBuf>,
    pub channel: Channel,
    pub voices: Vec<String>,
    pub blocks: Vec<String>,
    pub overrides: BTreeMap<String, String>,
    pub baseline: Option<PathBuf>,
    pub out: PathBuf,
    pub cache: PathBuf,
    pub root: PathBuf,
    pub show_text: bool,
    /// How much the close may slow (p95, against the baseline) before the rule fails.
    pub close_budget_secs: f64,
}

/// One read of a take.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Read {
    pub words: usize,
    pub sub: usize,
    pub del: usize,
    pub ins: usize,
    /// Per class, in `Class::ALL` order: `[words, sub, del, ins]`.
    pub classes: Vec<[usize; 4]>,
    pub terms_said: usize,
    pub terms_heard: usize,
    pub terms_inserted: usize,
    pub fillers: usize,
    /// VAD segments (the stage's); 1 for a turn read, 0 when nothing started.
    pub segments: usize,
    /// What reading cost once the operator stopped: the stage's last final, or the one
    /// read for `turn` and `file`. None when nothing was read.
    pub close_secs: Option<f64>,
    /// Every recognition the take took.
    pub read_secs: f64,
}

impl Read {
    pub fn errors(&self) -> usize {
        self.sub + self.del + self.ins
    }

    pub fn class(&self, c: Class) -> [usize; 4] {
        self.classes.get(c as usize).copied().unwrap_or_default()
    }

    fn scored(reference: &str, text: &str, terms: &[String]) -> Self {
        let s = score::score(reference, text, terms);
        Self {
            words: s.hyp_words,
            sub: s.sub,
            del: s.del,
            ins: s.ins,
            classes: Class::ALL
                .iter()
                .map(|&c| {
                    let k = s.class(c);
                    [k.words, k.sub, k.del, k.ins]
                })
                .collect(),
            terms_said: s.terms_said,
            terms_heard: s.terms_heard,
            terms_inserted: s.terms_inserted,
            fillers: s.fillers_heard,
            ..Self::default()
        }
    }
}

/// One take's results: what it was and how each way heard it. No text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TakeResult {
    pub id: String,
    pub block: String,
    pub condition: String,
    pub kind: Kind,
    pub tags: Vec<String>,
    pub source: String,
    pub seconds: f64,
    pub ref_words: usize,
    pub modes: BTreeMap<String, Read>,
}

impl TakeResult {
    pub fn key(&self) -> String {
        format!("{}|{}", self.id, self.source)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Results {
    pub meta: BTreeMap<String, Value>,
    pub takes: Vec<TakeResult>,
}

/// What the stage made of a take, and where its segments lay in the audio.
struct StageRun {
    text: String,
    segments: usize,
    close_secs: Option<f64>,
    read_secs: f64,
    /// Sample range of the turn: the first segment's pre-roll to the last segment's end.
    span: Option<(usize, usize)>,
}

/// The take through the speech-in stage, as a session runs it with the room's VAD.
async fn hear_stage(
    models: &Models,
    settings: &VoiceSettings,
    pcm: &[i16],
) -> anyhow::Result<StageRun> {
    let vad = VadAnalyzer::new(Silero::load(&models.silero)?, settings.vad_params());
    let pos = Arc::new(AtomicUsize::new(0));
    let clock_pos = pos.clone();
    // Any start far from zero: the VAD resets on its first call, then every 5 s of audio.
    let clock = Arc::new(move || 1.0e9 + clock_pos.load(Ordering::Relaxed) as f64 / 16_000.0);
    let mut stt = ParakeetStt::new(models.stt.clone(), Some(vad), settings.vad_params(), 0.0)
        .with_vad_clock(clock);
    let (out, mut rx) = Out::capture();
    let mut audio = pcm.to_vec();
    audio.extend(plan::room_noise(TAIL_SECS, -70.0, 99));
    let (mut texts, mut starts, mut stops) = (Vec::new(), Vec::new(), Vec::new());
    let (mut close, mut read_secs) = (None, 0.0);
    let mut end = 0usize;
    for chunk in audio.chunks(FRAME) {
        end += chunk.len();
        pos.store(end, Ordering::Relaxed);
        stt.process(
            Frame::AudioIn(Audio::new(chunk.to_vec(), SAMPLE_RATE)),
            Direction::Down,
            &out,
        )
        .await;
        while let Ok(p) = rx.try_recv() {
            match p.frame {
                Frame::VadStarted { .. } => starts.push(end - chunk.len()),
                Frame::VadStopped { .. } => stops.push(end),
                Frame::Transcript(t) => texts.push(t),
                Frame::Metrics(Metric::Ttfb { value, .. }) => {
                    read_secs += value;
                    close = Some(value);
                }
                _ => {}
            }
        }
    }
    stt.stopped().await;
    let pre_roll = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
    let span = match (starts.first(), stops.last()) {
        (Some(&s), Some(&e)) if e > s => Some((s.saturating_sub(pre_roll), e)),
        _ => None,
    };
    Ok(StageRun {
        text: texts.join(" "),
        segments: starts.len(),
        close_secs: close,
        read_secs,
        span,
    })
}

/// One read of `pcm` as the stage reads a segment: silence after it, the dither under it.
async fn read_once(models: &Models, pcm: &[i16]) -> (String, f64) {
    let mut segment = pcm.to_vec();
    segment.extend(std::iter::repeat_n(
        0i16,
        (TRAILING_SECS * f64::from(SAMPLE_RATE)) as usize,
    ));
    let audio = ghost_infer::audio::to_f32(&segment);
    let model = models.stt.clone();
    let began = Instant::now();
    let text = tokio::task::spawn_blocking(move || recognize(&model, &audio, true))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    (text, began.elapsed().as_secs_f64())
}

/// A take heard the three ways, scored; the transcripts too, for `--show-text`.
pub async fn hear(
    models: &Models,
    settings: &VoiceSettings,
    item: &Item,
) -> anyhow::Result<(TakeResult, BTreeMap<String, String>)> {
    let stage = hear_stage(models, settings, &item.pcm).await?;
    let mut texts = BTreeMap::new();
    let mut modes = BTreeMap::new();
    let mut r = Read::scored(&item.text, &stage.text, &item.terms);
    r.segments = stage.segments;
    r.close_secs = stage.close_secs;
    r.read_secs = stage.read_secs;
    modes.insert("stage".to_string(), r);
    texts.insert("stage".to_string(), stage.text.clone());

    let mut padded = item.pcm.clone();
    padded.extend(plan::room_noise(TAIL_SECS, -70.0, 99)); // as the stage heard it
    let (turn_text, turn_read) = match stage.span {
        Some((s, e)) => {
            let (t, secs) = read_once(models, &padded[s..e.min(padded.len())]).await;
            (t, Some(secs))
        }
        None => (String::new(), None),
    };
    let mut r = Read::scored(&item.text, &turn_text, &item.terms);
    r.segments = usize::from(stage.span.is_some());
    r.close_secs = turn_read;
    r.read_secs = turn_read.unwrap_or(0.0);
    modes.insert("turn".to_string(), r);
    texts.insert("turn".to_string(), turn_text);

    let (file_text, file_read) = read_once(models, &item.pcm).await;
    let mut r = Read::scored(&item.text, &file_text, &item.terms);
    r.segments = 1;
    r.close_secs = Some(file_read);
    r.read_secs = file_read;
    modes.insert("file".to_string(), r);
    texts.insert("file".to_string(), file_text);

    let ref_words = score::normalize(&item.text).len();
    Ok((
        TakeResult {
            id: item.id.clone(),
            block: item.block.clone(),
            condition: item.condition.clone(),
            kind: item.kind,
            tags: item.tags.clone(),
            source: item.source.clone(),
            seconds: item.seconds(),
            ref_words,
            modes,
        },
        texts,
    ))
}

fn seed(id: &str) -> u32 {
    id.bytes().fold(2166136261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    })
}

/// The plan's desk blocks spoken by Kokoro in each of `voices`, with a room under them.
pub fn rendered(voice: &Voice, voices: &[String], blocks: &[String]) -> anyhow::Result<Vec<Item>> {
    let plan = plan::plan()?;
    let mut items = Vec::new();
    for v in voices {
        let voice = voice.with_voice(v);
        for (block, take) in plan::renderable(&plan, blocks) {
            let s = seed(&format!("{}{v}", take.id));
            let pcm = match take.kind {
                Kind::Silence => plan::room_noise(6.0, ROOM_DBFS, s),
                _ => plan::in_room(&voice.render(&take.text)?.pcm, 0.8, 1.5, ROOM_DBFS, s),
            };
            items.push(Item {
                id: take.id.clone(),
                block: block.block.clone(),
                condition: block.condition.clone(),
                kind: take.kind,
                text: take.text.clone(),
                tags: take.tags.clone(),
                terms: take.terms.clone(),
                source: format!("kokoro:{v}"),
                pcm,
            });
        }
    }
    Ok(items)
}

fn git_commit(root: &Path) -> String {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    match run(&["rev-parse", "--short", "HEAD"]) {
        Some(c) => {
            let dirty = run(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|s| !s.is_empty());
            if dirty {
                format!("{c} (with changes)")
            } else {
                c
            }
        }
        None => "unknown".into(),
    }
}

/// The check after a take (ADR 0042): where the VAD hears speech, and what one read of
/// the whole take heard, against the line. For the screen; nothing is written.
pub async fn check_take(
    models: &Models,
    settings: &VoiceSettings,
    kind: Kind,
    line: &str,
    pcm: &[i16],
) -> anyhow::Result<take::Check> {
    let params = settings.vad_params();
    let silero = models.silero.clone();
    let audio = pcm.to_vec();
    let speech = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(f64, f64)>> {
        let mut vad = take::Vad::load(&silero, params)?;
        Ok(take::speech_span(&mut vad, &audio, params))
    })
    .await??;
    let (heard, _) = read_once(models, pcm).await;
    Ok(take::check(kind, line, pcm, speech, &heard))
}

/// `ghost-perf hear-check DIR`: every take of a session checked as the Hearing test checks
/// a new one, to find the takes to redo. What was heard is printed only with `show_text`.
pub async fn run_check(dir: &Path, show_text: bool) -> anyhow::Result<i32> {
    let settings = VoiceSettings::from_env()?;
    let models = Models::load(&settings)?;
    let text = std::fs::read_to_string(dir.join(plan::MANIFEST))
        .map_err(|e| anyhow::anyhow!("{}: {e}", dir.join(plan::MANIFEST).display()))?;
    let mut flagged = 0;
    let entries = plan::read_manifest(&text);
    for e in &entries {
        let Some(file) = e.file.as_ref().filter(|_| !e.skipped) else {
            continue;
        };
        let pcm = plan::read_mono(&dir.join(file))?;
        let c = check_take(&models, &settings, e.kind, &e.text, &pcm).await?;
        let redo = c.warnings.iter().any(take::Warning::redo);
        flagged += usize::from(redo);
        println!(
            "{} {:<7} {:>4.1}s  speech {}  words {}/{}{}",
            e.id,
            e.kind.name(),
            c.seconds,
            c.speech
                .map_or_else(|| "none".into(), |(a, b)| format!("{a:.2}-{b:.2}s")),
            c.matched,
            c.said,
            if redo { "  REDO" } else { "" }
        );
        for w in &c.warnings {
            println!("    {}", w.text());
        }
        if show_text {
            println!("    said:  {:?}", e.text);
            println!("    heard: {:?}", c.heard);
        }
    }
    println!(
        "\n{flagged} of {} takes to redo.",
        entries.iter().filter(|e| !e.skipped).count()
    );
    Ok(0)
}

/// What a run was: when, on what code and models, with which settings.
pub fn run_meta(
    settings: &VoiceSettings,
    overrides: &BTreeMap<String, String>,
    source: &str,
    channel: &str,
    commit: &str,
    wall_secs: f64,
) -> BTreeMap<String, Value> {
    let mut meta: BTreeMap<String, Value> = BTreeMap::new();
    meta.insert(
        "date".into(),
        json!(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
    );
    meta.insert("commit".into(), json!(commit));
    meta.insert("source".into(), json!(source));
    meta.insert("channel".into(), json!(channel));
    meta.insert(
        "speech to text".into(),
        json!(format!(
            "{} ({}), {} threads",
            settings
                .stt_dir
                .as_ref()
                .map_or_else(|| settings.stt_model.clone(), |d| d.display().to_string()),
            settings.stt_quantization.as_deref().unwrap_or("fp32"),
            settings.stt_threads
        )),
    );
    let v = settings.vad_params();
    meta.insert(
        "vad".into(),
        json!(format!(
            "confidence {}, start {} s, stop {} s, min volume {}",
            v.confidence, v.start_secs, v.stop_secs, v.min_volume
        )),
    );
    meta.insert(
        "settings overrides".into(),
        json!(if overrides.is_empty() {
            "none".to_string()
        } else {
            overrides
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        }),
    );
    meta.insert(
        "host".into(),
        json!(format!(
            "{} CPUs",
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1)
        )),
    );
    meta.insert("wall".into(), json!(format!("{wall_secs:.0}s")));
    meta
}

/// A recorded session scored on models already loaded: the daemon's Hearing test (ADR
/// 0042). `progress(done, total)` after each take. No text leaves here.
pub async fn score_session(
    models: &Models,
    settings: &VoiceSettings,
    dir: &Path,
    channel: Channel,
    commit: &str,
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<Results> {
    let t = Instant::now();
    let (items, _missing) = plan::recorded(dir, channel, &[])?;
    anyhow::ensure!(
        !items.is_empty(),
        "the session has no takes on {}",
        channel.name()
    );
    let mut takes = Vec::with_capacity(items.len());
    progress(0, items.len());
    for (i, item) in items.iter().enumerate() {
        let (result, _texts) = hear(models, settings, item).await?;
        takes.push(result);
        progress(i + 1, items.len());
    }
    let name = dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let meta = run_meta(
        settings,
        &BTreeMap::new(),
        &format!("recorded takes, session {name}"),
        channel.name(),
        commit,
        t.elapsed().as_secs_f64(),
    );
    Ok(Results { meta, takes })
}

pub fn read_results(path: &Path) -> anyhow::Result<Results> {
    let text =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

/// Run the takes, write `results.json` and `report.md`, print the verdict. 0, or 1 when a
/// baseline was given and the pass rule failed.
pub async fn run_hear(a: &HearArgs) -> anyhow::Result<i32> {
    let base = VoiceSettings::from_env()?;
    crate::debug::enable(base.debug_audio); // the operator's own debugging session only
    let cfg = RunConfig {
        overrides: a.overrides.clone(),
        ..RunConfig::default()
    };
    let settings = make_settings(&base, &cfg).map_err(|e| anyhow::anyhow!(e))?;
    let baseline = match &a.baseline {
        Some(p) => Some(read_results(p)?),
        None => None,
    };
    let t = Instant::now();
    let models = Models::load(&settings)?;
    println!("hear: models loaded in {:.1}s", t.elapsed().as_secs_f64());

    let (items, source, missing) = match &a.takes {
        Some(dir) => {
            let (items, missing) = plan::recorded(dir, a.channel, &a.blocks)?;
            (
                items,
                format!("recorded takes in {}", dir.display()),
                missing,
            )
        }
        None => {
            let voice = Voice::new(models.tts.clone(), &a.cache.join("utterances"));
            let items = rendered(&voice, &a.voices, &a.blocks)?;
            (
                items,
                format!(
                    "the plan's desk blocks spoken by Kokoro ({})",
                    a.voices.join(", ")
                ),
                Vec::new(),
            )
        }
    };
    anyhow::ensure!(!items.is_empty(), "no takes to hear");
    if !missing.is_empty() {
        println!(
            "hear: {} take(s) have no {} file and are left out: {}",
            missing.len(),
            a.channel.name(),
            missing.join(", ")
        );
    }
    println!("hear: {} takes from {source}", items.len());
    println!("hear: output in {}", a.out.display());

    let mut takes = Vec::new();
    for item in &items {
        let (result, texts) = hear(&models, &settings, item).await?;
        let errs: Vec<String> = MODES
            .iter()
            .map(|m| {
                let r = &result.modes[*m];
                if item.kind == Kind::Silence {
                    format!("{m} {} made up", r.words)
                } else {
                    format!("{m} {}/{}", r.errors(), result.ref_words)
                }
            })
            .collect();
        println!(
            "  {} {:<7} {:>4.1}s  segments {}  errors: {}",
            item.id,
            item.kind.name(),
            item.seconds(),
            result.modes["stage"].segments,
            errs.join(", ")
        );
        if a.show_text {
            println!("      said:  {:?}", item.text);
            for m in MODES {
                println!("      {m:<6} {:?}", texts[m]);
            }
        }
        takes.push(result);
    }

    let channel = if a.takes.is_some() {
        a.channel.name()
    } else {
        "rendered"
    };
    let meta = run_meta(
        &settings,
        &a.overrides,
        &source,
        channel,
        &git_commit(&a.root),
        t.elapsed().as_secs_f64(),
    );
    let results = Results { meta, takes };
    std::fs::write(
        a.out.join("results.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    let (text, verdict) = report::render(&results, baseline.as_ref(), a.close_budget_secs);
    std::fs::write(a.out.join("report.md"), &text)?;
    println!(
        "\n{}",
        report::summary(&results, baseline.as_ref(), a.close_budget_secs)
    );
    println!("hear: report {}", a.out.join("report.md").display());
    Ok(match verdict {
        Some(false) => 1,
        _ => 0,
    })
}
