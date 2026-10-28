//! The agents (ADR 0014, ADR 0035): a conversation with its own name, job, persona, voice
//! and private memory. The operator's word is *agent*; the code keeps *conversation*.

pub mod models;

pub use models::{Compaction, CompactionSource, GHOST, Profile, Role, TurnRecord, new_id, slugify};
