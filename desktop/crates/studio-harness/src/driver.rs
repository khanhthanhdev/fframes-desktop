//! Agent drivers: the [`AgentDriver`] trait every wire implements, secret
//! redaction and the production SDK-based ACP v1 driver (`AcpDriver`).

mod api;
mod events;
mod redact;
mod runtime;
mod transcript;
mod transport;

pub use api::{
    AgentDriver, AgentWire, DriverCapabilities, QuestionAnswer, QuestionItem, QuestionOption,
    QuestionPrompt, SteeringMode, start_driver,
};
pub use events::{
    AgentCapabilityInfo, AgentEvent, AgentEventKind, AgentFailure, AgentOptions, ClosedInfo,
    CommandInfo, ConfigKind, ConfigOption, ConfigValue, EventQueue, FailureKind, InitializedInfo,
    MessageRole, ModeOption, OptionValue, PermissionChoice, PermissionId, PermissionPrompt,
    PermissionReply, PermissionResolution, Phase, PlanEntry, PlanStatus, PromptOutcome,
    StopReasonKind, ToolEvent, ToolStatus, TurnOrigin,
};
pub use redact::{REDACTION_MARK, Redactor, StreamRedactor};
pub use runtime::{
    AcpDriver, ClaudeDriver, CodexDriver, DriverConfig, DriverError, DriverLimits, DriverMode,
    DriverOutcome, DriverStatus, McpStdioSupport, OwnershipSource, PromptFile, PromptImage,
    SessionInfo,
};
pub use transcript::{Transcript, TranscriptEntry, TranscriptPage};

use std::path::PathBuf;

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
    #[error("MCP server '{name}': the URL must start with http:// or https://")]
    InvalidUrl { name: String },
    #[error("MCP server '{name}': a header name or value is not valid")]
    InvalidHeader { name: String },
}

/// One stdio MCP server the agent should launch for a session (ACP v1 `McpServer::Stdio`).
///
/// Env values may be secrets: the driver adds every value to its redaction set and
/// `Debug` prints only the variable names. Fields are public for reading; build with
/// [`McpStdioServer::new`] (the driver re-validates before sending).
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

/// One streamable-HTTP MCP server (ACP v1 `McpServer::Http`). Header values may be
/// secrets: the driver adds them to its redaction set, they never reach a command line, and
/// `Debug` prints only the header names.
#[derive(Clone, PartialEq, Eq)]
pub struct McpHttpServer {
    pub name: String,
    /// `http://` or `https://` endpoint.
    pub url: String,
    /// `(header, value)` pairs sent with every request.
    pub headers: Vec<(String, String)>,
}

impl McpHttpServer {
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        headers: Vec<(String, String)>,
    ) -> Result<Self, McpConfigError> {
        let server = Self {
            name: name.into(),
            url: url.into(),
            headers,
        };
        server.validate()?;
        Ok(server)
    }

    pub fn validate(&self) -> Result<(), McpConfigError> {
        let name = &self.name;
        if name.is_empty() {
            return Err(McpConfigError::EmptyName);
        }
        if name.contains('\0') {
            return Err(McpConfigError::Nul {
                name: name.clone(),
                field: "name",
            });
        }
        let lower = self.url.to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://"))
            || self
                .url
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(McpConfigError::InvalidUrl { name: name.clone() });
        }
        for (header, value) in &self.headers {
            if header.is_empty()
                || !header
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || value.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
            {
                return Err(McpConfigError::InvalidHeader { name: name.clone() });
            }
        }
        Ok(())
    }

    /// Every header value, and the token of an `Authorization: Bearer <token>` value on its
    /// own; these are redacted from all driver output.
    pub fn secrets(&self) -> impl Iterator<Item = &str> {
        self.headers.iter().flat_map(|(_, value)| {
            let token = value
                .strip_prefix("Bearer ")
                .filter(|token| !token.is_empty());
            std::iter::once(value.as_str()).chain(token)
        })
    }
}

impl std::fmt::Debug for McpHttpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHttpServer")
            .field("name", &self.name)
            .field("url", &self.url)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// How the agent reaches one of the user's MCP servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    Stdio(McpStdioServer),
    Http(McpHttpServer),
}

impl McpTransport {
    pub fn name(&self) -> &str {
        match self {
            Self::Stdio(server) => &server.name,
            Self::Http(server) => &server.name,
        }
    }

    pub fn validate(&self) -> Result<(), McpConfigError> {
        match self {
            Self::Stdio(server) => server.validate(),
            Self::Http(server) => server.validate(),
        }
    }

    /// The values to redact: environment values or header values.
    pub fn secrets(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Stdio(server) => Box::new(server.secrets()),
            Self::Http(server) => Box::new(server.secrets()),
        }
    }
}

/// One of the user's own MCP servers as a session launches it: never pre-approved, and
/// without the tools the user turned off (where the agent can filter them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMcpLaunch {
    pub transport: McpTransport,
    /// Tool names (as the server lists them) the agent must not use. Claude denies them,
    /// Codex leaves them out; ACP has no way to filter a server's tools.
    pub disabled_tools: Vec<String>,
}

impl UserMcpLaunch {
    pub fn new(transport: McpTransport) -> Self {
        Self {
            transport,
            disabled_tools: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        self.transport.name()
    }

    pub fn secrets(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        self.transport.secrets()
    }
}

impl From<McpStdioServer> for UserMcpLaunch {
    fn from(server: McpStdioServer) -> Self {
        Self::new(McpTransport::Stdio(server))
    }
}

impl From<McpHttpServer> for UserMcpLaunch {
    fn from(server: McpHttpServer) -> Self {
        Self::new(McpTransport::Http(server))
    }
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
