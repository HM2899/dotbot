//! Native Rust pi-agent runtime boundary shared by Bot turns and background jobs.
use std::path::PathBuf;
use anyhow::Result;
use serde_json::Value;
use tokio::sync::{mpsc,oneshot};

#[derive(Debug, Clone)]
pub struct TurnSpec {
    pub cwd: PathBuf,
    pub session: Option<String>,
    /// Native backend sessions belong to the team, independently of skill plugin paths.
    pub session_dir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Stable across a session's turns, so providers can cache it.
    pub system_prompt: String,
    /// This turn only (lessons, prepared skills, delegation data), sent after the system
    /// prompt and never stored in the session.
    pub turn_context: String,
    pub prompt: String,
    pub mcp: Value,
    pub env: Vec<(String, String)>,
    /// The skill bundle for this turn: the team's, or a candidate's under evaluation.
    pub plugin_dir: Option<PathBuf>,
    /// Images the user sent with the message: (media type, base64).
    pub images: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Session(String),
    TextDelta(String),
    Text(String),
    ToolUse { id: String, name: String, input: Value },
    ToolResult { id: String, text: String, images: Vec<(String, String)>, is_error: bool },
    /// What the engine is doing between model and tool events, e.g. "compacting".
    Status(String),
    Done { text: String, is_error: bool },
}

/// A running turn. Dropping or sending stop cancels model/tool I/O.
pub struct Turn {
    pub events: mpsc::UnboundedReceiver<Event>,
    pub stop: oneshot::Sender<()>,
}
pub fn start(spec: TurnSpec) -> Result<Turn> { crate::pi_agent::start(spec) }
pub async fn ask(launch: &crate::providers::Launch, model: Option<&str>, system: &str, prompt: &str, schema: &Value) -> Result<Value> {
    crate::pi_agent::ask(&launch.env,model,system,prompt,schema).await
}
pub async fn ask_for(launch: &crate::providers::Launch, model: Option<&str>, system: &str, prompt: &str, schema: &Value, _owner: (Option<&str>, &str)) -> Result<Value> {
    ask(launch,model,system,prompt,schema).await
}
