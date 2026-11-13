//! Pure data and object-safe contracts shared by ferrocore components.

use async_trait::async_trait;
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub loop_type: String,
    pub agent_depth: u32,
    pub history: Vec<Message>,
    pub turns: u64,
    pub total_tokens: u64,
    pub last_record_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub pwd: Option<String>,
    pub last_exit_code: Option<i32>,
    pub jobs: Option<Vec<String>>,
    pub scratch_names: Option<Vec<String>>,
    pub scratch_total: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopAction {
    ExecShell(String),
    Final(String),
    Malformed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopError(pub String);

impl fmt::Display for LoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoopError {}

#[async_trait]
pub trait AgentLoop: Send + Sync {
    async fn prepare_turn(
        &self,
        session: &Session,
        state: &TerminalSnapshot,
    ) -> Result<Vec<Message>, LoopError>;

    async fn on_model_response(&self, text: &str) -> Result<LoopAction, LoopError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ToolError {}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn call(&self, input: Value) -> Result<Value, ToolError>;
}
