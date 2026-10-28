//! The agents (ADR 0014, ADR 0035): a conversation with its own name, job, persona, voice
//! and private memory. The operator's word is *agent*; the code keeps *conversation*.

pub mod files;
pub mod models;
pub mod service;
pub mod writer;

pub use files::ConversationFiles;
pub use models::{Compaction, CompactionSource, GHOST, Profile, Role, TurnRecord, new_id, slugify};
pub use service::{
    Conversations, ConversationsOptions, Message, RECENT_TEMPLATE, ResumeContext, SOURCE,
    SUMMARY_TEMPLATE, conversations_source, names_arg,
};
pub use writer::ConversationWriter;
