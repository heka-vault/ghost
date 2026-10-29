//! `ghost-perf`: the latency harness (ADR 0018), the port of `ghostd.perf`.
//!
//! Scripted conversations through `build_session`, headless, from the operator's lines
//! rendered once by Kokoro in another voice and played in at real-time pace, with the real
//! Silero, Parakeet, Smart Turn and Kokoro, the real gate and the mock GitHub app, and a
//! scripted model whose replies are the scenario's (`llm::ScriptedModel`), so the stages
//! around the model are what is measured. Each run gets a fresh temporary state directory:
//! its own SQLite, agents, mock GitHub and latency file, so runs are independent and the
//! operator's state is never read or written. Reports land in `.scratch/perf/<timestamp>/`
//! as `records.json`, `latency.jsonl` and `report.md`, in the shape of
//! `docs/benchmarks/perf/`.

pub mod audio;
pub mod cli;
pub mod harness;
pub mod llm;
pub mod micro;
pub mod report;
pub mod scenarios;
