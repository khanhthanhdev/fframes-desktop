//! A project's own choices on top of the settings for all projects, as Oleafly's skills
//! scope: which of the user's servers run, which of their tools and which project tools are
//! on. Kept in Studio's app data beside the project's agent sessions
//! (`projects/<project id>/agent/tool-settings.json`), never in the project's files.
//!
//! An override names only what differs: a choice equal to the setting for all projects is
//! dropped, so the project keeps following that setting. The server definitions themselves
//! (program, URL, variables) are the same in every project.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use studio_project::ProjectId;

use super::AgentToolSettings;
use crate::app_paths::AppPaths;

/// The file of a project's override, in its agent directory.
pub const OVERRIDE_FILE: &str = "tool-settings.json";

/// Where `project`'s override is kept.
pub fn project_override_path(paths: &AppPaths, project: &ProjectId) -> PathBuf {
    paths.agent(project).join(OVERRIDE_FILE)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectToolOverride {
    /// Server name → on or off in this project.
    pub servers: BTreeMap<String, bool>,
    /// Project tool → on or off in this project.
    pub tools: BTreeMap<String, bool>,
    /// Server name → its tool → on or off in this project.
    pub server_tools: BTreeMap<String, BTreeMap<String, bool>>,
}

impl ProjectToolOverride {
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty() && self.tools.is_empty() && self.server_tools.is_empty()
    }

    /// The settings this project's agent gets: `global` with this override on top.
    pub fn apply(&self, global: &AgentToolSettings) -> AgentToolSettings {
        let mut effective = global.clone();
        for server in &mut effective.mcp_servers {
            if let Some(on) = self.servers.get(&server.name) {
                server.enabled = *on;
            }
            for (tool, on) in self.server_tools.get(&server.name).into_iter().flatten() {
                server.set_tool(tool, *on);
            }
        }
        for (tool, on) in &self.tools {
            effective.set_tool(tool, *on);
        }
        effective
    }

    /// Whether this project changes anything about the server `name`.
    pub fn changes_server(&self, name: &str) -> bool {
        self.servers.contains_key(name) || self.server_tools.contains_key(name)
    }

    /// Whether this project changes any project tool.
    pub fn changes_tools(&self) -> bool {
        !self.tools.is_empty()
    }

    /// Follow the setting for all projects for the server `name` again.
    pub fn reset_server(&mut self, name: &str) {
        self.servers.remove(name);
        self.server_tools.remove(name);
    }

    /// Follow the settings for all projects for every project tool again.
    pub fn reset_tools(&mut self) {
        self.tools.clear();
    }

    /// Turns the server `name` on or off in this project.
    pub fn set_server(&mut self, global: &AgentToolSettings, name: &str, enabled: bool) {
        let everywhere = global
            .mcp_servers
            .iter()
            .find(|server| server.name == name)
            .map(|server| server.enabled);
        match everywhere {
            Some(on) if on != enabled => {
                self.servers.insert(name.to_owned(), enabled);
            }
            _ => {
                self.servers.remove(name);
            }
        }
    }

    /// Turns a project tool on or off in this project.
    pub fn set_tool(&mut self, global: &AgentToolSettings, tool: &str, enabled: bool) {
        let everywhere = !global.disabled_tools.iter().any(|off| off == tool);
        if everywhere == enabled {
            self.tools.remove(tool);
        } else {
            self.tools.insert(tool.to_owned(), enabled);
        }
    }

    /// Turns the tool `tool` of the server `server` on or off in this project.
    pub fn set_server_tool(
        &mut self,
        global: &AgentToolSettings,
        server: &str,
        tool: &str,
        enabled: bool,
    ) {
        let everywhere = global
            .mcp_servers
            .iter()
            .find(|each| each.name == server)
            .is_none_or(|each| !each.disabled_tools.iter().any(|off| off == tool));
        let tools = self.server_tools.entry(server.to_owned()).or_default();
        if everywhere == enabled {
            tools.remove(tool);
        } else {
            tools.insert(tool.to_owned(), enabled);
        }
        if tools.is_empty() {
            self.server_tools.remove(server);
        }
    }

    /// Drops what no longer applies: servers removed (or renamed) from the settings.
    pub fn retain_known(&mut self, global: &AgentToolSettings) {
        let known = |name: &String| global.mcp_servers.iter().any(|s| &s.name == name);
        self.servers.retain(|name, _| known(name));
        self.server_tools.retain(|name, _| known(name));
    }

    /// The saved override of `project`, or none when it has none or it cannot be read.
    pub fn load(paths: &AppPaths, project: &ProjectId) -> Self {
        std::fs::read_to_string(project_override_path(paths, project))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Saves the override of `project` through a temporary file renamed over the old one;
    /// an empty override removes the file.
    pub fn save(&self, paths: &AppPaths, project: &ProjectId) -> Result<(), String> {
        let path = project_override_path(paths, project);
        if self.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!(
                    "Cannot reset the project's agent settings: {error}"
                )),
                _ => Ok(()),
            };
        }
        let dir = paths.agent(project);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Cannot save the project's agent settings: {e}"))?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Cannot encode the project's agent settings: {e}"))?;
        let mut file = tempfile::NamedTempFile::new_in(&dir)
            .map_err(|e| format!("Cannot save the project's agent settings: {e}"))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.as_file().sync_all())
            .map_err(|e| format!("Cannot save the project's agent settings: {e}"))?;
        file.persist(&path)
            .map_err(|e| format!("Cannot save the project's agent settings: {}", e.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::UserMcpServer;
    use super::*;

    fn global() -> AgentToolSettings {
        let server = |name: &str, enabled: bool, off: &[&str]| UserMcpServer {
            enabled,
            disabled_tools: off.iter().map(|tool| (*tool).to_owned()).collect(),
            ..UserMcpServer::stdio(name, "/bin/server", vec![], vec![])
        };
        AgentToolSettings {
            mcp_servers: vec![
                server("github", true, &["push_files"]),
                server("sqlite", false, &[]),
            ],
            disabled_tools: vec!["render_strip".into()],
        }
    }

    #[test]
    fn a_project_override_changes_only_what_it_names_on_top_of_the_global_settings() {
        let global = global();
        let mut project = ProjectToolOverride::default();
        assert_eq!(project.apply(&global), global);

        project.set_server(&global, "sqlite", true);
        project.set_server(&global, "github", true); // the same as everywhere: no override
        project.set_tool(&global, "render_strip", true);
        project.set_tool(&global, "timeline", false);
        project.set_server_tool(&global, "github", "push_files", true);
        project.set_server_tool(&global, "github", "create_issue", false);
        assert!(!project.servers.contains_key("github"));
        assert!(project.changes_server("github") && project.changes_server("sqlite"));

        let effective = project.apply(&global);
        assert!(effective.mcp_servers[1].enabled, "sqlite is on here");
        assert!(effective.mcp_servers[0].enabled);
        assert_eq!(
            effective.mcp_servers[0].disabled_tools,
            vec!["create_issue".to_owned()]
        );
        assert_eq!(effective.disabled_tools, vec!["timeline".to_owned()]);
        assert_eq!(global.disabled_tools, vec!["render_strip".to_owned()]);

        // Choosing the global value again drops the entry.
        project.set_server_tool(&global, "github", "create_issue", true);
        project.set_server_tool(&global, "github", "push_files", false);
        assert!(!project.server_tools.contains_key("github"));
        project.reset_server("sqlite");
        project.reset_tools();
        assert!(project.is_empty());

        project.set_server(&global, "sqlite", true);
        let renamed = AgentToolSettings {
            mcp_servers: global.mcp_servers[..1].to_vec(),
            ..global.clone()
        };
        project.retain_known(&renamed);
        assert!(project.is_empty());
    }

    #[test]
    fn overrides_are_kept_per_project_in_app_data_and_an_empty_one_removes_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        let one = ProjectId::try_from("project-one".to_owned()).unwrap();
        let two = ProjectId::try_from("project-two".to_owned()).unwrap();
        let global = global();
        let mut project = ProjectToolOverride::default();
        project.set_server(&global, "sqlite", true);
        project.save(&paths, &one).unwrap();
        assert_eq!(
            project_override_path(&paths, &one),
            paths.agent(&one).join("tool-settings.json")
        );
        assert_eq!(ProjectToolOverride::load(&paths, &one), project);
        assert!(ProjectToolOverride::load(&paths, &two).is_empty());

        ProjectToolOverride::default().save(&paths, &one).unwrap();
        assert!(!project_override_path(&paths, &one).exists());
        ProjectToolOverride::default().save(&paths, &two).unwrap();
    }
}
