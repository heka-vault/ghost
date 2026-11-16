//! What a child process would cost: the pipe's round trip in Handy's frame format with no
//! model, and a worker's spawn plus model load.
//!
//! `encode_message` / `read_message` follow Handy's `engine_supervisor/protocol.rs`
//! (MIT, github.com/cjpais/Handy at aca8985): one frame is
//! `[u32 json_len][json][u32 pcm_len][pcm bytes]`, lengths little-endian, PCM as
//! little-endian f32. The echo worker is shaped like Handy's `worker.rs`: protocol on a
//! duplicate of stdout, a thread reading stdin into a channel, the main loop answering.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::common::{self, SR};
use crate::engines::{self, Device};

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Load { what: String },
    Run { options: transcribe_cpp::RunOptions },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Loaded {
        load_secs: f64,
    },
    /// Handy's worker answers with the whole `Transcript`; the echo answers with one of
    /// the same shape and a sentence-long text, so the reply's JSON is realistic.
    Transcript(transcribe_cpp::Transcript),
    Words(usize),
    Error(String),
}

pub fn encode_message<T: Serialize>(message: &T, pcm: Option<&[f32]>) -> io::Result<Vec<u8>> {
    let json = serde_json::to_vec(message).map_err(io::Error::other)?;
    let pcm = pcm.unwrap_or(&[]);
    let mut frame = Vec::with_capacity(8 + json.len() + pcm.len() * 4);
    frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
    frame.extend_from_slice(&json);
    frame.extend_from_slice(&((pcm.len() * 4) as u32).to_le_bytes());
    for sample in pcm {
        frame.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(frame)
}

pub fn write_message<T: Serialize>(
    w: &mut impl Write,
    message: &T,
    pcm: Option<&[f32]>,
) -> io::Result<()> {
    w.write_all(&encode_message(message, pcm)?)?;
    w.flush()
}

pub fn read_message<T: for<'de> Deserialize<'de>>(
    r: &mut impl Read,
) -> io::Result<Option<(T, Vec<f32>)>> {
    let mut len = [0u8; 4];
    let mut filled = 0;
    while filled < 4 {
        match r.read(&mut len[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let mut json = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut json)?;
    let message = serde_json::from_slice(&json).map_err(io::Error::other)?;
    r.read_exact(&mut len)?;
    let mut bytes = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut bytes)?;
    let pcm = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Ok(Some((message, pcm)))
}

fn take_stdout_for_protocol() -> io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain fd duplication, as Handy's worker does.
    unsafe {
        let fd = libc::dup(libc::STDOUT_FILENO);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(std::fs::File::from_raw_fd(fd))
    }
}

enum Engine {
    None,
    Tc(Box<engines::Loaded>),
    Ort(Box<ghost_infer::Parakeet>),
}

/// The child: `worker` answers runs with a model it loads on request; with no model it
/// echoes (the pipe's cost alone).
pub fn worker() -> anyhow::Result<()> {
    let mut output = BufWriter::new(take_stdout_for_protocol()?);
    let (tx, rx) = mpsc::channel::<(Request, Vec<f32>)>();
    std::thread::spawn(move || {
        let mut input = BufReader::new(io::stdin().lock());
        while let Ok(Some(m)) = read_message::<Request>(&mut input) {
            if tx.send(m).is_err() {
                break;
            }
        }
        std::process::exit(0);
    });
    let mut engine = Engine::None;
    let echo = transcribe_cpp::Transcript {
        text: "Close issue four oh three and tell the team it shipped.".into(),
        raw_text: "Close issue four oh three and tell the team it shipped.".into(),
        ..Default::default()
    };
    while let Ok((req, pcm)) = rx.recv() {
        let resp = match req {
            Request::Load { what } => {
                let t = Instant::now();
                let r = if let Some(path) = what.strip_prefix("tc:") {
                    engines::init_backends(Device::Cpu)
                        .and_then(|_| engines::load_tc(Path::new(path), Device::Cpu, 4))
                        .map(|l| Engine::Tc(Box::new(l)))
                } else if let Some(q) = what.strip_prefix("ort:") {
                    engines::load_ort(q, 4).map(|(m, _)| Engine::Ort(Box::new(m)))
                } else {
                    Err(anyhow::anyhow!("load what?"))
                };
                match r {
                    Ok(e) => {
                        engine = e;
                        Response::Loaded {
                            load_secs: t.elapsed().as_secs_f64(),
                        }
                    }
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::Run { options } => match &mut engine {
                Engine::None => Response::Transcript(echo.clone()),
                Engine::Tc(l) => match l.session.run(&pcm, &options) {
                    Ok(t) => Response::Words(common::words(&t.text).len()),
                    Err(e) => Response::Error(e.to_string()),
                },
                Engine::Ort(m) => match m.transcribe(&pcm) {
                    Ok(t) => Response::Words(common::words(&t).len()),
                    Err(e) => Response::Error(e.to_string()),
                },
            },
        };
        write_message(&mut output, &resp, None)?;
    }
    Ok(())
}

struct Worker {
    child: Child,
    stdin: BufWriter<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Worker {
    fn spawn() -> anyhow::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = BufWriter::new(child.stdin.take().unwrap());
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    fn call(&mut self, req: &Request, pcm: Option<&[f32]>) -> anyhow::Result<Response> {
        write_message(&mut self.stdin, req, pcm)?;
        Ok(read_message::<Response>(&mut self.stdout)?
            .ok_or_else(|| anyhow::anyhow!("worker went away"))?
            .0)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn stats(xs: &[f64]) -> serde_json::Value {
    json!({
        "n": xs.len(),
        "p50_ms": common::quantile(xs, 0.5).unwrap_or(0.0) * 1e3,
        "p95_ms": common::quantile(xs, 0.95).unwrap_or(0.0) * 1e3,
        "max_ms": xs.iter().cloned().fold(0.0, f64::max) * 1e3,
    })
}

/// The round trip of 2, 5 and 15 s of PCM to an echo child and back, and the same frame
/// encoded and decoded in process (what the copies alone cost).
pub fn round_trip(reps: usize, out: &Path) -> anyhow::Result<()> {
    let mut w = Worker::spawn()?;
    let run = Request::Run {
        options: transcribe_cpp::RunOptions::default(),
    };
    let mut results = serde_json::Map::new();
    for secs in [2usize, 5, 15] {
        // A tone with noise: the bytes don't matter to the pipe, only their count.
        let pcm: Vec<f32> = (0..secs * SR)
            .map(|i| (i as f32 * 0.05).sin() * 0.1)
            .collect();
        for _ in 0..20 {
            w.call(&run, Some(&pcm))?;
        }
        let mut rt = Vec::with_capacity(reps);
        for _ in 0..reps {
            let t = Instant::now();
            let r = w.call(&run, Some(&pcm))?;
            rt.push(t.elapsed().as_secs_f64());
            assert!(matches!(r, Response::Transcript(_)));
        }
        let mut local = Vec::with_capacity(reps);
        for _ in 0..reps {
            let t = Instant::now();
            let frame = encode_message(&run, Some(&pcm))?;
            let (_m, back): (Request, Vec<f32>) = read_message(&mut frame.as_slice())?.unwrap();
            local.push(t.elapsed().as_secs_f64());
            assert_eq!(back.len(), pcm.len());
        }
        println!(
            "{secs:>2} s: round trip {}  in process encode+decode {}",
            stats(&rt),
            stats(&local)
        );
        results.insert(
            format!("{secs}s"),
            json!({"bytes": secs * SR * 4, "round_trip": stats(&rt), "encode_decode_in_process": stats(&local)}),
        );
    }
    std::fs::create_dir_all(out)?;
    std::fs::write(
        out.join("pipe.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    Ok(())
}

/// A worker spawned and a model loaded in it, `reps` times each, then its first and second
/// read of a 5 s take (the first read in a fresh process pays the graph's set-up).
pub fn spawn_load(
    what: &[(&str, String)],
    take: &[f32],
    reps: usize,
    out: &Path,
) -> anyhow::Result<()> {
    let mut results = serde_json::Map::new();
    for (label, spec) in what {
        let mut spawn_ready = Vec::new();
        let mut load_inside = Vec::new();
        let mut first_read = Vec::new();
        let mut second_read = Vec::new();
        for _ in 0..reps {
            let t = Instant::now();
            let mut w = Worker::spawn()?;
            let r = w.call(&Request::Load { what: spec.clone() }, None)?;
            let ready = t.elapsed().as_secs_f64();
            let Response::Loaded { load_secs } = r else {
                anyhow::bail!("{label}: {r:?}");
            };
            let run = Request::Run {
                options: transcribe_cpp::RunOptions {
                    timestamps: transcribe_cpp::TimestampKind::None,
                    ..Default::default()
                },
            };
            let t = Instant::now();
            w.call(&run, Some(take))?;
            first_read.push(t.elapsed().as_secs_f64());
            let t = Instant::now();
            w.call(&run, Some(take))?;
            second_read.push(t.elapsed().as_secs_f64());
            spawn_ready.push(ready);
            load_inside.push(load_secs);
            drop(w);
        }
        println!(
            "{label}: spawn to loaded {:?}, load inside {:?}, first read {:?}, second {:?}",
            spawn_ready, load_inside, first_read, second_read
        );
        results.insert(
            label.to_string(),
            json!({"spawn_to_loaded_secs": spawn_ready, "load_in_worker_secs": load_inside,
                   "first_read_secs": first_read, "second_read_secs": second_read,
                   "read_audio_secs": take.len() as f64 / SR as f64}),
        );
    }
    std::fs::create_dir_all(out)?;
    std::fs::write(
        out.join("spawn.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    Ok(())
}
