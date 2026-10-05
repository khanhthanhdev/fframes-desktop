//! ACP driver: the legacy wire-shape structs, secret redaction and the production
//! SDK-based v1 driver (`AcpDriver`).

mod events;
mod redact;
mod runtime;
mod transcript;
mod transport;

pub use events::{
    AgentCapabilityInfo, AgentEvent, AgentEventKind, AgentFailure, AgentOptions, ClosedInfo,
    ConfigKind, ConfigOption, ConfigValue, EventQueue, FailureKind, InitializedInfo, MessageRole,
    ModeOption, OptionValue, PermissionChoice, PermissionId, PermissionPrompt, PermissionReply,
    PermissionResolution, Phase, PromptOutcome, StopReasonKind, ToolEvent, ToolStatus,
};
pub use redact::{REDACTION_MARK, Redactor, StreamRedactor};
pub use runtime::{
    AcpDriver, DriverConfig, DriverError, DriverLimits, DriverMode, DriverOutcome, DriverStatus,
    McpStdioSupport, SessionInfo,
};
pub use transcript::{Transcript, TranscriptEntry, TranscriptPage};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const ACP_PROTOCOL_VERSION: &str = "1";

/// Why a task-specific MCP stdio server configuration was rejected. Messages never
/// contain environment values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpConfigError {
    #[error("MCP server name must not be empty")]
    EmptyName,
    #[error("MCP server '{name}': command must be an absolute path")]
    RelativeCommand { name: String },
    #[error("MCP server '{name}': command must be valid UTF-8")]
    NonUtf8Command { name: String },
    #[error("MCP server '{name}': environment variable name must be non-empty and not contain '='")]
    InvalidEnvName { name: String },
    #[error("MCP server '{name}': {field} must not contain NUL")]
    Nul { name: String, field: &'static str },
}

/// One stdio MCP server the agent should launch for a session (ACP v1 `McpServer::Stdio`).
///
/// Env values may be secrets: the driver adds every value to its redaction set and
/// `Debug` prints only the variable names. Fields are public for reading; build with
/// [`McpStdioServer::new`] (the driver and legacy session re-validate before sending).
#[derive(Clone, PartialEq, Eq)]
pub struct McpStdioServer {
    pub name: String,
    /// Absolute path to the server executable.
    pub command: PathBuf,
    pub args: Vec<String>,
    /// `(name, value)` pairs.
    pub env: Vec<(String, String)>,
}

impl McpStdioServer {
    pub fn new(
        name: impl Into<String>,
        command: impl Into<PathBuf>,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<Self, McpConfigError> {
        let server = Self {
            name: name.into(),
            command: command.into(),
            args,
            env,
        };
        server.validate()?;
        Ok(server)
    }

    /// Checks the invariants [`Self::new`] enforces; for values built via the public fields.
    pub fn validate(&self) -> Result<(), McpConfigError> {
        let name = &self.name;
        let nul = |field: &'static str| McpConfigError::Nul {
            name: name.clone(),
            field,
        };
        if name.is_empty() {
            return Err(McpConfigError::EmptyName);
        }
        if name.contains('\0') {
            return Err(nul("name"));
        }
        if !self.command.is_absolute() {
            return Err(McpConfigError::RelativeCommand { name: name.clone() });
        }
        let Some(command) = self.command.to_str() else {
            return Err(McpConfigError::NonUtf8Command { name: name.clone() });
        };
        if command.contains('\0') {
            return Err(nul("command"));
        }
        if self.args.iter().any(|arg| arg.contains('\0')) {
            return Err(nul("args"));
        }
        for (key, value) in &self.env {
            if key.is_empty() || key.contains('=') {
                return Err(McpConfigError::InvalidEnvName { name: name.clone() });
            }
            if key.contains('\0') {
                return Err(nul("environment variable name"));
            }
            if value.contains('\0') {
                return Err(nul("environment variable value"));
            }
        }
        Ok(())
    }

    /// Every env value; these are redacted from all driver output.
    pub fn secrets(&self) -> impl Iterator<Item = &str> {
        self.env.iter().map(|(_, value)| value.as_str())
    }

    /// ACP v1 wire shape: `{"name","command","args","env":[{"name","value"}]}`.
    pub fn to_wire_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "command": self.command,
            "args": self.args,
            "env": self
                .env
                .iter()
                .map(|(name, value)| serde_json::json!({"name": name, "value": value}))
                .collect::<Vec<_>>(),
        })
    }
}

impl std::fmt::Debug for McpStdioServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpStdioServer")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args)
            .field(
                "env",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpInitializeRequest {
    pub protocol_version: String,
    pub client_name: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpInitializeResponse {
    pub protocol_version: String,
    pub server_info: ServerInfo,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpSessionRequest {
    pub working_dir: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpSessionResponse {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpPromptChunk {
    pub session_id: String,
    pub chunk_text: String,
    pub is_done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpPermissionRequest {
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpPermissionResponse {
    pub session_id: String,
    pub tool_call_id: String,
    pub granted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpAuthoritativeCompletion {
    pub session_id: String,
    pub stop_reason: String,
    pub modified_files: Vec<String>,
}

/// Redacts known secret patterns and tokens from diagnostic strings before persistence.
pub fn redact_sensitive_string(raw: &str) -> String {
    let mut sanitized = raw.to_string();

    // Redact Bearer / API token patterns
    let patterns = [
        ("Bearer ", 7),
        ("token=", 6),
        ("api_key=", 8),
        ("sk-", 3),
        ("ghp_", 4),
    ];

    for (prefix, len) in patterns {
        let mut search_idx = 0;
        while let Some(pos) = sanitized[search_idx..].find(prefix) {
            let abs_pos = search_idx + pos + len;
            // Find end of token (whitespace or quote or semicolon or comma or end of string)
            let end_pos = sanitized[abs_pos..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ';' || c == ',')
                .map(|e| abs_pos + e)
                .unwrap_or(sanitized.len());

            if end_pos > abs_pos {
                sanitized.replace_range(abs_pos..end_pos, "[REDACTED]");
                search_idx = abs_pos + "[REDACTED]".len();
            } else {
                search_idx = abs_pos;
            }

            if search_idx >= sanitized.len() {
                break;
            }
        }
    }

    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redaction_sanitizes_tokens() {
        let input = "Authorization: Bearer secret_token_xyz123; user_id=42";
        let redacted = redact_sensitive_string(input);
        assert!(!redacted.contains("secret_token_xyz123"));
        assert!(redacted.contains("Bearer [REDACTED]"));

        let key_input = "Connecting with api_key=sk-ant-12345678 to host";
        let key_redacted = redact_sensitive_string(key_input);
        assert!(!key_redacted.contains("sk-ant-12345678"));
        assert!(key_redacted.contains("[REDACTED]"));
    }
}
