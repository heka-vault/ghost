//! stt-engines: Ghost's speech to text against transcribe.cpp's engines on the operator's
//! recorded Hearing session (docs/benchmarks/2026-11-15-stt-engines.md).
//!
//! The takes are the operator's voice: nothing here writes or prints what a read heard.
//! Every output file holds numbers only. With `STT_ENGINES_SPANS=<spans.json>`, `tc` and
//! `ort` also read each take's turn span (the harness's `turn` read).
//!
//! ```text
//! stt-engines tc      <label> <model.gguf> <cpu|vulkan> <threads> <session> <out>
//! stt-engines stream  <label> <model.gguf> <cpu|vulkan> <threads> <session> <out> [rc=N|lcr=L,C,R]
//! stt-engines ort     <label> <fp32|int8> <session> <out>
//! stt-engines interim <label> <fp32|int8> <interim secs> <session> <out>
//! stt-engines spans   <session> <spans.json>
//! stt-engines clock   <tdt-v3.gguf> <session> <clock.json>
//! stt-engines pipe    <reps> <out>
//! stt-engines spawn   <reps> <tdt-v3.gguf> <session> <out>
//! stt-engines compare <baseline results.json> <runs dir> [baseline read] [candidate read]
//! stt-engines arrivals <clock.json> <interim dir> <stream run dir>...
//! stt-engines devices
//! ```

mod common;
mod compare;
mod engines;
mod pipe;
mod span;

use std::path::{Path, PathBuf};

use engines::Device;

fn device(s: &str) -> anyhow::Result<Device> {
    Ok(match s {
        "cpu" => Device::Cpu,
        "vulkan" => Device::Vulkan,
        _ => anyhow::bail!("cpu or vulkan"),
    })
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a = |i: usize| -> anyhow::Result<&str> {
        args.get(i)
            .map(String::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing argument {i}; see the module doc"))
    };
    let p = |i: usize| -> anyhow::Result<PathBuf> { Ok(PathBuf::from(a(i)?)) };
    match a(0)? {
        "tc" | "stream" => {
            let items = common::takes(&p(5)?)?;
            let out = p(6)?.join(a(1)?);
            let threads: i32 = a(4)?.parse()?;
            if a(0)? == "tc" {
                engines::tc_batch(a(1)?, &p(2)?, device(a(3)?)?, threads, &items, &out)
            } else {
                engines::tc_stream(
                    a(1)?,
                    &p(2)?,
                    device(a(3)?)?,
                    threads,
                    args.get(7).map(String::as_str),
                    &items,
                    &out,
                )
            }
        }
        "ort" => {
            let items = common::takes(&p(3)?)?;
            engines::ort_batch(a(1)?, a(2)?, &items, &p(4)?.join(a(1)?))
        }
        "interim" => {
            let items = common::takes(&p(4)?)?;
            engines::ort_interim(
                a(1)?,
                a(2)?,
                a(3)?.parse()?,
                &p(4)?,
                &items,
                &p(5)?.join(a(1)?),
            )
        }
        "spans" => span::spans(&common::takes(&p(1)?)?, &p(2)?),
        "clock" => engines::clock(&p(1)?, &common::takes(&p(2)?)?, &p(3)?),
        "pipe" => pipe::round_trip(a(1)?.parse()?, &p(2)?),
        "spawn" => {
            let items = common::takes(&p(3)?)?;
            // The first spoken take of about five seconds.
            let take = items
                .iter()
                .filter(|i| common::spoken(i))
                .min_by(|x, y| {
                    (x.seconds() - 5.0)
                        .abs()
                        .total_cmp(&(y.seconds() - 5.0).abs())
                })
                .expect("a spoken take");
            let audio = common::audio(take);
            let what = [
                ("ort fp32", "ort:fp32".to_string()),
                ("ort int8", "ort:int8".to_string()),
                ("transcribe-cpp tdt-v3", format!("tc:{}", a(2)?)),
            ];
            pipe::spawn_load(&what, &audio, a(1)?.parse()?, &p(4)?)
        }
        "worker" => pipe::worker(),
        "compare" => compare::compare(
            &p(1)?,
            &p(2)?,
            args.get(3).map_or("file", String::as_str),
            args.get(4).map_or("file", String::as_str),
        ),
        "arrivals" => {
            let runs: Vec<PathBuf> = args[3..].iter().map(PathBuf::from).collect();
            let refs: Vec<&Path> = runs.iter().map(PathBuf::as_path).collect();
            compare::arrivals(&p(1)?, &p(2)?, &refs)
        }
        "devices" => engines::devices(),
        other => anyhow::bail!("unknown command {other}; see the module doc"),
    }
}
