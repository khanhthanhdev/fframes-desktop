//! Search of the official MCP registry, after Oleafly's registry browser. An entry only
//! pre-fills the add form: nothing runs until the user tests or saves it.
//!
//! npm packages become `npx -y <package>@<version>`, PyPI packages `uvx <package>@<version>`
//! (the command left for the form to find on `PATH`), with the variable names the package
//! lists; streamable-HTTP remotes become HTTP servers whose headers read variables named
//! after them. Anything else is listed with the reason it cannot be pre-filled.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{UserMcpServer, is_env_name, is_header_name};

pub const REGISTRY_URL: &str = "https://registry.modelcontextprotocol.io/v0.1/servers";
const MAX_RESULTS: usize = 20;
/// The most entries one registry page holds (larger limits are refused with 422).
const PAGE_LIMIT: usize = 100;
/// Pages read for one search. The registry's search matches the query anywhere in the
/// name, `io.github.` namespaces included, and lists matches by name, so the servers named
/// after the query can come far down; the results are ranked here instead.
const MAX_PAGES: usize = 3;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_QUERY_BYTES: usize = 256;
const MAX_CURSOR_BYTES: usize = 512;

/// One server in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    /// The registry name, such as `io.github.owner/server`.
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    /// One per package or remote.
    pub options: Vec<RegistryOption>,
}

/// One way to run a registry server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryOption {
    /// What it is: `npm @scope/server@1.2.3` or `HTTP https://…`.
    pub label: String,
    /// The server to pre-fill the form with, or why it cannot be.
    pub draft: Result<UserMcpServer, String>,
}

/// An entry of a page, with the title the ranking also reads.
struct Found {
    entry: RegistryEntry,
    title: Option<String>,
}

/// Searches the registry for `query` (latest versions only): up to [`MAX_PAGES`] pages,
/// best matches first ([`rank`]). Blocks for up to 15 seconds a page; run it off the UI
/// thread.
pub fn search_registry(query: &str) -> Result<Vec<RegistryEntry>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Enter a name to search the registry.".into());
    }
    if query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control) {
        return Err("Use a shorter search.".into());
    }
    let mut found = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{REGISTRY_URL}?search={}&limit={PAGE_LIMIT}&version=latest",
            encode(query)
        );
        if let Some(cursor) = &cursor {
            url.push_str("&cursor=");
            url.push_str(&encode(cursor));
        }
        let (page, next) = parse_page(&fetch(&url)?)?;
        found.extend(page);
        // Later pages can only add more servers named after the query, in name order.
        let named = found.iter().filter(|f| tier(query, f) <= 1).count();
        match next {
            Some(next) if named < MAX_RESULTS => cursor = Some(next),
            _ => break,
        }
    }
    Ok(rank(query, found))
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let output = super::http::curl()
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "15",
            "--proto",
            "=https",
            "--max-filesize",
            &MAX_RESPONSE_BYTES.to_string(),
            "--header",
            "Accept: application/json",
            url,
        ])
        .output()
        .map_err(|e| format!("Cannot search the registry: curl is unavailable ({e})"))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot reach the MCP registry: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// How well an entry matches `query` (lower is better): 0 the last part of its name or
/// its title is the query, 1 it contains it, 2 its description does, 3 only its namespace
/// does (every `io.github.<owner>` matches "github").
fn tier(query: &str, found: &Found) -> u8 {
    let query = query.to_lowercase();
    let last = found
        .entry
        .name
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let title = found.title.as_deref().unwrap_or_default().to_lowercase();
    if last == query || title == query {
        0
    } else if last.contains(&query) || title.contains(&query) {
        1
    } else if found
        .entry
        .description
        .as_deref()
        .is_some_and(|d| d.to_lowercase().contains(&query))
    {
        2
    } else {
        3
    }
}

/// The [`MAX_RESULTS`] best matches, in the registry's order within a tier.
fn rank(query: &str, mut found: Vec<Found>) -> Vec<RegistryEntry> {
    found.sort_by_key(|f| tier(query, f));
    found
        .into_iter()
        .take(MAX_RESULTS)
        .map(|f| f.entry)
        .collect()
}
/// Percent-encodes a query value.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The entries of a registry answer (`{"servers": [{"server": {...}}], "metadata": ...}`).
/// Entries without a name or version are left out.
pub fn parse_registry(body: &[u8]) -> Result<Vec<RegistryEntry>, String> {
    let (found, _) = parse_page(body)?;
    Ok(found
        .into_iter()
        .take(MAX_RESULTS)
        .map(|f| f.entry)
        .collect())
}

/// A page's entries and the cursor of the next page.
fn parse_page(body: &[u8]) -> Result<(Vec<Found>, Option<String>), String> {
    let root: Value = serde_json::from_slice(body)
        .map_err(|_| "The MCP registry answered with something that is not JSON.".to_owned())?;
    let servers = root["servers"]
        .as_array()
        .ok_or("The MCP registry's answer lists no servers.")?;
    let found = servers
        .iter()
        .filter_map(|item| {
            let server = &item["server"];
            Some(Found {
                entry: entry(server)?,
                title: text(&server["title"], 512).map(str::to_owned),
            })
        })
        .collect();
    let next = text(&root["metadata"]["nextCursor"], MAX_CURSOR_BYTES).map(str::to_owned);
    Ok((found, next))
}

fn text(value: &Value, max: usize) -> Option<&str> {
    value
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control))
}

fn entry(server: &Value) -> Option<RegistryEntry> {
    let name = text(&server["name"], 512)?;
    let version = text(&server["version"], 255)?;
    let mut options: Vec<RegistryOption> = server["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|package| package_option(name, package))
        .collect();
    options.extend(
        server["remotes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|remote| remote_option(name, remote)),
    );
    Some(RegistryEntry {
        name: name.to_owned(),
        description: text(&server["description"], 4096).map(str::to_owned),
        version: version.to_owned(),
        options,
    })
}

/// The name the form starts with: the last part of the registry name, as a server name.
/// A generic last part (`com.notion/mcp`) gives way to the namespace's last label
/// (`notion`), so drafts from different servers are not all called `mcp`.
fn server_name(registry_name: &str) -> String {
    const GENERIC: [&str; 7] = [
        "mcp",
        "server",
        "mcp-server",
        "mcp_server",
        "mcpserver",
        "server-mcp",
        "remote",
    ];
    const TLDS: [&str; 7] = ["io", "com", "org", "net", "dev", "ai", "app"];
    let (namespace, last) = registry_name
        .rsplit_once('/')
        .unwrap_or(("", registry_name));
    let last = as_server_name(last);
    if !GENERIC.contains(&last.to_ascii_lowercase().as_str()) && !last.is_empty() {
        return last;
    }
    namespace
        .rsplit('.')
        .map(as_server_name)
        .find(|label| {
            let lower = label.to_ascii_lowercase();
            !label.is_empty()
                && !GENERIC.contains(&lower.as_str())
                && !TLDS.contains(&lower.as_str())
        })
        .unwrap_or(if last.is_empty() {
            "server".to_owned()
        } else {
            last
        })
}

/// `text` as a server name: letters, digits, `-` and `_` (anything else becomes `-`),
/// without leading or trailing `-`, at most 48 characters.
fn as_server_name(text: &str) -> String {
    let name: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name: String = name.trim_matches('-').chars().take(48).collect();
    name.trim_end_matches('-').to_owned()
}
/// `positional` argument values; a named or templated argument cannot be pre-filled.
fn arguments(package: &Value, field: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    for argument in package[field].as_array().into_iter().flatten() {
        let value = text(&argument["value"], 512)
            .filter(|value| !value.contains(['{', '}']))
            .filter(|_| argument["type"].as_str() == Some("positional"));
        match value {
            Some(value) => values.push(value.to_owned()),
            None => {
                return Err(
                    "It needs arguments you choose yourself. Add it by hand with Add server."
                        .into(),
                );
            }
        }
    }
    Ok(values)
}

fn package_option(registry_name: &str, package: &Value) -> RegistryOption {
    let kind = package["registryType"].as_str().unwrap_or("package");
    let identifier = text(&package["identifier"], 512).unwrap_or("");
    let version = text(&package["version"], 255);
    let label = match version {
        Some(version) => format!("{kind} {identifier}@{version}"),
        None => format!("{kind} {identifier}"),
    };
    let draft = (|| {
        let runner = match kind {
            "npm" => "npx",
            "pypi" => "uvx",
            _ => return Err("Studio can pre-fill npm and PyPI packages and HTTP servers.".into()),
        };
        if package["transport"]["type"].as_str().unwrap_or("stdio") != "stdio" {
            return Err("This package does not run over stdio.".into());
        }
        if identifier.is_empty()
            || !identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '-' | '_' | '.'))
        {
            return Err("The package name is not one Studio can pass on.".into());
        }
        let mut args = arguments(package, "runtimeArguments")?;
        if runner == "npx" && !args.iter().any(|arg| arg == "-y" || arg == "--yes") {
            args.insert(0, "-y".into());
        }
        args.push(match version {
            Some(version) if version != "latest" => format!("{identifier}@{version}"),
            _ => identifier.to_owned(),
        });
        args.extend(arguments(package, "packageArguments")?);
        let env = package["environmentVariables"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|variable| variable["name"].as_str())
            .filter(|name| is_env_name(name))
            .map(str::to_owned)
            .collect();
        Ok(UserMcpServer::stdio(
            server_name(registry_name),
            runner,
            args,
            env,
        ))
    })();
    RegistryOption { label, draft }
}

fn remote_option(registry_name: &str, remote: &Value) -> RegistryOption {
    let kind = remote["type"].as_str().unwrap_or("remote");
    let url = text(&remote["url"], 2048).unwrap_or("");
    let label = format!("HTTP {url}");
    let draft = (|| {
        if kind != "streamable-http" {
            return Err(format!(
                "Studio takes streamable HTTP servers; this one uses {}.",
                kind.to_uppercase()
            ));
        }
        if !url.starts_with("https://") || url.contains(['{', '}', ' ']) {
            return Err("The URL needs values you choose yourself. Add it by hand.".into());
        }
        let name = server_name(registry_name);
        let prefix: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        let mut bearer_env = None;
        let mut header_env = BTreeMap::new();
        for header in remote["headers"].as_array().into_iter().flatten() {
            let Some(header) = header["name"].as_str().filter(|h| is_header_name(h)) else {
                continue;
            };
            if header.eq_ignore_ascii_case("authorization") {
                bearer_env = Some(format!("{prefix}_TOKEN"));
            } else {
                let variable: String = header
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() {
                            c.to_ascii_uppercase()
                        } else {
                            '_'
                        }
                    })
                    .collect();
                header_env.insert(header.to_owned(), format!("{prefix}_{variable}"));
            }
        }
        Ok(UserMcpServer::http(name, url, bearer_env, header_env))
    })();
    RegistryOption { label, draft }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_settings::ServerTransport;

    const ANSWER: &str = r#"{
      "servers": [
        {"server": {
          "$schema": "https://static.modelcontextprotocol.io/schemas/2025-09-29/server.schema.json",
          "name": "io.github.example/papers", "description": "Search papers.", "version": "1.2.3",
          "packages": [
            {"registryType": "npm", "registryBaseUrl": "https://registry.npmjs.org", "identifier": "@example/papers",
             "version": "1.2.3", "transport": {"type": "stdio"},
             "environmentVariables": [{"name": "PAPERS_TOKEN", "isSecret": true}, {"name": "bad name"}]},
            {"registryType": "pypi", "identifier": "papers-mcp", "version": "0.4.0", "transport": {"type": "stdio"},
             "packageArguments": [{"type": "positional", "value": "--stdio"}]},
            {"registryType": "oci", "identifier": "docker.io/example/papers", "version": "1.2.3"},
            {"registryType": "npm", "identifier": "@example/papers", "version": "1.2.3",
             "packageArguments": [{"type": "named", "name": "--root", "value": "{root}"}]}
          ],
          "remotes": [
            {"type": "streamable-http", "url": "https://papers.example.com/mcp",
             "headers": [{"name": "Authorization", "isSecret": true}, {"name": "X-Org-Id"}]},
            {"type": "sse", "url": "https://papers.example.com/sse"}
          ]
        }},
        {"server": {"name": "no-version"}},
        {"server": {"name": "io.example/empty", "version": "2.0.0"}}
      ],
      "metadata": {"nextCursor": "x", "count": 3}
    }"#;

    #[test]
    fn registry_entries_pre_fill_npm_pypi_and_http_servers_and_say_why_others_cannot() {
        let entries = parse_registry(ANSWER.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2, "an entry without a version is left out");
        let papers = &entries[0];
        assert_eq!(papers.name, "io.github.example/papers");
        assert_eq!(papers.description.as_deref(), Some("Search papers."));
        assert_eq!(papers.options.len(), 6);

        let npm = papers.options[0].draft.as_ref().unwrap();
        assert_eq!(papers.options[0].label, "npm @example/papers@1.2.3");
        assert_eq!(
            npm,
            &UserMcpServer::stdio(
                "papers",
                "npx",
                vec!["-y".into(), "@example/papers@1.2.3".into()],
                vec!["PAPERS_TOKEN".into()],
            )
        );
        let pypi = papers.options[1].draft.as_ref().unwrap();
        assert_eq!(
            pypi.transport,
            ServerTransport::Stdio {
                command: "uvx".into(),
                args: vec!["papers-mcp@0.4.0".into(), "--stdio".into()],
                env: vec![],
            }
        );
        assert!(
            papers.options[2]
                .draft
                .as_ref()
                .unwrap_err()
                .contains("npm and PyPI")
        );
        assert!(papers.options[3].draft.is_err(), "templated arguments");

        let remote = papers.options[4].draft.as_ref().unwrap();
        assert_eq!(
            remote,
            &UserMcpServer::http(
                "papers",
                "https://papers.example.com/mcp",
                Some("PAPERS_TOKEN".into()),
                BTreeMap::from([("X-Org-Id".to_owned(), "PAPERS_X_ORG_ID".to_owned())]),
            )
        );
        assert_eq!(
            remote.problem(&[]),
            None,
            "a pre-filled HTTP server can be saved"
        );
        assert!(
            papers.options[5]
                .draft
                .as_ref()
                .unwrap_err()
                .contains("uses SSE.")
        );
        assert!(entries[1].options.is_empty());
    }

    #[test]
    fn servers_named_after_the_query_come_before_namespace_only_matches() {
        // As the registry lists a "github" search: by name, so every `io.github.<owner>`
        // server comes first whatever it is.
        let page = r#"{
          "servers": [
            {"server": {"name": "io.github.aaa/notes", "version": "1", "description": "Notes."}},
            {"server": {"name": "io.github.abc/issues-sync", "version": "1",
                        "description": "Syncs GitHub issues."}},
            {"server": {"name": "io.github.bob/tracker", "version": "1", "title": "GitHub tracker"}},
            {"server": {"name": "io.github.github/github-mcp-server", "version": "1"}},
            {"server": {"name": "io.github.zed/github", "version": "1"}}
          ],
          "metadata": {"nextCursor": "io.github.zed/github:1", "count": 5}
        }"#;
        let (found, next) = parse_page(page.as_bytes()).unwrap();
        assert_eq!(next.as_deref(), Some("io.github.zed/github:1"));
        let names: Vec<String> = rank("GitHub", found).into_iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            [
                "io.github.zed/github",
                "io.github.bob/tracker",
                "io.github.github/github-mcp-server",
                "io.github.abc/issues-sync",
                "io.github.aaa/notes",
            ]
        );
    }

    #[test]
    fn a_generic_last_name_gives_way_to_the_namespace() {
        for (registry, name) in [
            ("com.notion/mcp", "notion"),
            ("com.githubcard/mcp", "githubcard"),
            ("io.github.github/github-mcp-server", "github-mcp-server"),
            ("io.github.someone/MCP-Server", "someone"),
            ("ai.smithery/server", "smithery"),
            ("io.example/papers", "papers"),
            ("mcp", "mcp"),
            ("io/mcp", "mcp"),
            ("io.example/§§", "example"),
        ] {
            let got = server_name(registry);
            assert_eq!(got, name, "{registry}");
            let server = UserMcpServer::http(&got, "https://x.test/mcp", None, BTreeMap::new());
            assert_eq!(server.problem(&[]), None, "{registry} → {got}");
        }
        let long = format!("com.{}/mcp", "a".repeat(80));
        assert_eq!(server_name(&long).len(), 48);
    }

    #[test]
    fn a_broken_answer_is_an_error_and_queries_are_encoded() {
        assert!(parse_registry(b"<html>").is_err());
        assert!(parse_registry(b"{\"servers\": 3}").is_err());
        assert_eq!(parse_registry(b"{\"servers\": []}").unwrap(), vec![]);
        assert_eq!(encode("git hub/é&x"), "git%20hub%2F%C3%A9%26x");
        assert!(search_registry("  ").is_err());
    }
}
