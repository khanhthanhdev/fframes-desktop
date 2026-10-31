//! Codex launch: user flags, the version gate and the thread and turn settings that keep
//! every approval with the user and every write inside the draft.

use super::super::{
    McpStdioServer, McpTransport, UserMcpLaunch,
    native_launch::{self, UserArgs, Version},
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) const PRODUCT: &str = "Codex";
/// The oldest Codex release the app-server wire was validated against (S5 and the
/// generated schema).
pub(crate) const MIN_VERSION: Version = (0, 162, 1);
/// Asks before every command and file change; `on-request` asked for nothing in S5.
pub(crate) const APPROVAL_POLICY: &str = "untrusted";

/// User arguments a Codex adapter may carry: the model (`thread/start`) and the reasoning
/// effort (`turn/start`). Policies, sandbox, config and MCP servers are the driver's own.
const USER_FLAGS: [&str; 2] = ["--model", "--effort"];

pub(crate) fn split_user_args(args: &[String]) -> Result<UserArgs, String> {
    native_launch::split_user_args(PRODUCT, &USER_FLAGS, args)
}

/// `codex --version`, cached, checked against [`MIN_VERSION`].
pub(crate) fn version_gate(
    executable: &Path,
    prefix: &[String],
    env: &studio_bootstrap::ChildEnvironment,
) -> Result<Version, String> {
    native_launch::version_gate(PRODUCT, MIN_VERSION, executable, prefix, env)
}

pub(crate) fn check_version(version: Version) -> Result<(), String> {
    native_launch::check_version(PRODUCT, MIN_VERSION, version)
}

/// The full argument list after the program prefix: no `-c` overrides, the session's
/// settings travel in `thread/start` so no secret reaches the command line.
pub(crate) fn wire_args(user: &UserArgs) -> Vec<String> {
    let mut args = user.prefix.clone();
    args.push("app-server".into());
    args
}

/// `thread/start` / `thread/resume` config overrides: the user's own MCP servers and the
/// ChatGPT apps connector are switched off and only the studio's servers run.
/// `-c mcp_servers={}` and a whole `mcp_servers` object both leave the user's servers on
/// (they merge), so each one is disabled by name.
///
/// Studio's servers and the user's servers chosen in Studio are set field by field, as
/// Codex's `config.toml` `[mcp_servers.<name>]` keys (codex-rs `RawMcpServerConfig`): stdio
/// `command`, `args`, `env`; streamable HTTP `url` and `http_headers` (literal values: they
/// travel in this JSON-RPC request, never on a command line; Codex refuses a literal
/// `bearer_token`, so a token goes as an `Authorization` header); and `disabled_tools`, the
/// deny-list Codex applies to the server's tools.
///
/// A server is never sent under a name Codex's own configuration already has. These keys
/// are one more config layer that Codex merges key by key into the user's
/// `[mcp_servers.<name>]` table (codex-rs `merge_toml_values`); a key can be set but never
/// removed (a JSON `null` becomes an empty string, `json_to_toml`). The user's `command`
/// would stay beside Studio's `url`, which Codex refuses ("url is not supported for stdio",
/// `RawMcpServerConfig` → `McpServerConfig`), and any other key the user set there (`cwd`,
/// `enabled_tools`, `bearer_token_env_var`, …) would change Studio's server. So such a
/// server runs as `studio-<name>` (see [`session_name`]) and the user's entry is switched
/// off like every other configured server.
pub(crate) fn thread_config(
    configured: &[String],
    servers: &[McpStdioServer],
    user: &[UserMcpLaunch],
) -> ThreadConfig {
    let mut config = Map::new();
    config.insert("features.apps".into(), Value::Bool(false));
    for name in configured {
        config.insert(format!("mcp_servers.{name}.enabled"), Value::Bool(false));
    }
    let wanted: Vec<&str> = servers
        .iter()
        .map(|server| server.name.as_str())
        .chain(user.iter().map(UserMcpLaunch::name))
        .collect();
    let mut taken: Vec<String> = Vec::new();
    let mut renamed = BTreeMap::new();
    let mut name_of = |name: &str| -> String {
        let session = session_name(name, configured, &wanted, &taken);
        taken.push(session.clone());
        if session != name {
            renamed.insert(session.clone(), name.to_owned());
        }
        session
    };
    let pairs = |pairs: &[(String, String)]| {
        Value::Object(
            pairs
                .iter()
                .map(|(name, value)| (name.clone(), Value::from(value.clone())))
                .collect(),
        )
    };
    let stdio = |config: &mut Map<String, Value>, name: &str, server: &McpStdioServer| {
        let key = |field: &str| format!("mcp_servers.{name}.{field}");
        config.insert(key("command"), json!(server.command));
        config.insert(key("args"), json!(server.args));
        config.insert(key("env"), pairs(&server.env));
        config.insert(key("enabled"), Value::Bool(true));
    };
    for server in servers {
        let name = name_of(&server.name);
        stdio(&mut config, &name, server);
    }
    for server in user {
        let name = name_of(server.name());
        let key = |field: &str| format!("mcp_servers.{name}.{field}");
        match &server.transport {
            McpTransport::Stdio(launch) => stdio(&mut config, &name, launch),
            McpTransport::Http(http) => {
                config.insert(key("url"), json!(http.url));
                if !http.headers.is_empty() {
                    config.insert(key("http_headers"), pairs(&http.headers));
                }
                config.insert(key("enabled"), Value::Bool(true));
            }
        }
        if !server.disabled_tools.is_empty() {
            config.insert(key("disabled_tools"), json!(server.disabled_tools));
        }
    }
    ThreadConfig {
        config: Value::Object(config),
        renamed,
    }
}

/// The `thread/start` config overrides, and the servers Codex knows by another name.
pub(crate) struct ThreadConfig {
    pub(crate) config: Value,
    /// Name in the Codex session → the server's own name, for each server renamed because
    /// Codex's configuration already has its name.
    pub(crate) renamed: BTreeMap<String, String>,
}

/// The name `name` runs under: itself, unless Codex's configuration has it; then
/// `studio-<name>` (then `studio-<name>-2`, …), never a configured name, another server's
/// own name or a name already given.
fn session_name(name: &str, configured: &[String], wanted: &[&str], taken: &[String]) -> String {
    let free = |candidate: &str| {
        !configured.iter().any(|c| c == candidate)
            && !taken.iter().any(|t| t == candidate)
            && (candidate == name || !wanted.contains(&candidate))
    };
    if free(name) {
        return name.to_owned();
    }
    (1..)
        .map(|n| match n {
            1 => format!("studio-{name}"),
            n => format!("studio-{name}-{n}"),
        })
        .find(|candidate| free(candidate))
        .expect("an unused name")
}

/// The turn sandbox: writes only inside the draft (the thread's working directory), no
/// network, and neither `/tmp` nor `$TMPDIR` as extra writable roots.
pub(crate) fn sandbox_policy() -> Value {
    json!({
        "type": "workspaceWrite",
        "writableRoots": [],
        "networkAccess": false,
        "excludeTmpdirEnvVar": true,
        "excludeSlashTmp": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn only_model_and_effort_are_accepted_and_nothing_else_reaches_the_command_line() {
        let user = split_user_args(&strings(&["--model", "m", "--effort", "low"])).unwrap();
        assert_eq!(wire_args(&user), strings(&["app-server"]));
        for refused in [
            &["-c", "approval_policy=never"][..],
            &["--dangerously-bypass-approvals-and-sandbox"],
            &["--sandbox", "danger-full-access"],
            &["--config", "x"],
        ] {
            assert!(split_user_args(&strings(refused)).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn user_servers_are_disabled_and_only_studio_servers_run() {
        let server = McpStdioServer::new(
            "studio",
            std::env::current_exe().unwrap(),
            vec!["--serve".into()],
            vec![("TOKEN".into(), "v".into())],
        )
        .unwrap();
        let config = thread_config(&strings(&["rea"]), &[server], &[]);
        assert!(config.renamed.is_empty());
        let config = config.config;
        assert_eq!(config["features.apps"], false);
        assert_eq!(config["mcp_servers.rea.enabled"], false);
        assert_eq!(config["mcp_servers.studio.enabled"], true);
        assert_eq!(config["mcp_servers.studio.env"]["TOKEN"], "v");
        assert_eq!(config["mcp_servers.studio.args"], json!(["--serve"]));
        assert_eq!(sandbox_policy()["type"], "workspaceWrite");
        assert_eq!(sandbox_policy()["networkAccess"], false);
    }

    #[test]
    fn user_http_servers_use_codex_keys_and_turned_off_tools_are_disabled() {
        let mut docs = UserMcpLaunch::from(
            crate::McpHttpServer::new(
                "docs",
                "https://docs.test/mcp",
                vec![("Authorization".into(), "Bearer tok-123".into())],
            )
            .unwrap(),
        );
        docs.disabled_tools = vec!["delete_page".into()];
        let mut files = UserMcpLaunch::from(
            McpStdioServer::new(
                "files",
                std::env::current_exe().unwrap(),
                vec![],
                vec![("ROOT_TOKEN".into(), "secret-1".into())],
            )
            .unwrap(),
        );
        files.disabled_tools = vec!["write_file".into()];
        let config = thread_config(&strings(&["other"]), &[], &[docs, files]).config;
        assert_eq!(config["mcp_servers.docs.url"], "https://docs.test/mcp");
        assert_eq!(
            config["mcp_servers.docs.http_headers"],
            json!({"Authorization": "Bearer tok-123"})
        );
        assert_eq!(config["mcp_servers.docs.enabled"], true);
        assert_eq!(
            config["mcp_servers.docs.disabled_tools"],
            json!(["delete_page"])
        );
        assert!(config.get("mcp_servers.docs.command").is_none());
        assert!(config.get("mcp_servers.docs.bearer_token").is_none());
        assert_eq!(config["mcp_servers.files.enabled"], true);
        assert_eq!(config["mcp_servers.files.env"]["ROOT_TOKEN"], "secret-1");
        assert_eq!(
            config["mcp_servers.files.disabled_tools"],
            json!(["write_file"])
        );
        assert_eq!(config["mcp_servers.other.enabled"], false);
    }

    #[test]
    fn a_server_whose_name_codex_already_has_runs_under_another_name() {
        // config.toml has stdio `docs` and `files` (and a `studio-files` of its own): the
        // keys sent would merge into those entries, and Codex refuses `url` beside
        // `command`. Studio's servers run under names nobody configured.
        let docs = UserMcpLaunch::from(
            crate::McpHttpServer::new("docs", "https://docs.test/mcp", vec![]).unwrap(),
        );
        let mut files = UserMcpLaunch::from(
            McpStdioServer::new("files", std::env::current_exe().unwrap(), vec![], vec![]).unwrap(),
        );
        files.disabled_tools = vec!["write_file".into()];
        let thread = thread_config(
            &strings(&["docs", "files", "studio-files"]),
            &[],
            &[docs, files],
        );
        let config = &thread.config;
        for name in ["docs", "files", "studio-files"] {
            let entry: Vec<&String> = config
                .as_object()
                .unwrap()
                .keys()
                .filter(|key| key.starts_with(&format!("mcp_servers.{name}.")))
                .collect();
            assert_eq!(
                entry,
                [&format!("mcp_servers.{name}.enabled")],
                "only switched off"
            );
            assert_eq!(config[format!("mcp_servers.{name}.enabled")], false);
        }
        assert_eq!(
            config["mcp_servers.studio-docs.url"],
            "https://docs.test/mcp"
        );
        assert_eq!(config["mcp_servers.studio-docs.enabled"], true);
        assert_eq!(config["mcp_servers.studio-files-2.enabled"], true);
        assert_eq!(
            config["mcp_servers.studio-files-2.disabled_tools"],
            json!(["write_file"])
        );
        assert_eq!(
            thread.renamed,
            BTreeMap::from([
                ("studio-docs".to_owned(), "docs".to_owned()),
                ("studio-files-2".to_owned(), "files".to_owned()),
            ])
        );
    }
}
