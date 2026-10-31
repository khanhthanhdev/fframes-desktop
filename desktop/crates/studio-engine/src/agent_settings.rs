//! What the user adds to, or takes away from, every agent session: their own MCP servers
//! (stdio programs or streamable-HTTP endpoints), the tools of those servers and the
//! project tools they turned off. Kept in `app-settings.json`
//! ([`AppSettings::agent`](crate::app_settings::AppSettings::agent)); a project can change
//! the switches for itself ([`ProjectToolOverride`]) without touching the project's files.
//!
//! No secret is ever stored: a server names the environment variables it needs (passed to a
//! stdio server, or sent as HTTP headers), and their values are read from Studio's own
//! environment when a session starts. A server whose variable is missing is left out of
//! that session with a note, never started without it.

mod http;
mod project;
mod registry;

pub use project::{OVERRIDE_FILE, ProjectToolOverride, project_override_path};
pub use registry::{REGISTRY_URL, RegistryEntry, RegistryOption, parse_registry, search_registry};

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use studio_bootstrap::{ChildEnvironment, SpawnOptions, spawn_tracked};
use studio_harness::{McpHttpServer, McpStdioServer, McpTransport, UserMcpLaunch};

use crate::agent::model::{ToolNote, ToolNoteKind};
use crate::agent_tools::TOOL_NAMES;

/// The name of Studio's own MCP server in every session; no user server may take it.
pub const STUDIO_SERVER_NAME: &str = "fframes-studio";
/// How long [`probe`] waits for a server to start and list its tools.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest server name: it becomes part of every tool name the agent sees.
const MAX_NAME_CHARS: usize = 48;
/// The MCP revision the probe speaks.
const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentToolSettings {
    /// The user's own MCP servers, in the order they were added.
    pub mcp_servers: Vec<UserMcpServer>,
    /// Project tools (names from [`TOOL_NAMES`]) the agent is not offered.
    pub disabled_tools: Vec<String>,
}

/// One MCP server the user added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMcpServer {
    pub name: String,
    /// How the agent reaches it. Settings saved before HTTP servers existed hold the stdio
    /// fields at this level, which is still how a stdio server is written.
    #[serde(flatten)]
    pub transport: ServerTransport,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Tools of this server (names as it lists them) the agent must not use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
}

/// A stdio program, or a streamable-HTTP endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerTransport {
    Stdio {
        /// Absolute path to the program.
        command: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        /// Names of the environment variables the server gets from Studio's environment.
        #[serde(default)]
        env: Vec<String>,
    },
    Http {
        url: String,
        /// The variable whose value is sent as `Authorization: Bearer <value>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_env: Option<String>,
        /// Other headers: header name → the variable whose value it carries.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        header_env: BTreeMap<String, String>,
    },
}

fn enabled_by_default() -> bool {
    true
}

impl AgentToolSettings {
    /// The project tools the agent is offered, in [`TOOL_NAMES`] order.
    pub fn enabled_tools(&self) -> Vec<&'static str> {
        TOOL_NAMES
            .iter()
            .copied()
            .filter(|tool| !self.disabled_tools.iter().any(|off| off == tool))
            .collect()
    }

    /// Turns one project tool on or off.
    pub fn set_tool(&mut self, tool: &str, enabled: bool) {
        self.disabled_tools.retain(|off| off != tool);
        if !enabled && TOOL_NAMES.contains(&tool) {
            self.disabled_tools.push(tool.to_owned());
        }
    }

    /// Turns one tool of the user's server `server` on or off.
    pub fn set_server_tool(&mut self, server: &str, tool: &str, enabled: bool) {
        if let Some(server) = self.mcp_servers.iter_mut().find(|s| s.name == server) {
            server.set_tool(tool, enabled);
        }
    }

    /// The enabled servers ready to launch, with their variables read through `lookup`,
    /// and one note for each enabled server left out.
    pub fn launch_servers(
        &self,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> (Vec<UserMcpLaunch>, Vec<ToolNote>) {
        let mut servers = Vec::new();
        let mut notes = Vec::new();
        for server in self.mcp_servers.iter().filter(|server| server.enabled) {
            match server.launch(&lookup) {
                Ok(launch) => servers.push(launch),
                Err(reason) => notes.push(ToolNote::new(
                    ToolNoteKind::ServerLeftOut {
                        name: server.name.clone(),
                    },
                    format!("The MCP server `{}` was left out: {reason}", server.name),
                )),
            }
        }
        (servers, notes)
    }

    /// Adds the servers the user picked from another tool's settings. A server whose name
    /// is taken is skipped or replaces the one there, as chosen; one that cannot be saved
    /// is refused with the reason. Only names of variables come along, never values.
    pub fn import(
        &mut self,
        picked: impl IntoIterator<Item = (UserMcpServer, OnDuplicate)>,
    ) -> ImportReport {
        let mut report = ImportReport::default();
        for (server, on_duplicate) in picked {
            let name = server.name.clone();
            let existing = self
                .mcp_servers
                .iter()
                .position(|other| other.name.eq_ignore_ascii_case(&name));
            match (existing, on_duplicate) {
                (Some(_), OnDuplicate::Skip) => report.skipped.push(name),
                (Some(index), OnDuplicate::Overwrite) => {
                    let others: Vec<UserMcpServer> = self
                        .mcp_servers
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != index)
                        .map(|(_, other)| other.clone())
                        .collect();
                    match server.problem(&others) {
                        Some(problem) => report.refused.push((name, problem)),
                        None => {
                            self.mcp_servers[index] = server;
                            report.replaced.push(name);
                        }
                    }
                }
                (None, _) => match server.problem(&self.mcp_servers) {
                    Some(problem) => report.refused.push((name, problem)),
                    None => {
                        self.mcp_servers.push(server);
                        report.added.push(name);
                    }
                },
            }
        }
        report
    }
}

/// What an import does with a server whose name is already taken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnDuplicate {
    #[default]
    Skip,
    Overwrite,
}

/// What [`AgentToolSettings::import`] did, by server name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub skipped: Vec<String>,
    /// Name and why it could not be added.
    pub refused: Vec<(String, String)>,
}

impl UserMcpServer {
    /// A stdio server.
    pub fn stdio(
        name: impl Into<String>,
        command: impl Into<PathBuf>,
        args: Vec<String>,
        env: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: ServerTransport::Stdio {
                command: command.into(),
                args,
                env,
            },
            enabled: true,
            disabled_tools: Vec::new(),
        }
    }

    /// A streamable-HTTP server.
    pub fn http(
        name: impl Into<String>,
        url: impl Into<String>,
        bearer_env: Option<String>,
        header_env: BTreeMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: ServerTransport::Http {
                url: url.into(),
                bearer_env,
                header_env,
            },
            enabled: true,
            disabled_tools: Vec::new(),
        }
    }

    pub fn is_http(&self) -> bool {
        matches!(self.transport, ServerTransport::Http { .. })
    }

    /// The program and its arguments, or the URL: what the server is, on one line.
    pub fn summary(&self) -> String {
        match &self.transport {
            ServerTransport::Stdio { command, args, .. } => {
                std::iter::once(command.display().to_string())
                    .chain(args.iter().map(|arg| {
                        if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                            format!("\"{arg}\"")
                        } else {
                            arg.clone()
                        }
                    }))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            ServerTransport::Http { url, .. } => url.clone(),
        }
    }

    /// The names of every environment variable the server reads, in order.
    pub fn variables(&self) -> Vec<&str> {
        match &self.transport {
            ServerTransport::Stdio { env, .. } => env.iter().map(String::as_str).collect(),
            ServerTransport::Http {
                bearer_env,
                header_env,
                ..
            } => bearer_env
                .iter()
                .map(String::as_str)
                .chain(header_env.values().map(String::as_str))
                .collect(),
        }
    }

    /// The variables `lookup` has no (or an empty) value for: while any is missing, the
    /// server is left out of agent tasks.
    pub fn missing_variables(&self, lookup: impl Fn(&str) -> Option<String>) -> Vec<String> {
        self.variables()
            .into_iter()
            .filter(|name| lookup(name).is_none_or(|value| value.is_empty()))
            .map(str::to_owned)
            .collect()
    }

    /// Turns one of this server's tools on or off.
    pub fn set_tool(&mut self, tool: &str, enabled: bool) {
        self.disabled_tools.retain(|off| off != tool);
        if !enabled {
            self.disabled_tools.push(tool.to_owned());
        }
    }

    /// Why `self` cannot be saved beside `others` (the other servers, without the one it
    /// replaces), or `None`.
    pub fn problem(&self, others: &[UserMcpServer]) -> Option<String> {
        if self.name.is_empty() {
            return Some("Give the server a name.".into());
        }
        if self.name.chars().count() > MAX_NAME_CHARS
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Some(format!(
                "Use up to {MAX_NAME_CHARS} letters, digits, dashes and underscores for the name."
            ));
        }
        if self.name.eq_ignore_ascii_case(STUDIO_SERVER_NAME) {
            return Some(format!("`{STUDIO_SERVER_NAME}` is Studio's own server."));
        }
        if others
            .iter()
            .any(|other| other.name.eq_ignore_ascii_case(&self.name))
        {
            return Some(format!("A server named `{}` already exists.", self.name));
        }
        match &self.transport {
            ServerTransport::Stdio { command, .. } => {
                if !command.is_absolute() {
                    return Some("Choose the program by its full path.".into());
                }
                if !command.is_file() {
                    return Some(format!("No program at {}.", command.display()));
                }
            }
            ServerTransport::Http {
                url, header_env, ..
            } => {
                let lower = url.to_ascii_lowercase();
                if !(lower.starts_with("https://") || lower.starts_with("http://"))
                    || url.chars().any(|c| c.is_whitespace() || c.is_control())
                    || url.len() <= "http://".len()
                {
                    return Some("Enter a URL that starts with https:// or http://.".into());
                }
                if let Some(bad) = header_env.keys().find(|header| !is_header_name(header)) {
                    return Some(format!("`{bad}` is not a header name."));
                }
            }
        }
        if let Some(bad) = self.variables().into_iter().find(|name| !is_env_name(name)) {
            return Some(format!(
                "`{bad}` is not an environment variable name. List names only; values are read from Studio's environment."
            ));
        }
        None
    }

    /// The server as a session launches it, its variables read through `lookup`.
    pub fn launch(&self, lookup: impl Fn(&str) -> Option<String>) -> Result<UserMcpLaunch, String> {
        if let Some(problem) = self.problem(&[]) {
            return Err(problem);
        }
        let value = |name: &str| match lookup(name) {
            Some(value) if !value.is_empty() => Ok(value),
            _ => Err(format!("{name} is not set in Studio's environment.")),
        };
        let transport = match &self.transport {
            ServerTransport::Stdio { command, args, env } => {
                let env = env
                    .iter()
                    .map(|name| Ok((name.clone(), value(name)?)))
                    .collect::<Result<Vec<_>, String>>()?;
                McpTransport::Stdio(
                    McpStdioServer::new(&self.name, command, args.clone(), env)
                        .map_err(|error| error.to_string())?,
                )
            }
            ServerTransport::Http {
                url,
                bearer_env,
                header_env,
            } => {
                let mut headers = Vec::new();
                if let Some(name) = bearer_env {
                    headers.push((
                        "Authorization".to_owned(),
                        format!("Bearer {}", value(name)?),
                    ));
                }
                for (header, name) in header_env {
                    headers.push((header.clone(), value(name)?));
                }
                McpTransport::Http(
                    McpHttpServer::new(&self.name, url, headers)
                        .map_err(|error| error.to_string())?,
                )
            }
        };
        Ok(UserMcpLaunch {
            transport,
            disabled_tools: self.disabled_tools.clone(),
        })
    }
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `input` as an absolute program path: itself when it names a file, otherwise the first
/// match on `PATH` (with the `PATHEXT` extensions on Windows, so `npx` finds `npx.cmd`).
pub fn resolve_command(input: &str) -> Option<PathBuf> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let given = Path::new(input);
    if given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    if given.components().count() > 1 {
        return None;
    }
    let extensions: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new())
            .chain(
                pathext
                    .split(';')
                    .filter(|e| !e.is_empty())
                    .map(str::to_owned),
            )
            .collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        extensions.iter().find_map(|ext| {
            let candidate = dir.join(format!("{input}{ext}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

/// A server found in another tool's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundServer {
    /// "Claude Code", "Claude Desktop" or "Codex".
    pub source: &'static str,
    /// Ready to add: the command resolved to a full path when it could be.
    pub server: UserMcpServer,
    /// The other tool stores values (environment values, or header values that name no
    /// variable) that Studio does not copy; Studio only passes values on from its own
    /// environment.
    pub had_values: bool,
}

/// Servers in the user's Claude Code (`~/.claude.json`), Claude Desktop and Codex
/// (`~/.codex/config.toml`) settings: stdio programs, and streamable-HTTP URLs (SSE servers
/// are skipped). Header values that are a variable reference (`${NAME}`, `Bearer ${NAME}`,
/// Codex's `bearer_token_env_var` and `env_http_headers`) come along as variable names;
/// any other value stays behind. Unreadable files are skipped silently.
pub fn find_servers(home: &Path, app_data: Option<&Path>) -> Vec<FoundServer> {
    let mut found = Vec::new();
    let json = |path: PathBuf| -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    if let Some(config) = json(home.join(".claude.json")) {
        collect_json(&config["mcpServers"], "Claude Code", &mut found);
    }
    let desktop = match app_data {
        Some(app_data) => app_data.join("Claude").join("claude_desktop_config.json"),
        None => home
            .join("Library/Application Support/Claude")
            .join("claude_desktop_config.json"),
    };
    if let Some(config) = json(desktop) {
        collect_json(&config["mcpServers"], "Claude Desktop", &mut found);
    }
    if let Some(config) = std::fs::read_to_string(home.join(".codex").join("config.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        && let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table)
    {
        for (name, entry) in servers {
            let strings = |key: &str| -> Vec<String> {
                entry
                    .get(key)
                    .and_then(toml::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let table = |key: &str| entry.get(key).and_then(toml::Value::as_table);
            if let Some(command) = entry.get("command").and_then(toml::Value::as_str) {
                let env: Vec<String> = table("env")
                    .map(|env| env.keys().cloned().collect())
                    .unwrap_or_default();
                let had_values = !env.is_empty();
                found.push(FoundServer {
                    source: "Codex",
                    server: found_stdio(name, command, strings("args"), env),
                    had_values,
                });
            } else if let Some(url) = entry.get("url").and_then(toml::Value::as_str) {
                let bearer_env = entry
                    .get("bearer_token_env_var")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned);
                let header_env: BTreeMap<String, String> = table("env_http_headers")
                    .map(|headers| {
                        headers
                            .iter()
                            .filter_map(|(header, var)| {
                                Some((header.clone(), var.as_str()?.to_owned()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let had_values = table("http_headers").is_some_and(|h| !h.is_empty())
                    || entry.get("bearer_token").is_some();
                found.push(FoundServer {
                    source: "Codex",
                    server: UserMcpServer::http(name, url, bearer_env, header_env),
                    had_values,
                });
            }
        }
    }
    found
}

fn found_stdio(name: &str, command: &str, args: Vec<String>, env: Vec<String>) -> UserMcpServer {
    UserMcpServer::stdio(
        name,
        resolve_command(command).unwrap_or_else(|| PathBuf::from(command)),
        args,
        env,
    )
}

/// `NAME` from a header value `${NAME}` or `Bearer ${NAME}`, with which of the two it was.
fn variable_reference(value: &str) -> Option<(bool, &str)> {
    let (bearer, rest) = match value.strip_prefix("Bearer ") {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let name = rest.strip_prefix("${")?.strip_suffix('}')?;
    is_env_name(name).then_some((bearer, name))
}

fn collect_json(servers: &Value, source: &'static str, found: &mut Vec<FoundServer>) {
    let Some(servers) = servers.as_object() else {
        return;
    };
    for (name, entry) in servers {
        if let Some(command) = entry["command"].as_str() {
            let args = entry["args"]
                .as_array()
                .map(|args| {
                    args.iter()
                        .filter_map(|arg| arg.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let env: Vec<String> = entry["env"]
                .as_object()
                .map(|env| env.keys().cloned().collect())
                .unwrap_or_default();
            let had_values = !env.is_empty();
            found.push(FoundServer {
                source,
                server: found_stdio(name, command, args, env),
                had_values,
            });
            continue;
        }
        let Some(url) = entry["url"].as_str() else {
            continue;
        };
        if !matches!(
            entry["type"].as_str(),
            None | Some("http" | "streamable-http")
        ) {
            continue;
        }
        let mut bearer_env = None;
        let mut header_env = BTreeMap::new();
        let mut had_values = false;
        for (header, value) in entry["headers"].as_object().into_iter().flatten() {
            match value.as_str().and_then(variable_reference) {
                Some((true, name)) if header.eq_ignore_ascii_case("authorization") => {
                    bearer_env = Some(name.to_owned());
                }
                Some((false, name)) => {
                    header_env.insert(header.clone(), name.to_owned());
                }
                _ => had_values = true,
            }
        }
        found.push(FoundServer {
            source,
            server: UserMcpServer::http(name, url, bearer_env, header_env),
            had_values,
        });
    }
}

/// Why a [`probe`] failed, for the suggested fix beside the raw message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailureKind {
    /// No answer in time.
    TimedOut,
    /// The program could not be started (missing, or not a program).
    NotStarted,
    /// The program stopped before it answered.
    Exited,
    /// It answered with something that is not MCP JSON.
    NotMcp,
    /// It answered with an error, or (HTTP) refused the request.
    Refused,
    /// (HTTP) The URL could not be reached.
    Unreachable,
}

/// A failed [`probe`]: what kind of failure, and the raw message to show as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFailure {
    pub kind: ProbeFailureKind,
    pub detail: String,
}

impl ProbeFailure {
    pub fn new(kind: ProbeFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// A short suggested fix for a stdio server, or an HTTP one when `http`.
    pub fn hint(&self, http: bool) -> &'static str {
        match (self.kind, http) {
            (ProbeFailureKind::TimedOut, false) => {
                "The first run of npx or uvx may still be downloading the package. Try Test \
                 again, or run the command in a terminal to see what it prints."
            }
            (ProbeFailureKind::TimedOut, true) => {
                "The server did not answer in time. Check the URL, then try Test again."
            }
            (ProbeFailureKind::NotStarted, _) => {
                "The program was not found. Install it, or choose it again with Choose…."
            }
            (ProbeFailureKind::Exited, _) => {
                "The server stopped. Its last message is below; it often names a missing \
                 variable or argument."
            }
            (ProbeFailureKind::NotMcp, false) => {
                "It printed something that is not MCP. Check the arguments: some servers need \
                 a flag such as --stdio."
            }
            (ProbeFailureKind::NotMcp, true) => {
                "It did not answer as an MCP server. Check the URL: it is often the address \
                 that ends in /mcp."
            }
            (ProbeFailureKind::Refused, false) => {
                "The server answered with an error. Check its arguments and variables."
            }
            (ProbeFailureKind::Refused, true) => {
                "The server refused the request. Check the token variable and its value."
            }
            (ProbeFailureKind::Unreachable, _) => {
                "Studio could not reach the URL. Check it and your connection."
            }
        }
    }
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Starts or calls `server` the way an agent would, asks for its tools and stops: the
/// names of the tools it lists, or why it could not. Blocking (up to `timeout`): run it off
/// the UI thread. A stdio server runs in its own process group, so helpers it starts
/// (`npx`) stop with it; an HTTP server gets `initialize`, `notifications/initialized` and
/// `tools/list` POSTs, its headers passed to `curl` on its input, never its command line.
pub fn probe(server: &McpTransport, timeout: Duration) -> Result<Vec<String>, ProbeFailure> {
    match server {
        McpTransport::Stdio(server) => probe_stdio(server, timeout),
        McpTransport::Http(server) => http::probe(server, timeout),
    }
}

fn initialize_request() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "fframes-studio", "version": env!("CARGO_PKG_VERSION")},
    }})
}

fn tool_names(listed: &Value) -> Vec<String> {
    listed["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn probe_stdio(server: &McpStdioServer, timeout: Duration) -> Result<Vec<String>, ProbeFailure> {
    use ProbeFailureKind as Kind;
    let mut env = ChildEnvironment::default_allowlist();
    for (name, value) in &server.env {
        env.set(name.clone(), value.clone());
    }
    let mut options = SpawnOptions::new(&server.command);
    options
        .args(&server.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    options.env = env;
    let mut child = spawn_tracked(options).map_err(|error| {
        ProbeFailure::new(Kind::NotStarted, format!("It did not start: {error}"))
    })?;
    let process = child.child_mut();
    let stopped = || ProbeFailure::new(Kind::Exited, "It stopped before it answered.");
    let mut stdin = process.stdin.take().ok_or_else(stopped)?;
    let stdout = process.stdout.take().ok_or_else(stopped)?;
    let stderr = process.stderr.take();
    let (lines, received) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let (errors, error_text) = mpsc::channel::<String>();
    if let Some(mut stderr) = stderr {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = (&mut stderr).take(16 * 1024).read_to_end(&mut bytes);
            let _ = errors.send(String::from_utf8_lossy(&bytes).into_owned());
        });
    }
    let deadline = Instant::now() + timeout;
    // The first line on stdout that was not JSON: a server that logs to stdout, or a
    // program that is not an MCP server.
    let mut noise: Option<String> = None;
    let mut send = |message: Value| -> Result<(), ProbeFailure> {
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|_| stopped())
    };
    let mut wait_for = |id: u64| -> Result<Value, ProbeFailure> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = received.recv_timeout(left).map_err(|error| {
                let reason = match error {
                    mpsc::RecvTimeoutError::Timeout => ProbeFailure::new(
                        Kind::TimedOut,
                        format!("It did not answer within {} seconds.", timeout.as_secs()),
                    ),
                    mpsc::RecvTimeoutError::Disconnected => stopped(),
                };
                match &noise {
                    Some(line) => ProbeFailure::new(
                        Kind::NotMcp,
                        format!("{} It printed: {line}", reason.detail),
                    ),
                    None => reason,
                }
            })?;
            let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                if noise.is_none() && !line.trim().is_empty() {
                    noise = Some(line.trim().chars().take(240).collect());
                }
                continue;
            };
            if reply["id"] != json!(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(ProbeFailure::new(
                    Kind::Refused,
                    format!(
                        "It refused: {}",
                        error["message"].as_str().unwrap_or("no reason given")
                    ),
                ));
            }
            return Ok(reply["result"].clone());
        }
    };
    let outcome = (|| {
        send(initialize_request())?;
        wait_for(1)?;
        send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}))?;
        Ok(tool_names(&wait_for(2)?))
    })();
    drop(stdin);
    let _ = child.kill_forcefully();
    outcome.map_err(|failure: ProbeFailure| {
        let detail = error_text
            .recv_timeout(Duration::from_millis(500))
            .ok()
            .map(|text| last_line(&text))
            .filter(|line| !line.is_empty());
        match detail {
            Some(detail) => ProbeFailure::new(failure.kind, format!("{} {detail}", failure.detail)),
            None => failure,
        }
    })
}

/// The last non-empty line of a server's error output, bounded for a one-line notice.
fn last_line(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    line.trim().chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> UserMcpServer {
        UserMcpServer::stdio(
            name,
            std::env::current_exe().unwrap(),
            vec!["--serve".into()],
            vec!["GITHUB_TOKEN".into()],
        )
    }

    fn stdio_env(launch: &UserMcpLaunch) -> &[(String, String)] {
        match &launch.transport {
            McpTransport::Stdio(server) => &server.env,
            McpTransport::Http(_) => panic!("not stdio"),
        }
    }

    #[test]
    fn a_server_reads_its_variables_at_launch_and_is_left_out_without_them() {
        let settings = AgentToolSettings {
            mcp_servers: vec![
                server("github"),
                UserMcpServer {
                    enabled: false,
                    ..server("off")
                },
            ],
            disabled_tools: vec![],
        };
        let (servers, notes) = settings
            .launch_servers(|name| (name == "GITHUB_TOKEN").then(|| "ghp-secret".to_owned()));
        assert_eq!(servers.len(), 1, "the switched-off server never launches");
        assert_eq!(
            stdio_env(&servers[0]),
            &[("GITHUB_TOKEN".to_owned(), "ghp-secret".to_owned())]
        );
        assert!(notes.is_empty());

        let (servers, notes) = settings.launch_servers(|_| None);
        assert!(servers.is_empty());
        assert_eq!(
            notes,
            vec![ToolNote::new(
                ToolNoteKind::ServerLeftOut {
                    name: "github".into()
                },
                "The MCP server `github` was left out: GITHUB_TOKEN is not set in Studio's environment."
            )]
        );
        assert_eq!(
            server("github").missing_variables(|_| Some(String::new())),
            vec!["GITHUB_TOKEN".to_owned()],
            "an empty value is missing too"
        );
    }

    #[test]
    fn an_http_server_sends_its_variables_as_headers_read_only_at_launch() {
        let mut docs = UserMcpServer::http(
            "docs",
            "https://docs.test/mcp",
            Some("DOCS_TOKEN".into()),
            BTreeMap::from([("X-Team".to_owned(), "DOCS_TEAM".to_owned())]),
        );
        docs.set_tool("delete_page", false);
        assert_eq!(docs.problem(&[]), None);
        assert_eq!(docs.variables(), vec!["DOCS_TOKEN", "DOCS_TEAM"]);
        let saved = serde_json::to_string(&docs).unwrap();
        assert!(!saved.contains("tok-1"), "no value is ever stored");
        assert_eq!(serde_json::from_str::<UserMcpServer>(&saved).unwrap(), docs);

        let values = |name: &str| match name {
            "DOCS_TOKEN" => Some("tok-1".to_owned()),
            "DOCS_TEAM" => Some("studio".to_owned()),
            _ => None,
        };
        let launch = docs.launch(values).unwrap();
        let McpTransport::Http(http) = &launch.transport else {
            panic!("not http");
        };
        assert_eq!(http.url, "https://docs.test/mcp");
        assert_eq!(
            http.headers,
            vec![
                ("Authorization".to_owned(), "Bearer tok-1".to_owned()),
                ("X-Team".to_owned(), "studio".to_owned()),
            ]
        );
        assert_eq!(launch.disabled_tools, vec!["delete_page".to_owned()]);
        let secrets: Vec<&str> = launch.secrets().collect();
        assert!(secrets.contains(&"tok-1") && secrets.contains(&"Bearer tok-1"));
        assert!(!format!("{launch:?}").contains("tok-1"));

        assert_eq!(
            docs.missing_variables(|name| (name == "DOCS_TEAM").then(|| "x".into())),
            vec!["DOCS_TOKEN".to_owned()]
        );
        assert!(
            docs.launch(|_| None)
                .unwrap_err()
                .contains("DOCS_TOKEN is not set")
        );
        let bad = UserMcpServer::http("docs", "ftp://docs.test", None, BTreeMap::new());
        assert!(bad.problem(&[]).unwrap().contains("https://"));
        let bad_header = UserMcpServer::http(
            "docs",
            "https://docs.test",
            None,
            BTreeMap::from([("X Team".to_owned(), "TEAM".to_owned())]),
        );
        assert!(bad_header.problem(&[]).unwrap().contains("header"));
    }

    #[test]
    fn settings_saved_before_http_servers_still_read() {
        let old = r#"{"mcp_servers":[{"name":"github","command":"/usr/bin/gh","args":["-y"],"env":["GITHUB_TOKEN"],"enabled":false}],"disabled_tools":["timeline"]}"#;
        let settings: AgentToolSettings = serde_json::from_str(old).unwrap();
        let github = &settings.mcp_servers[0];
        assert_eq!(
            github.transport,
            ServerTransport::Stdio {
                command: "/usr/bin/gh".into(),
                args: vec!["-y".into()],
                env: vec!["GITHUB_TOKEN".into()],
            }
        );
        assert!(!github.enabled);
        assert!(github.disabled_tools.is_empty());
        let written = serde_json::to_value(&settings).unwrap();
        assert_eq!(written["mcp_servers"][0]["command"], "/usr/bin/gh");
        assert!(written["mcp_servers"][0].get("disabled_tools").is_none());
    }

    #[test]
    fn names_values_and_studio_s_own_server_are_refused() {
        assert_eq!(server("github").problem(&[]), None);
        assert!(server("").problem(&[]).is_some());
        assert!(server("has space").problem(&[]).is_some());
        assert!(
            server("a.b").problem(&[]).is_some(),
            "dots break Codex config keys"
        );
        assert!(
            server("FFrames-Studio")
                .problem(&[])
                .unwrap()
                .contains("Studio's own")
        );
        assert!(
            server("GitHub")
                .problem(&[server("github")])
                .unwrap()
                .contains("already")
        );
        let value = UserMcpServer::stdio(
            "github",
            std::env::current_exe().unwrap(),
            vec![],
            vec!["GITHUB_TOKEN=ghp-123".into()],
        );
        assert!(value.problem(&[]).unwrap().contains("names only"));
        let relative = UserMcpServer::stdio("github", "npx", vec![], vec![]);
        assert!(relative.problem(&[]).unwrap().contains("full path"));
    }

    #[test]
    fn tools_turn_off_by_name_and_unknown_names_are_ignored() {
        let mut settings = AgentToolSettings::default();
        assert_eq!(settings.enabled_tools(), TOOL_NAMES.to_vec());
        settings.set_tool("render_strip", false);
        settings.set_tool("not_a_tool", false);
        settings.set_tool("render_strip", false);
        assert_eq!(settings.disabled_tools, vec!["render_strip".to_owned()]);
        assert!(!settings.enabled_tools().contains(&"render_strip"));
        assert_eq!(settings.enabled_tools().len(), TOOL_NAMES.len() - 1);
        settings.set_tool("render_strip", true);
        assert!(settings.disabled_tools.is_empty());

        settings.mcp_servers.push(server("github"));
        settings.set_server_tool("github", "push_files", false);
        settings.set_server_tool("github", "push_files", false);
        assert_eq!(
            settings.mcp_servers[0].disabled_tools,
            vec!["push_files".to_owned()]
        );
        settings.set_server_tool("github", "push_files", true);
        assert!(settings.mcp_servers[0].disabled_tools.is_empty());
    }

    #[test]
    fn an_import_skips_or_overwrites_a_taken_name_as_chosen() {
        let mut settings = AgentToolSettings {
            mcp_servers: vec![
                UserMcpServer {
                    enabled: false,
                    ..server("github")
                },
                server("files"),
            ],
            disabled_tools: vec![],
        };
        let replacement = UserMcpServer::stdio(
            "GitHub",
            std::env::current_exe().unwrap(),
            vec!["--new".into()],
            vec![],
        );
        let report = settings.import([
            (server("files"), OnDuplicate::Skip),
            (replacement.clone(), OnDuplicate::Overwrite),
            (server("docs"), OnDuplicate::Skip),
            (
                UserMcpServer::stdio("missing", "/nowhere/server", vec![], vec![]),
                OnDuplicate::Skip,
            ),
        ]);
        assert_eq!(report.skipped, vec!["files".to_owned()]);
        assert_eq!(report.replaced, vec!["GitHub".to_owned()]);
        assert_eq!(report.added, vec!["docs".to_owned()]);
        assert_eq!(report.refused.len(), 1);
        assert_eq!(report.refused[0].0, "missing");
        let names: Vec<&str> = settings
            .mcp_servers
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["GitHub", "files", "docs"], "replaced in place");
        assert_eq!(settings.mcp_servers[0], replacement);
    }

    #[test]
    fn the_probe_lists_a_server_s_tools_and_says_why_one_fails() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(
            &script,
            r#"
import json, os, sys
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    if message["method"] == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}}
    else:
        result = {"tools": [{"name": "alpha"}, {"name": os.environ["SECOND_TOOL"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
"#,
        )
        .unwrap();
        let python = studio_test_support::python();
        let stdio = |args: Vec<String>, env: Vec<(String, String)>| {
            McpTransport::Stdio(McpStdioServer::new("fake", python, args, env).unwrap())
        };
        let server = stdio(
            vec![script.to_string_lossy().into_owned()],
            vec![("SECOND_TOOL".into(), "beta".into())],
        );
        assert_eq!(
            probe(&server, PROBE_TIMEOUT).unwrap(),
            vec!["alpha", "beta"]
        );

        let failing = stdio(
            vec!["-c".into(), "import sys; sys.exit('token rejected')".into()],
            vec![],
        );
        let error = probe(&failing, PROBE_TIMEOUT).unwrap_err();
        assert_eq!(error.kind, ProbeFailureKind::Exited);
        assert!(
            error.detail.starts_with("It stopped before it answered."),
            "{error}"
        );
        assert!(error.detail.ends_with("token rejected"), "{error}");

        let chatty = stdio(
            vec!["-c".into(), "print('Usage: server --stdio')".into()],
            vec![],
        );
        let error = probe(&chatty, PROBE_TIMEOUT).unwrap_err();
        assert_eq!(error.kind, ProbeFailureKind::NotMcp, "{error}");
        assert!(error.detail.contains("Usage: server --stdio"), "{error}");
        assert!(error.hint(false).contains("--stdio"));

        let silent = stdio(
            vec!["-c".into(), "import time; time.sleep(30)".into()],
            vec![],
        );
        let error = probe(&silent, Duration::from_secs(1)).unwrap_err();
        assert_eq!(error.kind, ProbeFailureKind::TimedOut);
        assert!(error.hint(false).contains("npx or uvx"));
        assert!(error.hint(false).contains("Try Test again"));

        let missing = McpTransport::Stdio(
            McpStdioServer::new(
                "gone",
                dir.path().join("no-such-program.exe"),
                vec![],
                vec![],
            )
            .unwrap(),
        );
        let error = probe(&missing, PROBE_TIMEOUT).unwrap_err();
        assert_eq!(error.kind, ProbeFailureKind::NotStarted, "{error}");
        assert!(error.hint(false).contains("not found"));
    }

    #[test]
    fn servers_are_found_in_claude_and_codex_settings_with_names_but_no_values() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".claude.json"),
            json!({"mcpServers": {
                "github": {"command": "definitely-not-on-path-xyz", "args": ["-y", "gh"], "env": {"GITHUB_TOKEN": "ghp-secret"}},
                "remote": {"type": "http", "url": "https://example.com/mcp", "headers": {"Authorization": "Bearer ${REMOTE_TOKEN}", "X-Key": "literal-secret"}},
                "stream": {"type": "sse", "url": "https://example.com/sse"},
            }})
            .to_string(),
        )
        .unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        std::fs::write(
            home.path().join(".codex").join("config.toml"),
            "model = \"x\"\n[mcp_servers.files]\ncommand = \"/usr/bin/files\"\nargs = [\"--root\", \".\"]\n\
             [mcp_servers.linear]\nurl = \"https://mcp.linear.app/mcp\"\nbearer_token_env_var = \"LINEAR_KEY\"\n\
             env_http_headers = { \"X-Org\" = \"LINEAR_ORG\" }\n",
        )
        .unwrap();
        let found = find_servers(home.path(), Some(home.path()));
        assert_eq!(found.len(), 4, "{found:#?}");
        let github = &found[0];
        assert_eq!(github.source, "Claude Code");
        assert_eq!(github.server.variables(), vec!["GITHUB_TOKEN"]);
        assert!(github.had_values);
        let remote = &found[1];
        assert_eq!(
            remote.server,
            UserMcpServer::http(
                "remote",
                "https://example.com/mcp",
                Some("REMOTE_TOKEN".into()),
                BTreeMap::new()
            )
        );
        assert!(remote.had_values, "the literal header stays behind");
        assert!(!format!("{found:?}").contains("secret"));
        assert_eq!(found[2].source, "Codex");
        assert_eq!(
            found[2].server.transport,
            ServerTransport::Stdio {
                command: "/usr/bin/files".into(),
                args: vec!["--root".into(), ".".into()],
                env: vec![],
            }
        );
        assert!(!found[2].had_values);
        assert_eq!(
            found[3].server,
            UserMcpServer::http(
                "linear",
                "https://mcp.linear.app/mcp",
                Some("LINEAR_KEY".into()),
                BTreeMap::from([("X-Org".to_owned(), "LINEAR_ORG".to_owned())])
            )
        );
        assert!(!found[3].had_values);
    }
}
