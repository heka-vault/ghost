//! Generic target/writer pairs so simple abilities don't write their own.

pub mod files;

pub use crate::models::{AppendText, FileSnapshot, FileTarget};
pub use files::{FileWriter, FilesError};
