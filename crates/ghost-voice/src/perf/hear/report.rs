//! The hear report: the three ways side by side, by block, by class and by tag; the
//! paired comparisons; and, against a baseline, the pass rule.
//!
//! **The pass rule** (docs/benchmarks/hear-harness.md), for the `stage` read, the one
//! Ghost ships, against the same takes in the baseline run: no measure is worse beyond
//! the noise of the set (word error rate, each class's errors, empty reads of a request or
//! reply, silent takes with words, made-up words), and the close's p95 has not grown by
//! more than the budget. A measure is "worse" only when its whole 95% bootstrap interval
//! is above zero.

use std::collections::BTreeMap;

use serde::Serialize;

use super::plan::Kind;
use super::score::{Class, Delta, Verdict, paired_delta};
use super::{MODES, Read, Results, TakeResult};

const RESAMPLES: usize = 4000;
const SEED: u64 = 60;

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn quantile(xs: &[f64], q: f64) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    Some(v[((q * (v.len() - 1) as f64).round() as usize).min(v.len() - 1)])
}

fn secs(x: Option<f64>) -> String {
    x.map_or_else(|| "-".into(), |s| format!("{:.2} s", s))
}

fn spoken(t: &TakeResult) -> bool {
    t.kind != Kind::Silence
}

/// A take's `(errors, words)` under one read.
type Get = Box<dyn Fn(&TakeResult, &Read) -> (f64, f64)>;

/// A measure over takes: each take's `(errors, words)` under one read, and which takes
/// it counts.
struct Measure {
    name: &'static str,
    of: fn(&TakeResult) -> bool,
    get: Get,
}

fn measures() -> Vec<Measure> {
    let mut m = vec![Measure {
        name: "word error rate",
        of: spoken,
        get: Box::new(|t, r| (r.errors() as f64, t.ref_words as f64)),
    }];
    for c in Class::ALL {
        m.push(Measure {
            name: match c {
                Class::Number => "number errors",
                Class::Term => "term errors",
                Class::Function => "function-word errors",
                Class::Content => "other-word errors",
            },
            of: spoken,
            get: Box::new(move |_, r| {
                let k = r.class(c);
                ((k[1] + k[2] + k[3]) as f64, k[0] as f64)
            }),
        });
    }
    m.push(Measure {
        name: "empty reads",
        of: spoken,
        get: Box::new(|_, r| (f64::from(u8::from(r.words == 0)), 1.0)),
    });
    m.push(Measure {
        name: "silent takes with words",
        of: |t| t.kind == Kind::Silence,
        get: Box::new(|_, r| (f64::from(u8::from(r.words > 0)), 1.0)),
    });
    m.push(Measure {
        name: "made-up words per silent take",
        of: |t| t.kind == Kind::Silence,
        get: Box::new(|_, r| (r.words as f64, 1.0)),
    });
    m
}

fn rate(pairs: &[(f64, f64)]) -> Option<f64> {
    let (e, w) = pairs
        .iter()
        .fold((0.0, 0.0), |(e, w), p| (e + p.0, w + p.1));
    (w > 0.0).then(|| e / w)
}

fn fmt_rate(name: &str, x: f64) -> String {
    if name.starts_with("made-up") {
        format!("{x:.2}")
    } else {
        pct(x)
    }
}

/// The takes in both runs, paired by id and source: (before, after).
fn pairs<'a>(a: &'a [TakeResult], b: &'a [TakeResult]) -> Vec<(&'a TakeResult, &'a TakeResult)> {
    let by_key: BTreeMap<String, &TakeResult> = a.iter().map(|t| (t.key(), t)).collect();
    b.iter()
        .filter_map(|t| by_key.get(&t.key()).map(|&p| (p, t)))
        .collect()
}

struct Compared {
    name: &'static str,
    n: usize,
    delta: Delta,
}

fn compare(pairs: &[(&TakeResult, &TakeResult)], mode_a: &str, mode_b: &str) -> Vec<Compared> {
    measures()
        .into_iter()
        .filter_map(|m| {
            let chosen: Vec<_> = pairs.iter().filter(|(x, _)| (m.of)(x)).collect();
            let a: Vec<(f64, f64)> = chosen
                .iter()
                .filter_map(|(x, _)| x.modes.get(mode_a).map(|r| (m.get)(x, r)))
                .collect();
            let b: Vec<(f64, f64)> = chosen
                .iter()
                .filter_map(|(_, y)| y.modes.get(mode_b).map(|r| (m.get)(y, r)))
                .collect();
            if a.is_empty() || a.len() != b.len() || a.iter().all(|p| p.1 == 0.0) {
                return None;
            }
            Some(Compared {
                name: m.name,
                n: a.len(),
                delta: paired_delta(&a, &b, RESAMPLES, SEED),
            })
        })
        .collect()
}

fn win_tie_loss(
    pairs: &[(&TakeResult, &TakeResult)],
    mode_a: &str,
    mode_b: &str,
) -> (usize, usize, usize) {
    let mut w = (0, 0, 0);
    for (x, y) in pairs {
        let (Some(a), Some(b)) = (x.modes.get(mode_a), y.modes.get(mode_b)) else {
            continue;
        };
        let (ea, eb) = if x.kind == Kind::Silence {
            (a.words, b.words)
        } else {
            (a.errors(), b.errors())
        };
        match eb.cmp(&ea) {
            std::cmp::Ordering::Less => w.0 += 1,
            std::cmp::Ordering::Equal => w.1 += 1,
            std::cmp::Ordering::Greater => w.2 += 1,
        }
    }
    w
}

fn close_p95(takes: &[&TakeResult], mode: &str) -> Option<f64> {
    let xs: Vec<f64> = takes
        .iter()
        .filter(|t| spoken(t))
        .filter_map(|t| t.modes.get(mode).and_then(|r| r.close_secs))
        .collect();
    quantile(&xs, 0.95)
}

fn signed(name: &str, x: f64) -> String {
    let s = fmt_rate(name, x.abs());
    if x > 0.0 {
        format!("+{s}")
    } else if x < 0.0 {
        format!("−{s}")
    } else {
        s
    }
}

fn comparison_parts(parts: &mut Vec<Part>, rows: &[Compared], wtl: (usize, usize, usize)) {
    let mut table = Table::new(&[
        "Measure",
        "Takes",
        "Before",
        "After",
        "Change",
        "95% interval",
        "Verdict",
    ]);
    for c in rows {
        let d = &c.delta;
        table.rows.push(Row {
            cells: vec![
                c.name.to_string(),
                c.n.to_string(),
                fmt_rate(c.name, d.a),
                fmt_rate(c.name, d.b),
                signed(c.name, d.delta),
                format!("{} to {}", signed(c.name, d.low), signed(c.name, d.high)),
                d.verdict().word().to_string(),
            ],
            verdict: Some(d.verdict()),
            take: None,
        });
    }
    parts.push(Part::Table(table));
    parts.push(Part::Text(format!(
        "Per take, fewer errors after / the same / more: {} / {} / {} (damage: {} of {}).",
        wtl.0,
        wtl.1,
        wtl.2,
        wtl.2,
        wtl.0 + wtl.1 + wtl.2
    )));
}

struct Summary {
    spoken: usize,
    wer: Option<f64>,
    requests_wer: Option<f64>,
    replies: usize,
    replies_empty: usize,
    empty: usize,
    silent: usize,
    silent_with_words: usize,
    made_up: usize,
    terms: (usize, usize, usize),
    numbers: (usize, usize),
    segments: f64,
    split: usize,
    close_p50: Option<f64>,
    close_p95: Option<f64>,
    cost_per_sec: Option<f64>,
}

fn summarize(takes: &[&TakeResult], mode: &str) -> Summary {
    let reads: Vec<(&TakeResult, &Read)> = takes
        .iter()
        .filter_map(|t| t.modes.get(mode).map(|r| (*t, r)))
        .collect();
    let spoken_reads: Vec<_> = reads.iter().filter(|(t, _)| spoken(t)).collect();
    let wer = rate(
        &spoken_reads
            .iter()
            .map(|(t, r)| (r.errors() as f64, t.ref_words as f64))
            .collect::<Vec<_>>(),
    );
    let requests_wer = rate(
        &spoken_reads
            .iter()
            .filter(|(t, _)| t.kind == Kind::Speech)
            .map(|(t, r)| (r.errors() as f64, t.ref_words as f64))
            .collect::<Vec<_>>(),
    );
    let silent: Vec<_> = reads
        .iter()
        .filter(|(t, _)| t.kind == Kind::Silence)
        .collect();
    let closes: Vec<f64> = spoken_reads
        .iter()
        .filter_map(|(_, r)| r.close_secs)
        .collect();
    let audio: f64 = reads.iter().map(|(t, _)| t.seconds).sum();
    let read: f64 = reads.iter().map(|(_, r)| r.read_secs).sum();
    Summary {
        spoken: spoken_reads.len(),
        wer,
        requests_wer,
        replies: spoken_reads
            .iter()
            .filter(|(t, _)| t.kind == Kind::Reply)
            .count(),
        replies_empty: spoken_reads
            .iter()
            .filter(|(t, r)| t.kind == Kind::Reply && r.words == 0)
            .count(),
        empty: spoken_reads.iter().filter(|(_, r)| r.words == 0).count(),
        silent: silent.len(),
        silent_with_words: silent.iter().filter(|(_, r)| r.words > 0).count(),
        made_up: silent.iter().map(|(_, r)| r.words).sum(),
        terms: spoken_reads.iter().fold((0, 0, 0), |a, (_, r)| {
            (
                a.0 + r.terms_heard,
                a.1 + r.terms_said,
                a.2 + r.terms_inserted,
            )
        }),
        numbers: spoken_reads.iter().fold((0, 0), |a, (_, r)| {
            let k = r.class(Class::Number);
            (a.0 + k[1] + k[2] + k[3], a.1 + k[0])
        }),
        segments: if spoken_reads.is_empty() {
            0.0
        } else {
            spoken_reads
                .iter()
                .map(|(_, r)| r.segments as f64)
                .sum::<f64>()
                / spoken_reads.len() as f64
        },
        split: spoken_reads.iter().filter(|(_, r)| r.segments > 1).count(),
        close_p50: quantile(&closes, 0.5),
        close_p95: quantile(&closes, 0.95),
        cost_per_sec: (audio > 0.0).then(|| read / audio),
    }
}

fn ratio(a: usize, b: usize) -> String {
    if b == 0 {
        "-".into()
    } else {
        format!("{a} of {b}")
    }
}

// --- the report as data: what the markdown and the desktop's Hearing test both show ----------

/// One row of a table. `verdict`: a comparison row's; `take`: the take a row is about
/// (`TakeResult::key`), so the desktop can play it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub cells: Vec<String>,
    pub verdict: Option<Verdict>,
    pub take: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

impl Table {
    fn new(columns: &[&str]) -> Self {
        Self {
            columns: columns.iter().map(|c| (*c).to_string()).collect(),
            rows: Vec::new(),
        }
    }

    fn with(columns: Vec<String>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    fn row(&mut self, cells: Vec<String>) {
        self.rows.push(Row {
            cells,
            verdict: None,
            take: None,
        });
    }
}

/// A section's content, in order. Text may hold `code` in backticks.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    Text {
        text: String,
    },
    Table(Table),
    /// Name and value pairs.
    Facts {
        facts: Vec<(String, String)>,
    },
    /// The pass rule's line: `pass`, and why.
    Pass {
        pass: bool,
        text: String,
    },
}

#[allow(non_snake_case)]
impl Part {
    fn Text(text: impl Into<String>) -> Self {
        Part::Text { text: text.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    /// 2 for a section, 3 for one inside the one before.
    pub level: u8,
    pub title: String,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub meta: Vec<(String, String)>,
    pub intro: String,
    pub sections: Vec<Section>,
    /// The pass rule for `stage`, when there is a baseline.
    pub pass: Option<bool>,
}

fn meta_value(v: &serde_json::Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), String::from)
}

fn section(level: u8, title: impl Into<String>) -> Section {
    Section {
        level,
        title: title.into(),
        parts: Vec::new(),
    }
}

/// The report as data, and the pass rule's verdict for the `stage` read when there is a
/// baseline.
pub fn build(results: &Results, baseline: Option<&Results>, budget: f64) -> Report {
    let takes: Vec<&TakeResult> = results.takes.iter().collect();
    let mut meta: Vec<(String, String)> = results
        .meta
        .iter()
        .map(|(k, v)| (k.clone(), meta_value(v)))
        .collect();
    meta.push(("takes".into(), takes.len().to_string()));
    let intro = "`stage` is what the speech-in stage hands the aggregator (each VAD segment read \
         alone, the finals joined); `turn` is the turn's audio read once, what the aggregator \
         hands the model for a turn of two segments or more (ADR 0044, `GHOST_TURN_REREAD`); \
         `file` is the whole take read once with no VAD. Fillers are left out of every count; \
         numbers are compared as digits."
        .to_string();
    let mut sections = Vec::new();

    // the summary
    let mut s = section(2, "Summary");
    let mut header = vec![String::new()];
    header.extend(MODES.iter().map(|m| (*m).to_string()));
    let mut table = Table::with(header);
    let sums: Vec<Summary> = MODES.iter().map(|m| summarize(&takes, m)).collect();
    let mut row = |label: &str, f: &dyn Fn(&Summary) -> String| {
        let mut cells = vec![label.to_string()];
        cells.extend(sums.iter().map(f));
        table.row(cells);
    };
    row("word error rate (requests and replies)", &|s| {
        s.wer.map_or("-".into(), pct)
    });
    row("word error rate (requests)", &|s| {
        s.requests_wer.map_or("-".into(), pct)
    });
    row("number errors", &|s| ratio(s.numbers.0, s.numbers.1));
    row("terms heard", &|s| ratio(s.terms.0, s.terms.1));
    row("terms made up", &|s| s.terms.2.to_string());
    row("empty reads (requests and replies)", &|s| {
        ratio(s.empty, s.spoken)
    });
    row("replies dropped", &|s| ratio(s.replies_empty, s.replies));
    row("silent takes with words", &|s| {
        ratio(s.silent_with_words, s.silent)
    });
    row("made-up words in silent takes", &|s| s.made_up.to_string());
    row("segments per take", &|s| format!("{:.2}", s.segments));
    row("takes split in pieces", &|s| ratio(s.split, s.spoken));
    row("read at the close, p50", &|s| secs(s.close_p50));
    row("read at the close, p95", &|s| secs(s.close_p95));
    row("model time per second of audio", &|s| secs(s.cost_per_sec));
    s.parts.push(Part::Table(table));
    sections.push(s);

    // by block
    let mut s = section(2, "By block");
    let mut header: Vec<String> = ["Block", "Condition", "Takes"]
        .iter()
        .map(|c| (*c).to_string())
        .collect();
    header.extend(MODES.iter().map(|m| (*m).to_string()));
    let mut table = Table::with(header);
    let mut blocks: BTreeMap<(String, String), Vec<&TakeResult>> = BTreeMap::new();
    for t in &takes {
        blocks
            .entry((t.block.clone(), t.condition.clone()))
            .or_default()
            .push(t);
    }
    for ((b, c), ts) in &blocks {
        let mut cells = vec![b.clone(), c.clone(), ts.len().to_string()];
        cells.extend(MODES.iter().map(|m| {
            let s = summarize(ts, m);
            if s.spoken == 0 {
                format!("{} made up", s.made_up)
            } else {
                s.wer.map_or("-".into(), pct)
            }
        }));
        table.row(cells);
    }
    s.parts.push(Part::Table(table));
    s.parts.push(Part::Text(
        "A speech block's cell is its word error rate; a silent block's is the words made up.",
    ));
    sections.push(s);

    // by tag
    let mut tags: BTreeMap<&str, Vec<&TakeResult>> = BTreeMap::new();
    for t in &takes {
        for tag in &t.tags {
            tags.entry(tag.as_str()).or_default().push(t);
        }
    }
    let mut header: Vec<String> = vec!["Tag".into(), "Takes".into()];
    header.extend(MODES.iter().map(|m| (*m).to_string()));
    let mut table = Table::with(header);
    for (tag, ts) in tags.iter().filter(|(_, ts)| ts.iter().any(|t| spoken(t))) {
        let mut cells = vec![
            (*tag).to_string(),
            ts.iter().filter(|t| spoken(t)).count().to_string(),
        ];
        cells.extend(
            MODES
                .iter()
                .map(|m| summarize(ts, m).wer.map_or("-".into(), pct)),
        );
        table.row(cells);
    }
    if !table.rows.is_empty() {
        let mut s = section(2, "By tag (word error rate)");
        s.parts.push(Part::Table(table));
        sections.push(s);
    }

    // by class
    let mut s = section(2, "Errors by class");
    s.parts.push(Part::Text(
        "Substitutions and deletions count against the word said; insertions against the word heard.",
    ));
    let mut table = Table::new(&[
        "Read",
        "Class",
        "Words said",
        "Substituted",
        "Dropped",
        "Inserted",
        "Rate",
    ]);
    for m in MODES {
        for c in Class::ALL {
            let k = takes
                .iter()
                .filter(|t| spoken(t))
                .filter_map(|t| t.modes.get(m).map(|r| r.class(c)))
                .fold([0usize; 4], |a, k| {
                    [a[0] + k[0], a[1] + k[1], a[2] + k[2], a[3] + k[3]]
                });
            let rate = if k[0] > 0 {
                pct((k[1] + k[2] + k[3]) as f64 / k[0] as f64)
            } else {
                "-".into()
            };
            table.row(vec![
                m.to_string(),
                c.name().to_string(),
                k[0].to_string(),
                k[1].to_string(),
                k[2].to_string(),
                k[3].to_string(),
                rate,
            ]);
        }
    }
    s.parts.push(Part::Table(table));
    sections.push(s);

    // within the run
    let same: Vec<(&TakeResult, &TakeResult)> = takes.iter().map(|t| (*t, *t)).collect();
    for (a, b, why) in [
        (
            "stage",
            "turn",
            "what reading the whole turn changes (#52 item 1, #60 item 1; ADR 0044)",
        ),
        (
            "stage",
            "file",
            "how far today's read is from the whole take read once",
        ),
    ] {
        let mut s = section(2, format!("`{a}` against `{b}`: {why}"));
        comparison_parts(
            &mut s.parts,
            &compare(&same, a, b),
            win_tie_loss(&same, a, b),
        );
        let (pa, pb) = (close_p95(&takes, a), close_p95(&takes, b));
        s.parts.push(Part::Text(format!(
            "Read at the close, p95: {} against {}.",
            secs(pa),
            secs(pb)
        )));
        sections.push(s);
    }

    // against the baseline
    let mut verdict = None;
    if let Some(base) = baseline {
        let paired = pairs(&base.takes, &results.takes);
        let mut s = section(2, "Against the baseline");
        let mut facts: Vec<(String, String)> = base
            .meta
            .iter()
            .filter(|(k, _)| {
                [
                    "date",
                    "commit",
                    "source",
                    "settings overrides",
                    "speech to text",
                ]
                .contains(&k.as_str())
            })
            .map(|(k, v)| (format!("baseline {k}"), meta_value(v)))
            .collect();
        facts.push((
            "takes in both".into(),
            format!("{} of {}", paired.len(), results.takes.len()),
        ));
        s.parts.push(Part::Facts { facts });
        sections.push(s);
        for m in MODES {
            let rows = compare(&paired, m, m);
            let mut s = section(3, format!("`{m}`, baseline → this run"));
            comparison_parts(&mut s.parts, &rows, win_tie_loss(&paired, m, m));
            let before: Vec<&TakeResult> = paired.iter().map(|(x, _)| *x).collect();
            let after: Vec<&TakeResult> = paired.iter().map(|(_, y)| *y).collect();
            let (p0, p1) = (close_p95(&before, m), close_p95(&after, m));
            let grew = match (p0, p1) {
                (Some(a), Some(b)) => b - a,
                _ => 0.0,
            };
            s.parts.push(Part::Text(format!(
                "Read at the close, p95: {} → {} ({:+.2} s; the budget is +{:.2} s).",
                secs(p0),
                secs(p1),
                grew,
                budget
            )));
            if m == "stage" {
                let worse: Vec<&str> = rows
                    .iter()
                    .filter(|c| c.delta.verdict() == Verdict::Worse)
                    .map(|c| c.name)
                    .collect();
                let better: Vec<&str> = rows
                    .iter()
                    .filter(|c| c.delta.verdict() == Verdict::Better)
                    .map(|c| c.name)
                    .collect();
                let slow = grew > budget;
                let pass = worse.is_empty() && !slow && !paired.is_empty();
                verdict = Some(pass);
                s.parts.push(Part::Pass {
                    pass,
                    text: format!(
                        "{}{}{}",
                        if worse.is_empty() {
                            "Nothing is worse beyond noise".to_string()
                        } else {
                            format!("Worse beyond noise: {}", worse.join(", "))
                        },
                        if slow {
                            format!("; the close's p95 grew {grew:.2} s, over the budget")
                        } else {
                            String::new()
                        },
                        if better.is_empty() {
                            ".".to_string()
                        } else {
                            format!("; better: {}.", better.join(", "))
                        },
                    ),
                });
            }
            sections.push(s);
        }
    }

    // where to listen
    let mut worst: Vec<&TakeResult> = takes.iter().copied().filter(|t| spoken(t)).collect();
    worst.sort_by_key(|t| std::cmp::Reverse(t.modes.get("stage").map_or(0, Read::errors)));
    let worst: Vec<&TakeResult> = worst
        .into_iter()
        .filter(|t| t.modes.get("stage").is_some_and(|r| r.errors() > 0))
        .take(10)
        .collect();
    if !worst.is_empty() {
        let mut s = section(2, "The takes to listen to");
        s.parts.push(Part::Text(
            "The most errors in `stage`; `--show-text` prints what each read heard.",
        ));
        let mut header: Vec<String> = ["Take", "Source", "Words said", "Segments"]
            .iter()
            .map(|c| (*c).to_string())
            .collect();
        header.extend(MODES.iter().map(|m| (*m).to_string()));
        let mut table = Table::with(header);
        for t in worst {
            let mut cells = vec![
                t.id.clone(),
                t.source.clone(),
                t.ref_words.to_string(),
                t.modes.get("stage").map_or(0, |r| r.segments).to_string(),
            ];
            cells.extend(MODES.iter().map(|m| {
                t.modes
                    .get(*m)
                    .map_or("-".into(), |r| r.errors().to_string())
            }));
            table.rows.push(Row {
                cells,
                verdict: None,
                take: Some(t.key()),
            });
        }
        s.parts.push(Part::Table(table));
        sections.push(s);
    }
    Report {
        meta,
        intro,
        sections,
        pass: verdict,
    }
}

fn markdown_table(out: &mut Vec<String>, t: &Table) {
    let head: Vec<String> = t
        .columns
        .iter()
        .map(|c| {
            if c.is_empty() {
                " ".into()
            } else {
                format!(" {c} ")
            }
        })
        .collect();
    out.push(format!("|{}|", head.join("|")));
    out.push(format!("|{}", "---|".repeat(t.columns.len())));
    for r in &t.rows {
        out.push(format!("| {} |", r.cells.join(" | ")));
    }
}

/// The report as markdown (`report.md`).
pub fn markdown(report: &Report) -> String {
    let mut out = vec![
        "# How Ghost hears: the hear harness".to_string(),
        String::new(),
    ];
    for (k, v) in &report.meta {
        out.push(format!("- **{k}:** {v}"));
    }
    out.push(String::new());
    out.push(report.intro.clone());
    out.push(String::new());
    for s in &report.sections {
        out.push(format!("{} {}", "#".repeat(s.level as usize), s.title));
        out.push(String::new());
        for p in &s.parts {
            match p {
                Part::Text { text } => out.push(text.clone()),
                Part::Facts { facts } => {
                    for (k, v) in facts {
                        out.push(format!("- **{k}:** {v}"));
                    }
                }
                Part::Table(t) => markdown_table(&mut out, t),
                Part::Pass { pass, text } => out.push(format!(
                    "**Pass rule for `stage`: {}.** {text}",
                    if *pass { "PASS" } else { "FAIL" }
                )),
            }
            out.push(String::new());
        }
    }
    out.join("\n")
}

/// The report as markdown, and the pass rule's verdict for the `stage` read when there is
/// a baseline.
pub fn render(
    results: &Results,
    baseline: Option<&Results>,
    budget: f64,
) -> (String, Option<bool>) {
    let report = build(results, baseline, budget);
    (markdown(&report), report.pass)
}

/// The word error rate of one read over a run's spoken takes.
pub fn wer(results: &Results, mode: &str) -> Option<f64> {
    let takes: Vec<&TakeResult> = results.takes.iter().collect();
    summarize(&takes, mode).wer
}

/// A few lines for the terminal.
pub fn summary(results: &Results, baseline: Option<&Results>, budget: f64) -> String {
    let takes: Vec<&TakeResult> = results.takes.iter().collect();
    let mut lines = Vec::new();
    for m in MODES {
        let s = summarize(&takes, m);
        lines.push(format!(
            "{m:<6} WER {:>6}  empty {:>8}  silent with words {:>8}  close p95 {}",
            s.wer.map_or("-".into(), pct),
            ratio(s.empty, s.spoken),
            ratio(s.silent_with_words, s.silent),
            secs(s.close_p95)
        ));
    }
    if let (_, Some(pass)) = render(results, baseline, budget) {
        lines.push(format!(
            "pass rule (stage against the baseline): {}",
            if pass { "PASS" } else { "FAIL" }
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf::hear::Read;

    fn read(errors: usize, words: usize, close: f64) -> Read {
        Read {
            words,
            sub: errors,
            classes: vec![[words, errors, 0, 0], [0; 4], [0; 4], [0; 4]],
            segments: 1,
            close_secs: Some(close),
            read_secs: close,
            ..Read::default()
        }
    }

    fn take(id: &str, kind: Kind, stage: Read, turn: Read) -> TakeResult {
        let mut modes = BTreeMap::new();
        modes.insert("stage".into(), stage);
        modes.insert("turn".into(), turn.clone());
        modes.insert("file".into(), turn);
        TakeResult {
            id: id.into(),
            block: "A".into(),
            condition: "desk".into(),
            kind,
            tags: vec!["numbers".into()],
            source: "take".into(),
            seconds: 3.0,
            ref_words: 6,
            modes,
        }
    }

    fn results(takes: Vec<TakeResult>) -> Results {
        Results {
            meta: BTreeMap::new(),
            takes,
        }
    }

    #[test]
    fn a_clearly_better_turn_read_shows_as_better() {
        let r = results(
            (0..30)
                .map(|i| {
                    take(
                        &format!("A{i:02}"),
                        Kind::Speech,
                        read(2, 6, 0.1),
                        read(0, 6, 0.2),
                    )
                })
                .collect(),
        );
        let (text, verdict) = render(&r, None, 0.15);
        assert_eq!(verdict, None);
        assert!(
            text.contains("| word error rate | 30 | 33.3% | 0.0% |"),
            "{text}"
        );
        assert!(text.contains("| better |"));
    }

    #[test]
    fn the_pass_rule_fails_on_a_worse_stage_and_passes_on_the_same() {
        let before = results(
            (0..30)
                .map(|i| {
                    take(
                        &format!("A{i:02}"),
                        Kind::Speech,
                        read(0, 6, 0.1),
                        read(0, 6, 0.1),
                    )
                })
                .collect(),
        );
        let worse = results(
            (0..30)
                .map(|i| {
                    take(
                        &format!("A{i:02}"),
                        Kind::Speech,
                        read(2, 6, 0.1),
                        read(0, 6, 0.1),
                    )
                })
                .collect(),
        );
        assert_eq!(render(&worse, Some(&before), 0.15).1, Some(false));
        assert_eq!(render(&before, Some(&before), 0.15).1, Some(true));
    }

    #[test]
    fn the_pass_rule_fails_when_the_close_slows_past_the_budget() {
        let before = results(
            (0..30)
                .map(|i| {
                    take(
                        &format!("A{i:02}"),
                        Kind::Speech,
                        read(0, 6, 0.1),
                        read(0, 6, 0.1),
                    )
                })
                .collect(),
        );
        let slow = results(
            (0..30)
                .map(|i| {
                    take(
                        &format!("A{i:02}"),
                        Kind::Speech,
                        read(0, 6, 0.5),
                        read(0, 6, 0.1),
                    )
                })
                .collect(),
        );
        let (text, verdict) = render(&slow, Some(&before), 0.15);
        assert_eq!(verdict, Some(false));
        assert!(text.contains("over the budget"));
    }

    #[test]
    fn silence_counts_made_up_words_not_errors() {
        let mut t = take("H01", Kind::Silence, read(0, 2, 0.1), read(0, 0, 0.1));
        t.ref_words = 0;
        let r = results(vec![t]);
        let (text, _) = render(&r, None, 0.15);
        assert!(
            text.contains("| silent takes with words | 1 of 1 | 0 of 1 | 0 of 1 |"),
            "{text}"
        );
        assert!(
            text.contains("| made-up words in silent takes | 2 | 0 | 0 |"),
            "{text}"
        );
    }
}
