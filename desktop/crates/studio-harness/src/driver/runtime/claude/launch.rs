//! Claude Code launch: flags, the private MCP and settings files, and the version gate.

use super::super::{
    McpStdioServer, McpTransport, UserMcpLaunch,
    native_launch::{self, UserArgs, Version},
};
use std::path::Path;

pub(crate) use native_launch::parse_version;

const PRODUCT: &str = "Claude Code";
/// The oldest Claude Code release the stream-json wire was validated against: every
/// later flag and frame shape the driver relies on was observed on it.
pub(crate) const MIN_VERSION: Version = (2, 1, 296);

/// User arguments a Claude adapter may carry, each with one value. Everything that
/// changes permissions, settings, MCP servers, the session or the wire format is the
/// driver's own and refused.
const USER_FLAGS: [&str; 3] = ["--model", "--effort", "--fallback-model"];

/// Splits the resolved launch arguments into the program prefix and the allowed flags.
pub(crate) fn split_user_args(args: &[String]) -> Result<UserArgs, String> {
    native_launch::split_user_args(PRODUCT, &USER_FLAGS, args)
}

/// How the session starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionStart {
    /// A new session with this id (`--session-id`).
    New(String),
    /// Continue an earlier session (`--resume=ID`).
    Resume(String),
}

impl SessionStart {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::New(id) | Self::Resume(id) => id,
        }
    }
}

/// The full argument list after the program prefix.
pub(crate) fn wire_args(
    user: &UserArgs,
    session: &SessionStart,
    mcp_config: &Path,
    settings: &Path,
) -> Vec<String> {
    let mut args = user.prefix.clone();
    args.extend(
        [
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
            // Every gated tool call becomes a `can_use_tool` request the user answers.
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "default",
            // The user's hooks, skills, agents and MCP servers do not load: only the
            // project's own settings and the studio's MCP server.
            "--setting-sources",
            "project,local",
            "--strict-mcp-config",
        ]
        .map(str::to_owned),
    );
    args.push("--mcp-config".into());
    args.push(mcp_config.to_string_lossy().into_owned());
    args.push("--settings".into());
    args.push(settings.to_string_lossy().into_owned());
    match session {
        SessionStart::New(id) => {
            args.push("--session-id".into());
            args.push(id.clone());
        }
        SessionStart::Resume(id) => args.push(format!("--resume={id}")),
    }
    args.extend(user.flags.iter().cloned());
    args
}

fn pairs(pairs: &[(String, String)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.clone(), serde_json::Value::from(value.clone())))
        .collect()
}

fn stdio_entry(server: &McpStdioServer) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": server.command,
        "args": server.args,
        "env": pairs(&server.env),
    })
}

/// `{"mcpServers": {...}}` for `--mcp-config`: the studio's servers and the user's (stdio,
/// or `{"type":"http","url","headers"}`). Holds environment and header values, so it is
/// only ever written to the driver's private directory and never passed on the command
/// line.
pub(crate) fn mcp_config_json(
    studio: &[McpStdioServer],
    user: &[UserMcpLaunch],
) -> serde_json::Value {
    let mut servers: serde_json::Map<String, serde_json::Value> = studio
        .iter()
        .map(|server| (server.name.clone(), stdio_entry(server)))
        .collect();
    for server in user {
        let entry = match &server.transport {
            McpTransport::Stdio(stdio) => stdio_entry(stdio),
            McpTransport::Http(http) => serde_json::json!({
                "type": "http",
                "url": http.url,
                "headers": pairs(&http.headers),
            }),
        };
        servers.insert(server.name().to_owned(), entry);
    }
    serde_json::json!({ "mcpServers": servers })
}

/// Flag settings: the studio's own MCP tools are allowed without asking (they are the
/// tools the studio offers the task); the user's servers still ask, and the tools the user
/// turned off are denied (`mcp__<server>__<tool>`). Draft folders are untrusted, so a
/// project's shared allow rules never apply there.
pub(crate) fn settings_json(
    studio: &[McpStdioServer],
    user: &[UserMcpLaunch],
) -> serde_json::Value {
    let allow: Vec<String> = studio
        .iter()
        .map(|server| format!("mcp__{}", server.name))
        .collect();
    let deny: Vec<String> = user
        .iter()
        .flat_map(|server| {
            server
                .disabled_tools
                .iter()
                .map(move |tool| format!("mcp__{}__{tool}", server.name()))
        })
        .collect();
    if deny.is_empty() {
        serde_json::json!({ "permissions": { "allow": allow } })
    } else {
        serde_json::json!({ "permissions": { "allow": allow, "deny": deny } })
    }
}

/// Refuses a release older than [`MIN_VERSION`], with the reason.
pub(crate) fn check_version(version: Version) -> Result<(), String> {
    native_launch::check_version(PRODUCT, MIN_VERSION, version)
}

/// `claude --version`, cached, checked against [`MIN_VERSION`].
pub(crate) fn version_gate(
    executable: &Path,
    prefix: &[String],
    env: &studio_bootstrap::ChildEnvironment,
) -> Result<Version, String> {
    native_launch::version_gate(PRODUCT, MIN_VERSION, executable, prefix, env)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn only_model_and_effort_flags_are_accepted_after_the_program_prefix() {
        let split =
            split_user_args(&strings(&["entry.js", "--model", "haiku", "--effort=low"])).unwrap();
        assert_eq!(split.prefix, strings(&["entry.js"]));
        assert_eq!(
            split.flags,
            strings(&["--model", "haiku", "--effort", "low"])
        );
        for refused in [
            &["--dangerously-skip-permissions"][..],
            &["--permission-mode", "bypassPermissions"],
            &["--settings", "x.json"],
            &["--mcp-config", "x.json"],
            &["--resume", "abc"],
            &["-p"],
        ] {
            assert!(split_user_args(&strings(refused)).is_err(), "{refused:?}");
        }
        assert!(split_user_args(&strings(&["--model"])).is_err());
        assert!(split_user_args(&strings(&["--model", "--effort", "low"])).is_err());
    }

    #[test]
    fn the_wire_flags_ask_the_user_and_load_only_the_studio_settings() {
        let user = UserArgs {
            prefix: vec![],
            flags: strings(&["--model", "haiku"]),
        };
        let args = wire_args(
            &user,
            &SessionStart::New("id-1".into()),
            Path::new("mcp.json"),
            Path::new("settings.json"),
        );
        let joined = args.join(" ");
        for expected in [
            "--permission-prompt-tool stdio",
            "--permission-mode default",
            "--setting-sources project,local",
            "--strict-mcp-config",
            "--replay-user-messages",
            "--session-id id-1",
            "--mcp-config mcp.json",
            "--settings settings.json",
        ] {
            assert!(joined.contains(expected), "{expected}: {joined}");
        }
        assert!(joined.ends_with("--model haiku"));
        assert!(!joined.contains("bypass"));
        let resumed = wire_args(
            &user,
            &SessionStart::Resume("id-2".into()),
            Path::new("m"),
            Path::new("s"),
        );
        assert!(resumed.contains(&"--resume=id-2".to_owned()));
        assert!(!resumed.contains(&"--session-id".to_owned()));
    }

    #[test]
    fn versions_parse_and_old_releases_are_refused() {
        assert_eq!(parse_version("2.1.296 (Claude Code)"), Some((2, 1, 296)));
        assert_eq!(parse_version("v2.2.0"), Some((2, 2, 0)));
        assert_eq!(parse_version("Claude Code"), None);
        assert!(check_version((2, 1, 296)).is_ok());
        assert!(check_version((2, 2, 0)).is_ok());
        assert!(check_version((2, 1, 295)).unwrap_err().contains("2.1.296"));
    }

    #[test]
    fn studio_servers_are_the_only_pre_approved_tools() {
        let server = McpStdioServer::new(
            "studio",
            std::env::current_exe().unwrap(),
            vec![],
            vec![("TOKEN".into(), "v".into())],
        )
        .unwrap();
        let settings = settings_json(std::slice::from_ref(&server), &[]);
        assert_eq!(
            settings,
            serde_json::json!({"permissions": {"allow": ["mcp__studio"]}})
        );
        let config = mcp_config_json(&[server], &[]);
        assert_eq!(config["mcpServers"]["studio"]["env"]["TOKEN"], "v");
        assert_eq!(config["mcpServers"]["studio"]["type"], "stdio");
    }

    #[test]
    fn user_http_servers_carry_their_headers_and_turned_off_tools_are_denied() {
        let studio =
            McpStdioServer::new("studio", std::env::current_exe().unwrap(), vec![], vec![])
                .unwrap();
        let mut docs = UserMcpLaunch::from(
            crate::McpHttpServer::new(
                "docs",
                "https://docs.test/mcp",
                vec![("Authorization".into(), "Bearer tok-123".into())],
            )
            .unwrap(),
        );
        docs.disabled_tools = vec!["delete_page".into()];
        let files = UserMcpLaunch::from(
            McpStdioServer::new("files", std::env::current_exe().unwrap(), vec![], vec![]).unwrap(),
        );
        let config = mcp_config_json(
            std::slice::from_ref(&studio),
            &[docs.clone(), files.clone()],
        );
        assert_eq!(
            config["mcpServers"]["docs"],
            serde_json::json!({
                "type": "http",
                "url": "https://docs.test/mcp",
                "headers": {"Authorization": "Bearer tok-123"},
            })
        );
        assert_eq!(config["mcpServers"]["files"]["type"], "stdio");
        assert_eq!(
            settings_json(&[studio], &[docs, files]),
            serde_json::json!({"permissions": {
                "allow": ["mcp__studio"],
                "deny": ["mcp__docs__delete_page"],
            }})
        );
    }
}
