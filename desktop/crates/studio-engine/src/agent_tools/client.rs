//! Broker client shared by the `studio-tools` CLI and the `studio-mcp` stdio server.
//!
//! A client is created from a *capability file* written by the app-owned broker
//! ([`super::broker`]): `{"version":1,"socket":..,"capability":..,"secret":..}`. Only the
//! file path ever travels on argv or through the environment; the secret stays in the
//! private file and is never printed, logged, put in an error or exposed through `Debug`.
//!
//! The wire protocol is newline-delimited JSON over the broker's local stream (a unix
//! socket, or a named pipe on Windows, see [`super::transport`]): a `hello` line, then
//! one `{"id":N,"method":..,"params":..}` request answered by one
//! `{"id":N,"result":..}` / `{"id":N,"error":{"code","message"}}` line. A client has a
//! single call in flight; replies are read through a bounded line reader.
//!
//! This module also owns the whole `studio-tools` command-line behavior
//! ([`run_cli`]) so the binary stays a three-line `main`.
use super::{MAX_REQUEST_BYTES, MAX_TEXT_REPLY_BYTES};
use super::{TOOL_NAMES, ToolError, ToolErrorCode, tool_descriptions};
use serde_json::{Value, json};
use std::fmt;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The capability file is a few hundred bytes; anything larger is refused unread.
pub const MAX_CAPABILITY_FILE_BYTES: usize = 4096;
/// Default wall-clock limit for one call (builds and renders may be slow).
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Environment variable naming the capability file (path only, never the secret).
pub const CAPABILITY_ENV: &str = "FFRAMES_STUDIO_CAPABILITY";
pub const EXIT_OK: i32 = 0;
pub const EXIT_TOOL_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_UNAVAILABLE: i32 = 3;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// A reply is at most the bounded JSON text plus the framing around it.
const MAX_REPLY_LINE_BYTES: usize = MAX_TEXT_REPLY_BYTES + 4096;

/// Why a client could not be created.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("capability file unusable: {0}")]
    CapabilityFile(String),
    #[error("broker unreachable: {0}")]
    Unreachable(String),
    /// The broker verified the request and refused the capability (expired, revoked,
    /// stale task) or could not authenticate it.
    #[error("broker rejected the capability: {0}")]
    Rejected(ToolError),
    #[error("broker protocol error: {0}")]
    Protocol(String),
}

impl ClientError {
    /// The error as it is shown to tools: rejections keep their code, everything else is
    /// `unavailable`.
    pub fn to_tool_error(&self) -> ToolError {
        match self {
            Self::Rejected(error) => error.clone(),
            other => ToolError::new(ToolErrorCode::Unavailable, other.to_string()),
        }
    }
}

/// A failed call: the tool itself answered with an error, or the transport broke.
#[derive(Debug, thiserror::Error)]
pub enum CallFailure {
    #[error("{0}")]
    Tool(ToolError),
    #[error("broker transport failed: {0}")]
    Transport(String),
}

impl CallFailure {
    pub fn into_tool_error(self) -> ToolError {
        match self {
            Self::Tool(error) => error,
            Self::Transport(message) => ToolError::new(ToolErrorCode::Unavailable, message),
        }
    }
}

pub(crate) enum LineRead {
    Line(Vec<u8>),
    Eof,
    /// No complete line yet: the read timed out or reading was not allowed.
    Idle,
    /// The line exceeded the limit; its bytes are dropped, never buffered.
    TooLong,
}

/// Newline-delimited reader that never buffers more than `limit` plus one read chunk.
/// Partial lines survive read timeouts, so it can be driven by short socket timeouts.
/// `next` also takes an absolute `until`: once it passes, the call returns `Idle` even if
/// the peer keeps delivering bytes, so a one-byte-at-a-time peer can never keep a caller
/// inside the read loop past its deadline or past its cancellation checks.
#[derive(Debug)]
pub(crate) struct LineReader {
    buf: Vec<u8>,
    limit: usize,
    skipping: bool,
    final_partial: bool,
}

impl LineReader {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            limit,
            skipping: false,
            final_partial: false,
        }
    }

    /// Deliver a last unterminated line at end of stream (stdio pipes).
    pub(crate) fn accepting_final_partial(mut self) -> Self {
        self.final_partial = true;
        self
    }

    /// Next complete line. Buffered lines are always delivered first; without one, the
    /// reader reads from `source` until a line, end of stream, an error, the read
    /// timeout, or `until` (when given) ends the call with `Idle`.
    pub(crate) fn next<R: Read + ?Sized>(
        &mut self,
        source: &mut R,
        may_read: bool,
        until: Option<Instant>,
    ) -> io::Result<LineRead> {
        loop {
            if self.skipping {
                match self.buf.iter().position(|b| *b == b'\n') {
                    Some(end) => {
                        self.buf.drain(..=end);
                        self.skipping = false;
                    }
                    None => self.buf.clear(),
                }
            }
            if !self.skipping {
                if let Some(end) = self.buf.iter().position(|b| *b == b'\n') {
                    if end > self.limit {
                        self.buf.drain(..=end);
                        return Ok(LineRead::TooLong);
                    }
                    let mut line: Vec<u8> = self.buf.drain(..=end).collect();
                    line.pop();
                    return Ok(LineRead::Line(line));
                }
                if self.buf.len() > self.limit {
                    self.buf.clear();
                    self.skipping = true;
                    return Ok(LineRead::TooLong);
                }
            }
            if !may_read || until.is_some_and(|until| Instant::now() >= until) {
                return Ok(LineRead::Idle);
            }
            let mut chunk = [0u8; 8192];
            match source.read(&mut chunk) {
                Ok(0) => {
                    if self.final_partial && !self.skipping && !self.buf.is_empty() {
                        return Ok(LineRead::Line(std::mem::take(&mut self.buf)));
                    }
                    return Ok(LineRead::Eof);
                }
                Ok(read) => self.buf.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(LineRead::Idle);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

/// The capability secret between reading the file and sending the hello. Never printed.
struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug)]
struct CapabilityFile {
    socket: PathBuf,
    capability: String,
    secret: Secret,
    /// The tools the capability may call; `None` (a file without the field) means all.
    tools: Option<Vec<String>>,
}

fn parse_capability(bytes: &[u8]) -> Result<CapabilityFile, ClientError> {
    // Deliberately no serde error text: it could echo parts of the file.
    let malformed = || ClientError::CapabilityFile("malformed capability file".into());
    let value: Value = serde_json::from_slice(bytes).map_err(|_| malformed())?;
    let field = |name: &str| value.get(name).and_then(Value::as_str);
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(ClientError::CapabilityFile(
            "unsupported capability file version".into(),
        ));
    }
    let (Some(socket), Some(capability), Some(secret)) =
        (field("socket"), field("capability"), field("secret"))
    else {
        return Err(malformed());
    };
    if !Path::new(socket).is_absolute()
        || capability.is_empty()
        || capability.len() > 128
        || secret.len() != 64
        || !secret.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(malformed());
    }
    let tools = match value.get("tools") {
        None => None,
        Some(Value::Array(names)) => Some(
            names
                .iter()
                .map(|name| name.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(malformed)?,
        ),
        Some(_) => return Err(malformed()),
    };
    Ok(CapabilityFile {
        socket: PathBuf::from(socket),
        capability: capability.to_string(),
        secret: Secret(secret.to_string()),
        tools,
    })
}

fn hello_line(capability: &CapabilityFile) -> Result<Vec<u8>, ClientError> {
    let mut line = serde_json::to_vec(&json!({
        "hello": {"capability": capability.capability, "secret": capability.secret.0}
    }))
    .map_err(|_| ClientError::Protocol("cannot encode hello".into()))?;
    line.push(b'\n');
    Ok(line)
}

fn broker_error(value: &Value) -> Option<ToolError> {
    serde_json::from_value(value.get("error")?.clone()).ok()
}

mod imp {
    use super::*;
    use crate::agent_tools::transport::{self, Stream};

    /// One authenticated connection to the broker with a single call in flight.
    #[derive(Debug)]
    pub struct BrokerClient {
        stream: Stream,
        reader: LineReader,
        next_id: u64,
        call_timeout: Duration,
        broken: bool,
    }

    impl BrokerClient {
        /// Read the capability file (refusing symlinks, files not owned by us and files
        /// anyone else can read), connect with a timeout and perform the hello.
        pub fn connect(capability_file: &Path) -> Result<Self, ClientError> {
            let capability = read_capability(capability_file)?;
            let stream = transport::connect(&capability.socket, CONNECT_TIMEOUT)
                .map_err(|error| ClientError::Unreachable(error.to_string()))?;
            stream
                .set_write_timeout(Some(WRITE_TIMEOUT))
                .map_err(|error| ClientError::Unreachable(error.to_string()))?;
            let mut client = Self {
                stream,
                reader: LineReader::new(MAX_REPLY_LINE_BYTES),
                next_id: 1,
                call_timeout: DEFAULT_CALL_TIMEOUT,
                broken: false,
            };
            let hello = hello_line(&capability)?;
            drop(capability);
            client
                .stream
                .write_all(&hello)
                .and_then(|()| client.stream.flush())
                .map_err(|error| ClientError::Unreachable(error.to_string()))?;
            let line = client
                .read_line(Instant::now() + HELLO_TIMEOUT)
                .map_err(ClientError::Unreachable)?;
            let reply: Value = serde_json::from_slice(&line)
                .map_err(|_| ClientError::Protocol("malformed hello reply".into()))?;
            if reply.get("ok") == Some(&Value::Bool(true)) {
                Ok(client)
            } else if let Some(error) = broker_error(&reply) {
                Err(ClientError::Rejected(error))
            } else {
                Err(ClientError::Protocol("unexpected hello reply".into()))
            }
        }

        /// The tools `capability_file` may call (`None`: all of them), read with the same
        /// checks as [`Self::connect`]. The broker enforces the list; this only lets a
        /// facade leave the others out of its listing.
        pub fn allowed_tools(capability_file: &Path) -> Result<Option<Vec<String>>, ClientError> {
            Ok(read_capability(capability_file)?.tools)
        }

        /// Replace the per-call wall-clock limit (default 10 minutes).
        pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
            self.call_timeout = timeout;
            self
        }

        /// One call; transport problems are `unavailable`.
        pub fn call(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
            self.try_call(method, params)
                .map_err(CallFailure::into_tool_error)
        }

        /// One call that tells tool errors from transport failures apart.
        pub fn try_call(&mut self, method: &str, params: Value) -> Result<Value, CallFailure> {
            if self.broken {
                return Err(CallFailure::Transport(
                    "the broker connection is closed".into(),
                ));
            }
            let id = self.next_id;
            self.next_id += 1;
            let mut request =
                serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
                    .map_err(|error| CallFailure::Tool(ToolError::invalid(error.to_string())))?;
            if request.len() > MAX_REQUEST_BYTES {
                return Err(CallFailure::Tool(ToolError::new(
                    ToolErrorCode::TooLarge,
                    format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
                )));
            }
            request.push(b'\n');
            if let Err(error) = self
                .stream
                .write_all(&request)
                .and_then(|()| self.stream.flush())
            {
                self.broken = true;
                return Err(CallFailure::Transport(error.to_string()));
            }
            let line = match self.read_line(Instant::now() + self.call_timeout) {
                Ok(line) => line,
                Err(message) => {
                    self.broken = true;
                    return Err(CallFailure::Transport(message));
                }
            };
            let reply: Value = match serde_json::from_slice(&line) {
                Ok(reply) => reply,
                Err(_) => {
                    self.broken = true;
                    return Err(CallFailure::Transport("malformed broker reply".into()));
                }
            };
            match (reply.get("id"), broker_error(&reply)) {
                // The broker could not attribute the line (oversized/garbled): it closes.
                (Some(Value::Null) | None, Some(error)) => {
                    self.broken = true;
                    Err(CallFailure::Tool(error))
                }
                (Some(reply_id), outcome) if reply_id.as_u64() == Some(id) => match outcome {
                    Some(error) => {
                        if matches!(
                            error.code,
                            ToolErrorCode::Expired
                                | ToolErrorCode::StaleTask
                                | ToolErrorCode::Unauthorized
                        ) {
                            self.broken = true;
                        }
                        Err(CallFailure::Tool(error))
                    }
                    None => match reply.get("result") {
                        Some(result) => Ok(result.clone()),
                        None => {
                            self.broken = true;
                            Err(CallFailure::Transport(
                                "broker reply has neither result nor error".into(),
                            ))
                        }
                    },
                },
                _ => {
                    self.broken = true;
                    Err(CallFailure::Transport(
                        "broker reply does not match the request id".into(),
                    ))
                }
            }
        }

        fn read_line(&mut self, deadline: Instant) -> Result<Vec<u8>, String> {
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("timed out waiting for the broker".into());
                }
                self.stream
                    .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))
                    .map_err(|error| error.to_string())?;
                match self
                    .reader
                    .next(&mut self.stream, true, Some(deadline))
                    .map_err(|error| error.to_string())?
                {
                    LineRead::Line(line) => return Ok(line),
                    LineRead::Eof => return Err("the broker closed the connection".into()),
                    LineRead::TooLong => return Err("broker reply is too large".into()),
                    LineRead::Idle => {}
                }
            }
        }
    }

    fn read_capability(path: &Path) -> Result<CapabilityFile, ClientError> {
        let unusable = |reason: String| ClientError::CapabilityFile(reason);
        let file = transport::open_private_file(path).map_err(unusable)?;
        let meta = file
            .metadata()
            .map_err(|error| unusable(format!("cannot stat {}: {error}", path.display())))?;
        if meta.len() > MAX_CAPABILITY_FILE_BYTES as u64 {
            return Err(unusable(format!(
                "{} exceeds {MAX_CAPABILITY_FILE_BYTES} bytes",
                path.display()
            )));
        }
        let mut bytes = Vec::new();
        file.take(MAX_CAPABILITY_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| unusable(format!("cannot read {}: {error}", path.display())))?;
        if bytes.len() > MAX_CAPABILITY_FILE_BYTES {
            return Err(unusable(format!(
                "{} exceeds {MAX_CAPABILITY_FILE_BYTES} bytes",
                path.display()
            )));
        }
        parse_capability(&bytes)
    }
}

pub use imp::BrokerClient;

pub const CLI_USAGE: &str = "usage: studio-tools [--capability <file>] <tool> [--json '<params object>']\n       studio-tools --list\n       studio-tools --help\n\nThe capability file path may also come from FFRAMES_STUDIO_CAPABILITY.\nThe reply JSON is printed on stdout; tool errors go to stderr as {\"error\":{..}}.\nExit codes: 0 ok, 1 tool error, 2 usage error, 3 broker unreachable or capability unusable.\n";

#[derive(Debug, PartialEq)]
enum CliCommand {
    Help,
    List,
    Call {
        capability: Option<PathBuf>,
        tool: String,
        params: Value,
    },
}

fn parse_cli(args: Vec<OsString>) -> Result<CliCommand, String> {
    let mut capability: Option<PathBuf> = None;
    let mut tool: Option<String> = None;
    let mut json_params: Option<String> = None;
    let (mut list, mut help) = (false, false);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|_| "arguments must be valid UTF-8".to_string())?;
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_string(), Some(value.to_string()))
            }
            _ => (arg.clone(), None),
        };
        let mut value_of = |name: &str| -> Result<String, String> {
            match inline.clone() {
                Some(value) => Ok(value),
                None => args
                    .next()
                    .ok_or_else(|| format!("{name} needs a value"))?
                    .into_string()
                    .map_err(|_| format!("{name} value must be valid UTF-8")),
            }
        };
        match flag.as_str() {
            "--capability" => {
                if capability
                    .replace(PathBuf::from(value_of("--capability")?))
                    .is_some()
                {
                    return Err("--capability given twice".into());
                }
            }
            "--json" => {
                if json_params.replace(value_of("--json")?).is_some() {
                    return Err("--json given twice".into());
                }
            }
            "--list" if inline.is_none() => list = true,
            "--help" | "-h" if inline.is_none() => help = true,
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            _ => {
                if tool.replace(arg).is_some() {
                    return Err("more than one tool named".into());
                }
            }
        }
    }
    if help {
        return Ok(CliCommand::Help);
    }
    if list {
        if tool.is_some() || json_params.is_some() {
            return Err("--list takes no tool or --json".into());
        }
        return Ok(CliCommand::List);
    }
    let tool = tool.ok_or_else(|| "no tool named".to_string())?;
    if !TOOL_NAMES.contains(&tool.as_str()) {
        return Err(format!(
            "unknown tool `{}`; tools: {}",
            tool.chars().take(64).collect::<String>(),
            TOOL_NAMES.join(", ")
        ));
    }
    let params = match json_params {
        None => json!({}),
        Some(text) => {
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| format!("--json is not valid JSON: {e}"))?;
            if !value.is_object() {
                return Err("--json must be a JSON object".into());
            }
            value
        }
    };
    Ok(CliCommand::Call {
        capability,
        tool,
        params,
    })
}

fn write_error(err: &mut dyn Write, error: &ToolError) {
    let _ = writeln!(err, "{}", json!({"error": error}));
}

/// The whole `studio-tools` program. `args` excludes the program name. Stdout carries
/// exactly the compact reply JSON the broker returned (what MCP exposes as
/// `structuredContent`); a tool error is `{"error":{"code","message"}}` on stderr.
pub fn run_cli(
    args: Vec<OsString>,
    env_capability: Option<OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let command = match parse_cli(args) {
        Ok(command) => command,
        Err(message) => {
            let _ = write!(err, "studio-tools: {message}\n{CLI_USAGE}");
            return EXIT_USAGE;
        }
    };
    match command {
        CliCommand::Help => {
            let _ = out.write_all(CLI_USAGE.as_bytes());
            EXIT_OK
        }
        CliCommand::List => {
            let list = serde_json::to_string(&tool_descriptions()).unwrap_or_default();
            let _ = writeln!(out, "{list}");
            EXIT_OK
        }
        CliCommand::Call {
            capability,
            tool,
            params,
        } => {
            let Some(path) = capability.or_else(|| {
                env_capability
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            }) else {
                let _ = write!(
                    err,
                    "studio-tools: no capability file (use --capability or {CAPABILITY_ENV})\n{CLI_USAGE}"
                );
                return EXIT_USAGE;
            };
            let mut client = match BrokerClient::connect(&path) {
                Ok(client) => client,
                Err(error) => {
                    write_error(err, &error.to_tool_error());
                    return EXIT_UNAVAILABLE;
                }
            };
            match client.try_call(&tool, params) {
                Ok(value) => {
                    let text = serde_json::to_string(&value).unwrap_or_default();
                    if writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
                        return EXIT_UNAVAILABLE;
                    }
                    EXIT_OK
                }
                Err(CallFailure::Tool(error)) => {
                    write_error(err, &error);
                    EXIT_TOOL_ERROR
                }
                Err(failure @ CallFailure::Transport(_)) => {
                    write_error(err, &failure.into_tool_error());
                    EXIT_UNAVAILABLE
                }
            }
        }
    }
}
