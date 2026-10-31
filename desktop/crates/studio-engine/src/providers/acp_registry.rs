//! The public ACP agent registry, for Settings → Agent → CLI agents → Find more agents.
//!
//! Fetched only when the user asks, with the system's `curl` (as [`crate::updates`] does), so
//! the app needs no HTTP client. Studio only lists what the registry describes: an entry can
//! fill in the custom agent form or open the agent's page, and nothing is downloaded,
//! installed or started from here.
use std::process::Command;

use serde_json::Value;

use super::provider_profiles::{
    BUILTIN_ANTIGRAVITY_ID, BUILTIN_CLAUDE_ID, BUILTIN_CODEX_ID, BUILTIN_PI_ID, is_openable,
};

/// The registry's latest agent list.
pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The largest registry answer read, in bytes.
const REGISTRY_LIMIT: u64 = 4 * 1024 * 1024;
/// The longest description kept, in characters.
const DESCRIPTION_LIMIT: usize = 400;

/// Registry ids of the agents Studio lists as built-in profiles.
const BUILTIN_ENTRIES: [(&str, &str); 4] = [
    ("claude-acp", BUILTIN_CLAUDE_ID),
    ("codex-acp", BUILTIN_CODEX_ID),
    ("pi-acp", BUILTIN_PI_ID),
    ("antigravity-acp", BUILTIN_ANTIGRAVITY_ID),
];

/// One way the registry says an agent is distributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Distribution {
    /// An npm package, run with `npx` by other clients.
    Npm { package: String, args: Vec<String> },
    /// A Python package, run with `uvx` by other clients.
    Python { package: String, args: Vec<String> },
    /// Release archives per platform. `here` is the build for this computer, if any.
    Binary {
        platforms: usize,
        here: Option<BinaryTarget>,
    },
}

/// The binary build of an agent for this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryTarget {
    /// The executable's file name inside the archive, without any folder.
    pub file_name: String,
    pub args: Vec<String>,
}

/// One agent the registry lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub website: Option<String>,
    pub repository: Option<String>,
    pub distribution: Vec<Distribution>,
}

/// What an entry fills in on the custom agent form. The executable is left to the user
/// unless a file of [`Self::executable_name`] is found where Detect agents looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomDraft {
    pub label: String,
    pub args: Vec<String>,
    /// The binary's file name for this computer, without `.exe`, `.cmd` or `.bat`.
    pub executable_name: Option<String>,
}

impl RegistryAgent {
    /// The built-in profile this entry is, if Studio already lists it.
    pub fn builtin_profile(&self) -> Option<&'static str> {
        BUILTIN_ENTRIES
            .iter()
            .find(|(registry, _)| *registry == self.id)
            .map(|(_, profile)| *profile)
    }

    /// Whether `query` (any case) is in the id, name or description. An empty query matches.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || [&self.id, &self.name, &self.description]
                .iter()
                .any(|text| text.to_lowercase().contains(&query))
    }

    /// The page to open for this agent: its website, its repository, or its npm page. Only
    /// `https://` links.
    pub fn page_url(&self) -> Option<String> {
        let npm = self.distribution.iter().find_map(|kind| match kind {
            Distribution::Npm { package, .. } => Some(format!(
                "https://www.npmjs.com/package/{}",
                package_name(package)
            )),
            _ => None,
        });
        [self.website.clone(), self.repository.clone(), npm]
            .into_iter()
            .flatten()
            .find(|url| is_openable(url))
    }

    /// How the agent is distributed, in a few words: "npm package agent@1.2.0",
    /// "Binary for this computer", ….
    pub fn distribution_label(&self) -> String {
        let kinds: Vec<String> = self
            .distribution
            .iter()
            .map(|kind| match kind {
                Distribution::Npm { package, .. } => format!("npm package {package}"),
                Distribution::Python { package, .. } => format!("Python package {package}"),
                Distribution::Binary { here: Some(_), .. } => "Binary for this computer".into(),
                Distribution::Binary { platforms, .. } => {
                    format!("Binary for {platforms} other platforms")
                }
            })
            .collect();
        if kinds.is_empty() {
            "No distribution listed".into()
        } else {
            kinds.join(" · ")
        }
    }

    /// What the entry fills in on the custom agent form: the arguments of the binary for
    /// this computer when there is one, otherwise those of its package.
    pub fn custom_draft(&self) -> CustomDraft {
        let binary = self.distribution.iter().find_map(|kind| match kind {
            Distribution::Binary {
                here: Some(target), ..
            } => Some(target),
            _ => None,
        });
        let args = binary.map(|target| target.args.clone()).or_else(|| {
            self.distribution.iter().find_map(|kind| match kind {
                Distribution::Npm { args, .. } | Distribution::Python { args, .. } => {
                    Some(args.clone())
                }
                Distribution::Binary { .. } => None,
            })
        });
        CustomDraft {
            label: self.name.chars().take(64).collect(),
            args: args.unwrap_or_default(),
            executable_name: binary.map(|target| {
                let name = target.file_name.as_str();
                [".exe", ".cmd", ".bat"]
                    .iter()
                    .find_map(|extension| name.strip_suffix(extension))
                    .unwrap_or(name)
                    .to_owned()
            }),
        }
    }
}

/// `name@1.2.0` or `@scope/name@1.2.0` without its version.
fn package_name(package: &str) -> &str {
    match package.rfind('@') {
        Some(at) if at > 0 => &package[..at],
        _ => package,
    }
}

/// The registry's name for this computer's platform, such as `windows-x86_64`.
pub fn current_platform() -> Option<&'static str> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "darwin",
        "linux" => "linux",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some(match (os, arch) {
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", _) => "windows-aarch64",
        ("darwin", "x86_64") => "darwin-x86_64",
        ("darwin", _) => "darwin-aarch64",
        ("linux", "x86_64") => "linux-x86_64",
        _ => "linux-aarch64",
    })
}

/// Reads the registry's JSON, with `platform` naming this computer's binary builds. An
/// entry without an id or a name is left out; unknown fields are ignored.
pub fn parse_registry(json: &[u8], platform: Option<&str>) -> Result<Vec<RegistryAgent>, String> {
    let payload: Value = serde_json::from_slice(json)
        .map_err(|e| format!("The agent registry sent something unexpected ({e})."))?;
    let agents = payload["agents"]
        .as_array()
        .ok_or("The agent registry has no agent list.")?;
    let text = |value: &Value| value.as_str().map(str::trim).unwrap_or_default().to_owned();
    let link = |value: &Value| Some(text(value)).filter(|url| is_openable(url));
    let strings = |value: &Value| {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut list: Vec<RegistryAgent> = agents
        .iter()
        .filter_map(|agent| {
            let (id, name) = (text(&agent["id"]), text(&agent["name"]));
            if id.is_empty() || name.is_empty() {
                return None;
            }
            let found = &agent["distribution"];
            let mut distribution = Vec::new();
            if let Some(binary) = found["binary"].as_object() {
                let here = platform
                    .and_then(|platform| binary.get(platform))
                    .and_then(|target| {
                        let cmd = text(&target["cmd"]);
                        let file_name = cmd.rsplit(['/', '\\']).next()?.to_owned();
                        (!file_name.is_empty()).then(|| BinaryTarget {
                            file_name,
                            args: strings(&target["args"]),
                        })
                    });
                distribution.push(Distribution::Binary {
                    platforms: binary.len(),
                    here,
                });
            }
            for (key, python) in [("npx", false), ("uvx", true)] {
                let package = text(&found[key]["package"]);
                if package.is_empty() {
                    continue;
                }
                let args = strings(&found[key]["args"]);
                distribution.push(if python {
                    Distribution::Python { package, args }
                } else {
                    Distribution::Npm { package, args }
                });
            }
            Some(RegistryAgent {
                id,
                name,
                description: text(&agent["description"])
                    .chars()
                    .take(DESCRIPTION_LIMIT)
                    .collect(),
                version: text(&agent["version"]),
                website: link(&agent["website"]),
                repository: link(&agent["repository"]),
                distribution,
            })
        })
        .collect();
    list.sort_by_key(|agent| agent.name.to_lowercase());
    Ok(list)
}

/// Downloads and reads the registry. Blocks for up to 20 seconds; run it off the UI thread.
pub fn fetch_registry() -> Result<Vec<RegistryAgent>, String> {
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--proto",
        "=https",
        "--max-time",
        "20",
        "--max-filesize",
        &REGISTRY_LIMIT.to_string(),
        REGISTRY_URL,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("Cannot reach the agent registry: curl is unavailable ({e})."))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot reach the agent registry: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_registry(&output.stdout, current_platform())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "version": "1.0.0",
      "agents": [
        {
          "id": "zeta-acp", "name": "Zeta", "version": "0.10.0",
          "description": "  A native agent.  ",
          "repository": "https://github.com/example/zeta",
          "distribution": { "binary": {
            "windows-x86_64": { "archive": "https://example.com/z.zip", "cmd": "./bin/zeta-acp.exe", "args": ["acp"] },
            "linux-x86_64": { "archive": "https://example.com/z.tar.gz", "cmd": "./zeta-acp" }
          } }
        },
        {
          "id": "claude-acp", "name": "Claude Agent", "version": "0.31.0",
          "description": "ACP wrapper for Claude Code",
          "distribution": { "npx": { "package": "@agentclientprotocol/claude-agent-acp@0.31.0" } }
        },
        {
          "id": "alpha", "name": "Alpha", "version": "2.0.0",
          "website": "http://insecure.example.com",
          "distribution": { "uvx": { "package": "alpha-agent==2.0.0", "args": ["serve", "--acp"] } }
        },
        { "id": "", "name": "No id" },
        { "id": "no-name" }
      ],
      "extensions": []
    }"#;

    fn parsed(platform: Option<&str>) -> Vec<RegistryAgent> {
        parse_registry(FIXTURE.as_bytes(), platform).unwrap()
    }

    #[test]
    fn reads_entries_sorted_by_name_and_skips_nameless_ones() {
        let agents = parsed(Some("windows-x86_64"));
        let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Claude Agent", "Zeta"]);
        let zeta = &agents[2];
        assert_eq!(zeta.description, "A native agent.");
        assert_eq!(zeta.version, "0.10.0");
        assert_eq!(
            zeta.distribution,
            [Distribution::Binary {
                platforms: 2,
                here: Some(BinaryTarget {
                    file_name: "zeta-acp.exe".into(),
                    args: vec!["acp".into()],
                }),
            }]
        );
        assert_eq!(zeta.distribution_label(), "Binary for this computer");
        assert_eq!(
            parsed(Some("darwin-aarch64"))[2].distribution_label(),
            "Binary for 2 other platforms"
        );
    }

    #[test]
    fn rejects_an_answer_that_is_not_the_registry() {
        assert!(parse_registry(b"<html>", None).is_err());
        assert!(parse_registry(br#"{"message":"Not Found"}"#, None).is_err());
    }

    #[test]
    fn a_binary_for_this_computer_fills_its_name_and_arguments() {
        let agents = parsed(Some("windows-x86_64"));
        let draft = agents[2].custom_draft();
        assert_eq!(draft.label, "Zeta");
        assert_eq!(draft.args, ["acp"]);
        assert_eq!(draft.executable_name.as_deref(), Some("zeta-acp"));
        // A package fills its arguments only; Studio never runs npx or uvx for it.
        let alpha = agents[0].custom_draft();
        assert_eq!(alpha.args, ["serve", "--acp"]);
        assert_eq!(alpha.executable_name, None);
        assert_eq!(
            agents[0].distribution_label(),
            "Python package alpha-agent==2.0.0"
        );
    }

    #[test]
    fn pages_are_https_only_and_built_ins_are_known() {
        let agents = parsed(None);
        // The http:// website is dropped; Alpha has no other page.
        assert_eq!(agents[0].website, None);
        assert_eq!(agents[0].page_url(), None);
        assert_eq!(
            agents[1].page_url().as_deref(),
            Some("https://www.npmjs.com/package/@agentclientprotocol/claude-agent-acp")
        );
        assert_eq!(
            agents[2].page_url().as_deref(),
            Some("https://github.com/example/zeta")
        );
        assert_eq!(agents[1].builtin_profile(), Some(BUILTIN_CLAUDE_ID));
        assert_eq!(agents[2].builtin_profile(), None);
    }

    #[test]
    fn search_matches_name_id_and_description_in_any_case() {
        let agents = parsed(None);
        assert!(agents[1].matches("claude CODE"));
        assert!(agents[2].matches("zeta-acp"));
        assert!(agents[2].matches("  "));
        assert!(!agents[2].matches("claude"));
    }
}
