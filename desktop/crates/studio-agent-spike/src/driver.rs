use serde::{Deserialize, Serialize};

pub const ACP_PROTOCOL_VERSION: &str = "1";

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
