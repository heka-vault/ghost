//! What Ghost made, on Ghost's own app (ADR 0033, ADR 0052): `output_read`, a read within
//! the result budget, and `output_delete`, a change through the gate. Writing and replacing
//! stay `outputs.write` and `outputs.replace`. Only inside `out/`, never through a link.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{GhostServer, reply};
use crate::apps::MAX_RESULT_TEXT;
use crate::outputs::{delete_output, read_output};

/// What one read returns of a file: the result budget, less room for the rest.
pub const READ_CHARS: usize = MAX_RESULT_TEXT - 1_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OutputRead {
    /// The file's name in Ghost's outputs, as list_outputs gives it.
    pub name: String,
    /// Where to start, in characters (0 unless given): next_offset from the last read.
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OutputDelete {
    /// The file's name in Ghost's outputs.
    pub name: String,
}

impl GhostServer {
    fn config_dir(&self) -> Result<std::path::PathBuf, String> {
        self.more()
            .config_dir
            .clone()
            .ok_or_else(|| "Ghost's folder is not available here".to_string())
    }

    pub(crate) fn do_output_read(&self, p: &OutputRead) -> Result<Value, String> {
        let text = read_output(&self.config_dir()?, &p.name).map_err(|e| e.to_string())?;
        let total = text.chars().count();
        let start = p.offset.unwrap_or(0).min(total);
        let part: String = text.chars().skip(start).take(READ_CHARS).collect();
        let end = start + part.chars().count();
        let mut out = json!({
            "name": p.name.trim(),
            "characters": total,
            "offset": start,
            "text": part,
        });
        if end < total {
            out["next_offset"] = json!(end);
            out["note"] = json!("There is more: read again with offset next_offset.");
        }
        Ok(out)
    }

    pub(crate) fn do_output_delete(&self, p: &OutputDelete) -> Result<Value, String> {
        delete_output(&self.config_dir()?, &p.name).map_err(|e| e.to_string())?;
        Ok(json!({"name": p.name.trim(), "status": "deleted"}))
    }
}

#[tool_router(router = outputs_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "Read a file Ghost made in its outputs folder, a part at a time when it is long.",
        annotations(read_only_hint = true)
    )]
    fn output_read(&self, Parameters(p): Parameters<OutputRead>) -> CallToolResult {
        reply(self.do_output_read(&p))
    }

    #[tool(description = "Delete a file from Ghost's outputs folder.")]
    fn output_delete(&self, Parameters(p): Parameters<OutputDelete>) -> CallToolResult {
        reply(self.do_output_delete(&p))
    }
}
