//! Minimal Model Context Protocol server over stdio, implemented here without an MCP
//! crate. It exposes the six project tools and forwards every `tools/call` unchanged
//! (same method, `arguments` verbatim) to the app-owned broker ([`super::broker`]).
//!
//! # Qualification
//! [`MCP_QUALIFICATION`]: this server has **not** been exercised against any real MCP
//! client, provider or ACP adapter (none exists on this machine). It is exercised only by
//! in-process and subprocess tests that speak the protocol directly. `studio-mcp
//! --protocol` prints [`MCP_PROTOCOL_VERSION`] and this qualification.
//!
//! # Behavior
//! - Transport: newline-delimited JSON-RPC 2.0, one message per line. Stdout carries
//!   nothing but responses; logs go to stderr, bounded to 16 KiB in total.
//! - `initialize` answers with [`MCP_PROTOCOL_VERSION`] whatever version the client asked
//!   for (per the lifecycle spec the client then decides whether to continue).
//!   Capabilities: `{"tools":{"listChanged":false}}`.
//! - Notifications (no `id`) never get a response. `ping` answers `{}`.
//! - `tools/list` returns exactly the six [`tool_descriptions`].
//! - `tools/call`: an unknown tool name is JSON-RPC error `-32602`; otherwise the call
//!   goes to the broker. A reply becomes `{content:[{type:"text",text:<compact reply
//!   JSON>}],structuredContent:<reply>,isError:false}`; a tool error becomes
//!   `{content:[{type:"text",text:<compact {code,message}>}],isError:true}` (a tool
//!   failure is not a JSON-RPC error).
//! - The broker connection is opened lazily on the first `tools/call`, so `initialize`
//!   and `tools/list` work without a broker. A connection the broker closed is dropped
//!   and re-established by the next call.
//! - Errors: `-32700` invalid JSON (id `null`); `-32600` invalid request, JSON batches,
//!   non-object messages and lines over 1 MiB (rejected without parsing); `-32601`
//!   unknown method; `-32602` invalid params; [`NOT_INITIALIZED`] (`-32002`) for
//!   `tools/list`/`tools/call` before `initialize`.
use super::{
    TOOL_NAMES, ToolError, ToolErrorCode,
    client::{BrokerClient, CAPABILITY_ENV, EXIT_OK, EXIT_USAGE, LineRead, LineReader},
    tool_descriptions,
};
use serde_json::{Map, Value, json};
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
};

/// The MCP protocol revision this server implements.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
pub const MCP_QUALIFICATION: &str =
    "unqualified: not exercised against any real provider/adapter (none available on this machine)";
/// Longest accepted input line.
pub const MAX_MCP_LINE_BYTES: usize = 1024 * 1024;
/// Total stderr output of `studio-mcp`; later messages are dropped.
pub const MAX_LOG_BYTES: usize = 16 * 1024;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
/// Request that needs `initialize` first (MCP-style "server not initialized").
pub const NOT_INITIALIZED: i64 = -32002;

/// One open broker connection, as far as the MCP handler is concerned.
pub trait ToolCaller {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, ToolError>;
}

impl ToolCaller for BrokerClient {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
        BrokerClient::call(self, method, params)
    }
}

type Connector = Box<dyn FnMut() -> Result<Box<dyn ToolCaller>, ToolError>>;

pub struct McpServer {
    connector: Connector,
    caller: Option<Box<dyn ToolCaller>>,
    initialized: bool,
    diagnostics: Vec<String>,
}

fn response(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn failure(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

impl McpServer {
    /// `connector` opens the broker connection; it is called lazily by `tools/call`.
    pub fn new(
        connector: impl FnMut() -> Result<Box<dyn ToolCaller>, ToolError> + 'static,
    ) -> Self {
        Self {
            connector: Box::new(connector),
            caller: None,
            initialized: false,
            diagnostics: Vec::new(),
        }
    }

    /// The `-32600` answer for a line that exceeded [`MAX_MCP_LINE_BYTES`].
    pub fn line_too_long_response() -> String {
        failure(
            &Value::Null,
            INVALID_REQUEST,
            "line exceeds the 1 MiB message limit",
        )
    }

    /// Short messages worth logging (tool failures, broker connection problems).
    pub fn drain_diagnostics(&mut self) -> Vec<String> {
        std::mem::take(&mut self.diagnostics)
    }

    fn note(&mut self, message: String) {
        if self.diagnostics.len() < 16 {
            self.diagnostics.push(message);
        }
    }

    /// One input line in, the single-line JSON-RPC response out (`None` for
    /// notifications and blank lines).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        self.handle_bytes(line.as_bytes())
    }

    pub fn handle_bytes(&mut self, line: &[u8]) -> Option<String> {
        if line.len() > MAX_MCP_LINE_BYTES {
            return Some(Self::line_too_long_response());
        }
        let text = line.trim_ascii();
        if text.is_empty() {
            return None;
        }
        let Ok(value) = serde_json::from_slice::<Value>(text) else {
            return Some(failure(&Value::Null, PARSE_ERROR, "parse error"));
        };
        let Value::Object(message) = value else {
            return Some(failure(
                &Value::Null,
                INVALID_REQUEST,
                "only single JSON-RPC request objects are supported (no batches)",
            ));
        };
        let Some(method) = message.get("method") else {
            // A response to a server request: this server sends none, so ignore it.
            if message.contains_key("result") || message.contains_key("error") {
                return None;
            }
            return Some(failure(&Value::Null, INVALID_REQUEST, "missing `method`"));
        };
        let id = match message.get("id") {
            None => None,
            Some(id @ (Value::String(_) | Value::Number(_))) => Some(id.clone()),
            Some(_) => {
                return Some(failure(
                    &Value::Null,
                    INVALID_REQUEST,
                    "`id` must be a string or a number",
                ));
            }
        };
        let valid =
            message.get("jsonrpc").and_then(Value::as_str) == Some("2.0") && method.is_string();
        let Some(id) = id else {
            // Notifications (`notifications/initialized`, `notifications/cancelled`, ...)
            // need no answer, even malformed ones.
            return None;
        };
        if !valid {
            return Some(failure(
                &id,
                INVALID_REQUEST,
                "expected jsonrpc \"2.0\" and a string `method`",
            ));
        }
        let method = method.as_str().unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(self.dispatch(&id, method, params))
    }

    fn dispatch(&mut self, id: &Value, method: &str, params: Value) -> String {
        match method {
            "ping" => response(id, json!({})),
            "initialize" => {
                let Some(params) = params.as_object() else {
                    return failure(id, INVALID_PARAMS, "initialize needs a params object");
                };
                if !params.get("protocolVersion").is_some_and(Value::is_string) {
                    return failure(id, INVALID_PARAMS, "initialize needs a protocolVersion");
                }
                self.initialized = true;
                response(
                    id,
                    json!({
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {
                            "name": "fframes-studio",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                    }),
                )
            }
            "tools/list" | "tools/call" if !self.initialized => {
                failure(id, NOT_INITIALIZED, "send `initialize` first")
            }
            "tools/list" => response(id, json!({"tools": tool_descriptions()})),
            "tools/call" => self.call_tool(id, &params),
            other => failure(
                id,
                METHOD_NOT_FOUND,
                &format!(
                    "unknown method `{}`",
                    other.chars().take(64).collect::<String>()
                ),
            ),
        }
    }

    fn call_tool(&mut self, id: &Value, params: &Value) -> String {
        let Some(params) = params.as_object() else {
            return failure(id, INVALID_PARAMS, "tools/call needs a params object");
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return failure(id, INVALID_PARAMS, "tools/call needs a tool `name`");
        };
        if !TOOL_NAMES.contains(&name) {
            return failure(
                id,
                INVALID_PARAMS,
                &format!(
                    "unknown tool `{}`",
                    name.chars().take(64).collect::<String>()
                ),
            );
        }
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => Value::Object(Map::new()),
            Some(arguments @ Value::Object(_)) => arguments.clone(),
            Some(_) => return failure(id, INVALID_PARAMS, "`arguments` must be an object"),
        };
        match self.forward(name, arguments) {
            Ok(value) => {
                let text = serde_json::to_string(&value).unwrap_or_default();
                response(
                    id,
                    json!({
                        "content": [{"type": "text", "text": text}],
                        "structuredContent": value,
                        "isError": false,
                    }),
                )
            }
            Err(error) => {
                self.note(format!("tool {name} failed: {:?}", error.code));
                let text = serde_json::to_string(&error).unwrap_or_default();
                response(
                    id,
                    json!({"content": [{"type": "text", "text": text}], "isError": true}),
                )
            }
        }
    }

    fn forward(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
        if self.caller.is_none() {
            match (self.connector)() {
                Ok(caller) => self.caller = Some(caller),
                Err(error) => {
                    self.note(format!("broker connection failed: {:?}", error.code));
                    return Err(error);
                }
            }
        }
        let Some(caller) = self.caller.as_mut() else {
            return Err(ToolError::new(
                ToolErrorCode::Internal,
                "no broker connection",
            ));
        };
        let result = caller.call(method, params);
        if let Err(error) = &result
            && matches!(
                error.code,
                ToolErrorCode::Unavailable
                    | ToolErrorCode::Expired
                    | ToolErrorCode::StaleTask
                    | ToolErrorCode::Unauthorized
            )
        {
            // The broker closed (or the transport broke): reconnect on the next call.
            self.caller = None;
        }
        result
    }
}

/// stderr with a hard total budget; once spent, everything is dropped silently.
struct BoundedLog<'a> {
    sink: &'a mut dyn Write,
    written: usize,
    exhausted: bool,
}

const LOG_LIMIT_NOTICE: &str = "studio-mcp: log limit reached, further messages suppressed\n";

impl<'a> BoundedLog<'a> {
    fn new(sink: &'a mut dyn Write) -> Self {
        Self {
            sink,
            written: 0,
            exhausted: false,
        }
    }

    fn log(&mut self, message: &str) {
        if self.exhausted {
            return;
        }
        let message: String = message.chars().take(512).collect();
        let line = format!("studio-mcp: {message}\n");
        if self.written + line.len() + LOG_LIMIT_NOTICE.len() > MAX_LOG_BYTES {
            let _ = self.sink.write_all(LOG_LIMIT_NOTICE.as_bytes());
            self.exhausted = true;
            return;
        }
        if self.sink.write_all(line.as_bytes()).is_ok() {
            self.written += line.len();
        }
    }
}

pub const MCP_USAGE: &str = "usage: studio-mcp [--capability <file>]\n       studio-mcp --protocol\n       studio-mcp --help\n\nA stdio MCP server: newline-delimited JSON-RPC on stdin/stdout. The capability file path\nmay also come from FFRAMES_STUDIO_CAPABILITY. Logs go to stderr (at most 16 KiB).\n";

/// The whole `studio-mcp` program. `args` excludes the program name. Exit codes: 0 on
/// stdin EOF (or `--protocol`/`--help`), 2 for usage errors, 1 when stdout breaks.
pub fn run_stdio(
    args: Vec<OsString>,
    env_capability: Option<OsString>,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut log = BoundedLog::new(stderr);
    let mut capability: Option<PathBuf> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str().map(str::to_string) else {
            log.log("arguments must be valid UTF-8");
            return EXIT_USAGE;
        };
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--protocol" if inline.is_none() => {
                let _ = writeln!(
                    stdout,
                    "MCP protocol {MCP_PROTOCOL_VERSION}\n{MCP_QUALIFICATION}"
                );
                return EXIT_OK;
            }
            "--help" | "-h" if inline.is_none() => {
                let _ = stdout.write_all(MCP_USAGE.as_bytes());
                return EXIT_OK;
            }
            "--capability" => {
                let value = match inline.or_else(|| args.next().and_then(|v| v.into_string().ok()))
                {
                    Some(value) if !value.is_empty() => value,
                    _ => {
                        log.log("--capability needs a file path");
                        return EXIT_USAGE;
                    }
                };
                if capability.replace(PathBuf::from(value)).is_some() {
                    log.log("--capability given twice");
                    return EXIT_USAGE;
                }
            }
            other => {
                log.log(&format!(
                    "unknown argument `{}`",
                    other.chars().take(64).collect::<String>()
                ));
                log.log(MCP_USAGE);
                return EXIT_USAGE;
            }
        }
    }
    let Some(capability) = capability.or_else(|| {
        env_capability
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }) else {
        log.log(&format!(
            "no capability file (use --capability or {CAPABILITY_ENV})"
        ));
        return EXIT_USAGE;
    };
    let mut server = McpServer::new(move || {
        BrokerClient::connect(&capability)
            .map(|client| Box::new(client) as Box<dyn ToolCaller>)
            .map_err(|error| error.to_tool_error())
    });
    log.log(&format!(
        "serving MCP {MCP_PROTOCOL_VERSION} on stdio ({MCP_QUALIFICATION})"
    ));
    let mut reader = LineReader::new(MAX_MCP_LINE_BYTES).accepting_final_partial();
    loop {
        let reply = match reader.next(stdin, true, None) {
            Ok(LineRead::Line(line)) => server.handle_bytes(&line),
            Ok(LineRead::TooLong) => Some(McpServer::line_too_long_response()),
            Ok(LineRead::Idle) => continue,
            Ok(LineRead::Eof) => return EXIT_OK,
            Err(error) => {
                log.log(&format!("stdin failed: {error}"));
                return 1;
            }
        };
        if let Some(reply) = reply
            && writeln!(stdout, "{reply}")
                .and_then(|()| stdout.flush())
                .is_err()
        {
            log.log("stdout closed");
            return 1;
        }
        for message in server.drain_diagnostics() {
            log.log(&message);
        }
    }
}
