//! Settings → Agent → MCP servers and Tools, after Oleafly's MCP manager, import dialog,
//! registry browser and tool list.
//!
//! MCP servers: the user's own servers as cards (name, program or URL, the variables it
//! reads, an on/off switch, a status, Test, Edit and Remove, and once tested the server's
//! tools with a switch each), a form to add or edit one (a stdio program or a streamable
//! HTTP URL), an import panel for the servers Claude Code, Claude Desktop and Codex already
//! use, and a search of the official MCP registry that fills in the form. Tools: one switch
//! per project tool of Studio's own server.
//!
//! With a project open, both views have an "All projects / This project" switch, as
//! Oleafly's skills: in This project the switches change an override kept for that project
//! in Studio's app data, never in the project's files. The agent of the open project gets
//! the settings with that override on top ([`AgentToolsPage::effective`]). Every change is
//! announced with [`AgentToolsChanged`]; the shell saves the settings and the open project's
//! override and hands the effective settings to the project's agent.

use std::collections::{BTreeMap, HashMap, HashSet};

use gpui::{
    AnyElement, AppContext, ClipboardItem, Context, ElementId, Entity, EventEmitter,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_engine::agent_settings::{
    AgentToolSettings, FoundServer, OnDuplicate, PROBE_TIMEOUT, ProbeFailure, ProbeFailureKind,
    ProjectToolOverride, RegistryEntry, STUDIO_SERVER_NAME, ServerTransport, UserMcpServer,
    find_servers, parse_registry, probe, resolve_command, search_registry,
};
use studio_engine::agent_tools::TOOL_NAMES;
use studio_engine::app_paths::AppPaths;
use studio_project::ProjectId;
use studio_theme::{Appearance, METRICS, MONO_FAMILY, TextStyle, Theme, text_style};

use crate::agent::composer_tools::tool_copy;
use crate::kit::{
    Badge, Button, ButtonSize, Emphasis, EmptyState, FormField, Icon, IconButton, IconButtonSize,
    Notice, PanelSection, SegmentOption, SegmentedControl, SettingRow, Spinner, TextInput,
    TextInputState, Toggle, Tone, Tooltip, focus_ring, reveal_probe, setting_card, setting_note,
};

/// Commands and URLs longer than this are shortened in the middle on a card.
const COMMAND_CHARS: usize = 80;
/// How long a linked server's card stays highlighted.
const HIGHLIGHT: std::time::Duration = std::time::Duration::from_millis(2500);

/// The two views this page draws, one per Agent tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ToolsView {
    #[default]
    Mcp,
    Tools,
}

/// What the switches change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Scope {
    /// The settings for all projects.
    #[default]
    All,
    /// The open project's override.
    Project,
}

/// The gallery's sample states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fixture {
    /// Sample servers in every state, and sample tool choices.
    Servers,
    /// The import panel with a server already added.
    Import,
    /// The registry browser with results.
    Registry,
    /// The registry browser while a search runs.
    RegistryLoading,
    /// The registry browser after a search failed.
    RegistryFailed,
    /// The sample servers after a link to one of them (linear), which is highlighted.
    Linked,
    /// The servers with a project open in This project.
    Project,
}

/// The settings changed; the shell saves them (and the open project's override) and
/// applies [`AgentToolsPage::effective`] to the next task.
pub(super) struct AgentToolsChanged(pub AgentToolSettings);

/// The last connection test of one server.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Probe {
    Checking,
    Ready(Vec<String>),
    Failed(ProbeFailure),
}

/// The add/edit form.
struct Editor {
    /// The index of the server being edited; `None` adds one.
    replacing: Option<usize>,
    /// A streamable-HTTP server rather than a program.
    http: bool,
    name: Entity<TextInputState>,
    command: Entity<TextInputState>,
    args: Entity<TextInputState>,
    env: Entity<TextInputState>,
    url: Entity<TextInputState>,
    bearer: Entity<TextInputState>,
    headers: Entity<TextInputState>,
    problem: Option<String>,
}

/// One server found in another tool's settings, and what to do with it.
#[derive(Debug, Clone)]
struct ImportRow {
    found: FoundServer,
    picked: bool,
    on_duplicate: OnDuplicate,
}

enum Import {
    Closed,
    Searching,
    Found(Vec<ImportRow>),
}

enum RegistryState {
    Idle,
    Loading,
    Failed(String),
    Found(Vec<RegistryEntry>),
}

struct RegistryBrowser {
    query: Entity<TextInputState>,
    state: RegistryState,
    /// Bumped by every search, so an older answer that comes late is dropped.
    generation: u64,
}

/// The open project and its override.
struct ProjectScope {
    id: ProjectId,
    name: String,
    overrides: ProjectToolOverride,
}

pub(super) struct AgentToolsPage {
    theme: Theme,
    pub(super) view: ToolsView,
    /// The settings for all projects.
    settings: AgentToolSettings,
    project: Option<ProjectScope>,
    scope: Scope,
    probes: HashMap<String, Probe>,
    /// Servers whose tool list is open.
    expanded: HashSet<String>,
    editor: Option<Editor>,
    import: Import,
    registry: Option<RegistryBrowser>,
    /// One message for the whole page, Oleafly's shared strip: the last outcome.
    message: Option<(Tone, String)>,
    /// The gallery's environment and project, in place of Studio's.
    fixture_env: Option<HashMap<String, String>>,
    /// The settings page's scrolling, which a link to a server moves.
    scroll: ScrollHandle,
    /// The server a link pointed at: its card wears the focus ring for [`HIGHLIGHT`].
    highlighted: Option<String>,
    /// The highlighted card still has to be scrolled into view.
    reveal: bool,
    /// Bumped by every link, so an older highlight's timer leaves a newer one alone.
    highlight_generation: u64,
}

impl EventEmitter<AgentToolsChanged> for AgentToolsPage {}

/// Splits an argument line like a shell would for plain words and double quotes.
fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

/// The arguments as one editable line: words with spaces are quoted.
fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Variable names typed with commas or spaces between them.
fn split_names(line: &str) -> Vec<String> {
    line.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `Header=VARIABLE` pairs typed with commas between them.
fn split_headers(line: &str) -> Result<BTreeMap<String, String>, String> {
    line.split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((header, name)) if !header.trim().is_empty() && !name.trim().is_empty() => {
                Ok((header.trim().to_owned(), name.trim().to_owned()))
            }
            _ => Err("Write each header as Header=VARIABLE, separated by commas.".to_owned()),
        })
        .collect()
}

/// The headers as one editable line.
fn join_headers(headers: &BTreeMap<String, String>) -> String {
    headers
        .iter()
        .map(|(header, name)| format!("{header}={name}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `server`'s command at most about `max` characters long. The program's folder goes
/// first, since the program's name and the arguments say what the server is; then the
/// middle.
fn short_command(server: &UserMcpServer, max: usize) -> String {
    let full = server.summary();
    if full.chars().count() <= max {
        return full;
    }
    let shorter = match &server.transport {
        ServerTransport::Stdio { command, .. } => {
            let program = command.display().to_string();
            match (full.strip_prefix(&program), command.file_name()) {
                (Some(rest), Some(name))
                    if command
                        .parent()
                        .is_some_and(|dir| dir != std::path::Path::new("")) =>
                {
                    format!(
                        "…{}{}{rest}",
                        std::path::MAIN_SEPARATOR,
                        name.to_string_lossy()
                    )
                }
                _ => full,
            }
        }
        ServerTransport::Http { .. } => full,
    };
    shorten_middle(&shorter, max)
}

/// `text` at most `max` characters long, shortened in the middle: the start of a command
/// (its program) and its end (the last argument) stay.
fn shorten_middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max || max < 3 {
        return text.to_owned();
    }
    let head = (max - 1) * 3 / 5;
    let tail = max - 1 - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{} … {}", start.trim_end(), end.trim_start())
}

/// "A", "A and B", "A, B and C".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl AgentToolsPage {
    pub(super) fn new(settings: AgentToolSettings, scroll: ScrollHandle) -> Self {
        Self {
            theme: Theme::new(Appearance::Light),
            view: ToolsView::default(),
            settings,
            project: None,
            scope: Scope::default(),
            probes: HashMap::new(),
            expanded: HashSet::new(),
            editor: None,
            import: Import::Closed,
            registry: None,
            message: None,
            fixture_env: None,
            scroll,
            highlighted: None,
            reveal: false,
            highlight_generation: 0,
        }
    }

    /// Shows the server named `name`, a link's target: scrolls its card into view and
    /// highlights it for a moment. A name with no server changes nothing.
    pub(super) fn show_server(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self
            .settings
            .mcp_servers
            .iter()
            .any(|server| server.name == name)
        {
            return;
        }
        self.highlighted = Some(name.to_owned());
        self.reveal = true;
        self.highlight_generation += 1;
        let generation = self.highlight_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIGHLIGHT).await;
            let _ = this.update(cx, |page, cx| {
                if page.highlight_generation == generation {
                    page.highlighted = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The server a link highlights now.
    #[cfg(test)]
    pub(super) fn highlighted(&self) -> Option<&str> {
        self.highlighted.as_deref()
    }

    /// What the open project's agent gets: the settings for all projects with the
    /// project's override on top.
    pub(super) fn effective(&self) -> AgentToolSettings {
        match &self.project {
            Some(project) => project.overrides.apply(&self.settings),
            None => self.settings.clone(),
        }
    }

    /// The open project and its override, for the shell to save.
    pub(super) fn project_override(&self) -> Option<(ProjectId, ProjectToolOverride)> {
        self.project
            .as_ref()
            .map(|project| (project.id.clone(), project.overrides.clone()))
    }

    /// Applies the composer's Tools popover, which shows [`Self::effective`]: with a project
    /// open the differences go to its override, so the switch the user flipped is what its
    /// agent gets even when the override already changed that tool or server.
    pub(super) fn set_from_composer(&mut self, wanted: &AgentToolSettings, cx: &mut Context<Self>) {
        let shown = self.effective();
        let global = &mut self.settings;
        let mut project = self.project.as_mut().map(|project| &mut project.overrides);
        for tool in studio_engine::agent_tools::TOOL_NAMES {
            let enabled = wanted.enabled_tools().contains(&tool);
            if shown.enabled_tools().contains(&tool) != enabled {
                match project.as_deref_mut() {
                    Some(overrides) => overrides.set_tool(global, tool, enabled),
                    None => global.set_tool(tool, enabled),
                }
            }
        }
        for server in &wanted.mcp_servers {
            let was = shown.mcp_servers.iter().find(|s| s.name == server.name);
            if was.is_none_or(|was| was.enabled == server.enabled) {
                continue;
            }
            match project.as_deref_mut() {
                Some(overrides) => overrides.set_server(global, &server.name, server.enabled),
                None => {
                    if let Some(each) = global
                        .mcp_servers
                        .iter_mut()
                        .find(|s| s.name == server.name)
                    {
                        each.enabled = server.enabled;
                    }
                }
            }
        }
        self.changed(cx);
    }

    /// Replaces the settings for all projects without announcing them (they were just
    /// loaded).
    pub(super) fn load(&mut self, settings: AgentToolSettings, cx: &mut Context<Self>) {
        self.settings = settings;
        cx.notify();
    }

    /// Follows the open project (`None` when none is): its override is read from `paths`
    /// when the project changes. The gallery's sample project stays.
    pub(super) fn follow_project(
        &mut self,
        project: Option<(ProjectId, String)>,
        paths: Option<&AppPaths>,
    ) {
        if self.fixture_env.is_some() {
            return;
        }
        match project {
            None => {
                self.project = None;
                self.scope = Scope::All;
            }
            Some((id, name)) => {
                if self
                    .project
                    .as_ref()
                    .is_some_and(|project| project.id == id)
                {
                    return;
                }
                let overrides = paths
                    .map(|paths| ProjectToolOverride::load(paths, &id))
                    .unwrap_or_default();
                self.project = Some(ProjectScope {
                    id,
                    name,
                    overrides,
                });
                self.scope = Scope::All;
            }
        }
    }

    /// The gallery's samples.
    pub(super) fn fixture(&mut self, which: Fixture, window: &mut Window, cx: &mut Context<Self>) {
        let stdio = |name: &str, command: &str, args: &[&str], env: &[&str]| {
            UserMcpServer::stdio(
                name,
                command,
                args.iter().map(|arg| (*arg).to_owned()).collect(),
                env.iter().map(|name| (*name).to_owned()).collect(),
            )
        };
        let npx = r"C:\Program Files\nodejs\npx.cmd";
        let filesystem = UserMcpServer {
            disabled_tools: vec!["write_file".into(), "move_file".into()],
            ..stdio(
                "filesystem",
                npx,
                &[
                    "-y",
                    "@modelcontextprotocol/server-filesystem",
                    r"C:\Users\Administrator\Documents\Studio Projects\launch-video\assets",
                ],
                &[],
            )
        };
        self.settings = AgentToolSettings {
            mcp_servers: vec![
                stdio(
                    "github",
                    npx,
                    &["-y", "@modelcontextprotocol/server-github"],
                    &["GITHUB_PERSONAL_ACCESS_TOKEN"],
                ),
                stdio("context7", npx, &["-y", "@upstash/context7-mcp"], &[]),
                filesystem,
                UserMcpServer::http(
                    "linear",
                    "https://mcp.linear.app/mcp",
                    Some("LINEAR_API_KEY".into()),
                    BTreeMap::new(),
                ),
                UserMcpServer {
                    enabled: false,
                    ..stdio("sqlite", r"C:\Tools\uvx.exe", &["mcp-server-sqlite"], &[])
                },
            ],
            disabled_tools: vec!["render_strip".into(), "source_lookup".into()],
        };
        let names = |tools: &[&str]| tools.iter().map(|tool| (*tool).to_owned()).collect();
        self.probes = HashMap::from([
            (
                "context7".to_owned(),
                Probe::Failed(ProbeFailure::new(
                    ProbeFailureKind::TimedOut,
                    "It did not answer within 20 seconds.",
                )),
            ),
            (
                "filesystem".to_owned(),
                Probe::Ready(names(&[
                    "read_file",
                    "write_file",
                    "list_directory",
                    "search_files",
                    "move_file",
                ])),
            ),
            (
                "linear".to_owned(),
                Probe::Ready(names(&[
                    "list_issues",
                    "get_issue",
                    "create_issue",
                    "update_issue",
                    "list_projects",
                    "search_documentation",
                ])),
            ),
        ]);
        self.expanded = HashSet::from(["filesystem".to_owned()]);
        self.fixture_env = Some(HashMap::from([(
            "LINEAR_API_KEY".to_owned(),
            "lin_api_sample".to_owned(),
        )]));
        self.editor = None;
        self.import = Import::Closed;
        self.registry = None;
        self.message = None;
        self.project = None;
        self.scope = Scope::All;
        self.highlighted = None;
        self.reveal = false;
        match which {
            Fixture::Servers => {}
            // Without the timer: the gallery draws the fixture again on every frame.
            Fixture::Linked => {
                self.highlighted = Some("linear".into());
                self.reveal = true;
            }
            Fixture::RegistryLoading | Fixture::RegistryFailed => {
                let query = Self::input("github, postgres, docs…", window, cx);
                query.update(cx, |input, cx| input.set_text("papers", cx));
                self.registry = Some(RegistryBrowser {
                    query,
                    state: if which == Fixture::RegistryLoading {
                        RegistryState::Loading
                    } else {
                        RegistryState::Failed(
                            "Cannot reach the MCP registry: curl: (6) Could not resolve host: \
                             registry.modelcontextprotocol.io"
                                .into(),
                        )
                    },
                    generation: 0,
                });
            }
            Fixture::Import => {
                let found = |source, server: UserMcpServer, had_values| ImportRow {
                    picked: true,
                    on_duplicate: OnDuplicate::Skip,
                    found: FoundServer {
                        source,
                        server,
                        had_values,
                    },
                };
                self.import = Import::Found(vec![
                    ImportRow {
                        on_duplicate: OnDuplicate::Overwrite,
                        ..found(
                            "Claude Code",
                            stdio(
                                "github",
                                npx,
                                &["-y", "@modelcontextprotocol/server-github"],
                                &["GITHUB_PERSONAL_ACCESS_TOKEN"],
                            ),
                            true,
                        )
                    },
                    found(
                        "Claude Code",
                        UserMcpServer::http(
                            "notion",
                            "https://mcp.notion.com/mcp",
                            Some("NOTION_TOKEN".into()),
                            BTreeMap::new(),
                        ),
                        false,
                    ),
                    found(
                        "Claude Desktop",
                        stdio(
                            "postgres",
                            npx,
                            &["-y", "@modelcontextprotocol/server-postgres"],
                            &["DATABASE_URL"],
                        ),
                        true,
                    ),
                    ImportRow {
                        picked: false,
                        ..found(
                            "Codex",
                            stdio("context7", npx, &["-y", "@upstash/context7-mcp"], &[]),
                            false,
                        )
                    },
                ]);
            }
            Fixture::Registry => {
                let query = Self::input("github, postgres, docs…", window, cx);
                query.update(cx, |input, cx| input.set_text("papers", cx));
                self.registry = Some(RegistryBrowser {
                    query,
                    state: RegistryState::Found(
                        parse_registry(REGISTRY_FIXTURE.as_bytes()).unwrap_or_default(),
                    ),
                    generation: 0,
                });
            }
            Fixture::Project => {
                let mut overrides = ProjectToolOverride::default();
                overrides.set_server(&self.settings, "sqlite", true);
                overrides.set_server_tool(&self.settings, "filesystem", "write_file", true);
                overrides.set_tool(&self.settings, "render_strip", true);
                overrides.set_tool(&self.settings, "timeline", false);
                self.project = Some(ProjectScope {
                    id: ProjectId::try_from("launch-video".to_owned())
                        .expect("a valid sample project id"),
                    name: "Launch video".into(),
                    overrides,
                });
                self.scope = Scope::Project;
                self.probes.insert(
                    "sqlite".into(),
                    Probe::Ready(names(&["read_query", "write_query", "list_tables"])),
                );
            }
        }
    }

    fn input(
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInputState> {
        let placeholder = placeholder.to_owned();
        cx.new(|cx| {
            let mut input = TextInputState::single_line(window, cx);
            input.set_placeholder(placeholder);
            input
        })
    }

    fn env_value(&self, name: &str) -> Option<String> {
        match &self.fixture_env {
            Some(env) => env.get(name).cloned(),
            None => std::env::var(name).ok(),
        }
    }

    fn missing(&self, server: &UserMcpServer) -> Vec<String> {
        server.missing_variables(|name| self.env_value(name))
    }

    /// The settings the switches show: the open project's in This project.
    fn shown(&self) -> AgentToolSettings {
        match self.scope {
            Scope::All => self.settings.clone(),
            Scope::Project => self.effective(),
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = &mut self.project {
            project.overrides.retain_known(&self.settings);
        }
        cx.emit(AgentToolsChanged(self.settings.clone()));
        cx.notify();
    }

    fn say(&mut self, tone: Tone, text: impl Into<String>, cx: &mut Context<Self>) {
        self.message = Some((tone, text.into()));
        cx.notify();
    }

    /// Turns the server at `index` on or off, for every project or for this one.
    fn set_enabled(&mut self, index: usize, enabled: bool, cx: &mut Context<Self>) {
        let Some(name) = self.settings.mcp_servers.get(index).map(|s| s.name.clone()) else {
            return;
        };
        match (self.scope, &mut self.project) {
            (Scope::Project, Some(project)) => {
                project.overrides.set_server(&self.settings, &name, enabled);
            }
            _ => self.settings.mcp_servers[index].enabled = enabled,
        }
        self.changed(cx);
    }

    /// Turns one tool of the server `server` on or off, for every project or for this one.
    fn set_server_tool(&mut self, server: &str, tool: &str, enabled: bool, cx: &mut Context<Self>) {
        match (self.scope, &mut self.project) {
            (Scope::Project, Some(project)) => {
                project
                    .overrides
                    .set_server_tool(&self.settings, server, tool, enabled);
            }
            _ => self.settings.set_server_tool(server, tool, enabled),
        }
        self.changed(cx);
    }

    /// Turns project tools on or off, for every project or for this one.
    fn set_tools(&mut self, tools: &[&str], enabled: bool, cx: &mut Context<Self>) {
        for tool in tools {
            match (self.scope, &mut self.project) {
                (Scope::Project, Some(project)) => {
                    project.overrides.set_tool(&self.settings, tool, enabled);
                }
                _ => self.settings.set_tool(tool, enabled),
            }
        }
        self.changed(cx);
    }

    // ---- scope -------------------------------------------------------------------------------

    /// "All projects / This project", above both views while a project is open.
    fn render_scope(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let theme = self.theme;
        let entity = cx.entity();
        let note = match self.scope {
            Scope::All => {
                "Changes apply to every project that has no change of its own.".to_owned()
            }
            Scope::Project => format!(
                "Changes apply to {} only. They are kept in Studio, not in the project's files.",
                project.name
            ),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_2))
                .child(
                    // A row, so the control keeps its own width.
                    div().flex().child(
                        SegmentedControl::new(
                            &theme,
                            "agent-tools-scope",
                            vec![
                                SegmentOption::new("All projects", self.scope == Scope::All),
                                SegmentOption::new("This project", self.scope == Scope::Project),
                            ],
                        )
                        .on_select(move |index, _, app| {
                            entity.update(app, |page, cx| {
                                page.scope = if *index == 1 {
                                    Scope::Project
                                } else {
                                    Scope::All
                                };
                                cx.notify();
                            })
                        }),
                    ),
                )
                .child(self.muted(note))
                .into_any_element(),
        )
    }

    /// "Changed for this project" and Reset, when the override changes `what`.
    fn project_change(
        &self,
        id: impl Into<ElementId>,
        reset: impl Fn(&mut ProjectToolOverride) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_1))
            .child(Badge::new("Changed for this project").tone(Tone::Info))
            .child(
                Button::new(&self.theme, id)
                    .label("Reset")
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .on_click(move |_, _, app| {
                        entity.update(app, |page, cx| {
                            if let Some(project) = &mut page.project {
                                reset(&mut project.overrides);
                            }
                            page.changed(cx);
                        })
                    }),
            )
            .into_any_element()
    }

    // ---- MCP servers -------------------------------------------------------------------------

    fn open_editor(
        &mut self,
        replacing: Option<usize>,
        draft: Option<UserMcpServer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let server = draft
            .or_else(|| replacing.and_then(|index| self.settings.mcp_servers.get(index).cloned()));
        let filled =
            |text: String, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let input = Self::input(placeholder, window, cx);
                input.update(cx, |input, cx| input.set_text(text, cx));
                input
            };
        let mut texts: [String; 7] = Default::default();
        let mut http = false;
        if let Some(server) = &server {
            texts[0] = server.name.clone();
            match &server.transport {
                ServerTransport::Stdio { command, args, env } => {
                    let command = command.display().to_string();
                    texts[1] = resolve_command(&command)
                        .map(|path| path.display().to_string())
                        .unwrap_or(command);
                    texts[2] = join_args(args);
                    texts[3] = env.join(", ");
                }
                ServerTransport::Http {
                    url,
                    bearer_env,
                    header_env,
                } => {
                    http = true;
                    texts[4] = url.clone();
                    texts[5] = bearer_env.clone().unwrap_or_default();
                    texts[6] = join_headers(header_env);
                }
            }
        }
        let [name, command, args, env, url, bearer, headers] = texts;
        let editor = Editor {
            replacing,
            http,
            name: filled(name, "github", window, cx),
            command: filled(command, "npx, or the full path of the program", window, cx),
            args: filled(args, "-y @modelcontextprotocol/server-github", window, cx),
            env: filled(env, "GITHUB_PERSONAL_ACCESS_TOKEN", window, cx),
            url: filled(url, "https://example.com/mcp", window, cx),
            bearer: filled(bearer, "EXAMPLE_TOKEN", window, cx),
            headers: filled(headers, "X-Team-Id=EXAMPLE_TEAM", window, cx),
            problem: None,
        };
        editor
            .name
            .update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.editor = Some(editor);
        self.import = Import::Closed;
        self.registry = None;
        cx.notify();
    }

    /// The form's server, the command resolved to a full path, and the servers it must not
    /// clash with; or why the form cannot make one.
    fn edited(
        &self,
        cx: &Context<Self>,
    ) -> Option<Result<(UserMcpServer, Vec<UserMcpServer>), String>> {
        let editor = self.editor.as_ref()?;
        let text = |input: &Entity<TextInputState>| input.read(cx).text().trim().to_owned();
        let kept = editor
            .replacing
            .and_then(|index| self.settings.mcp_servers.get(index));
        let name = text(&editor.name);
        let mut server = if editor.http {
            let headers = match split_headers(&text(&editor.headers)) {
                Ok(headers) => headers,
                Err(problem) => return Some(Err(problem)),
            };
            let bearer = text(&editor.bearer);
            UserMcpServer::http(
                name,
                text(&editor.url),
                (!bearer.is_empty()).then_some(bearer),
                headers,
            )
        } else {
            let command = text(&editor.command);
            UserMcpServer::stdio(
                name,
                resolve_command(&command).unwrap_or_else(|| command.into()),
                split_args(&text(&editor.args)),
                split_names(&text(&editor.env)),
            )
        };
        if let Some(kept) = kept {
            server.enabled = kept.enabled;
            server.disabled_tools = kept.disabled_tools.clone();
        }
        let others = self
            .settings
            .mcp_servers
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != editor.replacing)
            .map(|(_, server)| server.clone())
            .collect();
        Some(Ok((server, others)))
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(edited) = self.edited(cx) else {
            return;
        };
        let problem = match &edited {
            Ok((server, others)) => server.problem(others),
            Err(problem) => Some(problem.clone()),
        };
        if let Some(problem) = problem {
            if let Some(editor) = &mut self.editor {
                editor.problem = Some(problem);
            }
            cx.notify();
            return;
        }
        let Ok((server, _)) = edited else {
            return;
        };
        let replacing = self.editor.take().and_then(|editor| editor.replacing);
        let name = server.name.clone();
        match replacing.and_then(|index| self.settings.mcp_servers.get_mut(index)) {
            Some(slot) => {
                self.probes.remove(&slot.name);
                self.expanded.remove(&slot.name);
                *slot = server;
            }
            None => self.settings.mcp_servers.push(server),
        }
        self.message = Some((
            Tone::Success,
            format!("Saved `{name}`. It starts with the next agent task."),
        ));
        self.changed(cx);
        self.test_server(&name, cx);
    }

    fn remove_server(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.settings.mcp_servers.len() {
            return;
        }
        let server = self.settings.mcp_servers.remove(index);
        self.probes.remove(&server.name);
        self.expanded.remove(&server.name);
        self.message = Some((Tone::Neutral, format!("Removed `{}`.", server.name)));
        self.changed(cx);
    }

    /// Starts or calls the saved server `name` as an agent would and lists its tools, off
    /// the UI thread.
    fn test_server(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(server) = self.settings.mcp_servers.iter().find(|s| s.name == name) else {
            return;
        };
        if !self.missing(server).is_empty() {
            // The card says which variable is missing; nothing starts without it.
            self.probes.remove(name);
            cx.notify();
            return;
        }
        let launch = match server.launch(|name| self.env_value(name)) {
            Ok(launch) => launch,
            Err(reason) => {
                self.probes.insert(
                    name.to_owned(),
                    Probe::Failed(ProbeFailure::new(ProbeFailureKind::NotStarted, reason)),
                );
                cx.notify();
                return;
            }
        };
        let name = name.to_owned();
        self.probes.insert(name.clone(), Probe::Checking);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { probe(&launch.transport, PROBE_TIMEOUT) })
                .await;
            let _ = this.update(cx, |page, cx| {
                // A server removed or renamed meanwhile keeps no stale result.
                if page.probes.get(&name) == Some(&Probe::Checking) {
                    page.probes.insert(
                        name,
                        match result {
                            Ok(tools) => Probe::Ready(tools),
                            Err(failure) => Probe::Failed(failure),
                        },
                    );
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- import --------------------------------------------------------------------------------

    fn search_imports(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.registry = None;
        self.import = Import::Searching;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                        .map(std::path::PathBuf::from);
                    let app_data = std::env::var_os("APPDATA").map(std::path::PathBuf::from);
                    home.map(|home| find_servers(&home, app_data.as_deref()))
                        .unwrap_or_default()
                })
                .await;
            let _ = this.update(cx, |page, cx| {
                if matches!(page.import, Import::Searching) {
                    let rows = found
                        .into_iter()
                        .map(|found| {
                            let taken = page.taken(&found.server.name);
                            ImportRow {
                                picked: !taken,
                                on_duplicate: OnDuplicate::Skip,
                                found,
                            }
                        })
                        .collect();
                    page.import = Import::Found(rows);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether a server is already named `name`.
    fn taken(&self, name: &str) -> bool {
        self.settings
            .mcp_servers
            .iter()
            .any(|server| server.name.eq_ignore_ascii_case(name))
    }

    fn import_selected(&mut self, cx: &mut Context<Self>) {
        let Import::Found(rows) = std::mem::replace(&mut self.import, Import::Closed) else {
            return;
        };
        let picked: Vec<ImportRow> = rows.into_iter().filter(|row| row.picked).collect();
        let kept_values: Vec<&'static str> = picked
            .iter()
            .filter(|row| row.found.had_values)
            .map(|row| row.found.source)
            .collect();
        let report = self.settings.import(
            picked
                .iter()
                .map(|row| (row.found.server.clone(), row.on_duplicate)),
        );
        for name in report.replaced.iter() {
            self.probes.remove(name);
            self.expanded.remove(name);
        }
        let mut parts = Vec::new();
        let count = |n: usize| {
            if n == 1 {
                "1 server".to_owned()
            } else {
                format!("{n} servers")
            }
        };
        if !report.added.is_empty() || !report.replaced.is_empty() {
            parts.push(format!(
                "Imported {}.",
                count(report.added.len() + report.replaced.len())
            ));
        }
        if !report.skipped.is_empty() {
            parts.push(format!("Skipped {}.", and_list(&report.skipped)));
        }
        for (name, problem) in &report.refused {
            parts.push(format!("`{name}` was not added: {problem}"));
        }
        if !kept_values.is_empty() {
            parts.push(
                "Variable values stay in the other tools; set them in Studio's environment too."
                    .into(),
            );
        }
        let tone = if report.refused.is_empty() {
            Tone::Success
        } else {
            Tone::Warning
        };
        let tested: Vec<String> = report
            .added
            .iter()
            .chain(&report.replaced)
            .cloned()
            .collect();
        if parts.is_empty() {
            parts.push("Nothing was imported.".into());
        }
        self.message = Some((tone, parts.join(" ")));
        self.changed(cx);
        for name in tested {
            self.test_server(&name, cx);
        }
    }

    // ---- registry ------------------------------------------------------------------------------

    fn open_registry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = None;
        self.import = Import::Closed;
        let query = Self::input("github, postgres, docs…", window, cx);
        query.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.registry = Some(RegistryBrowser {
            query,
            state: RegistryState::Idle,
            generation: 0,
        });
        cx.notify();
    }

    fn search_registry(&mut self, cx: &mut Context<Self>) {
        let Some(registry) = &mut self.registry else {
            return;
        };
        let query = registry.query.read(cx).text().trim().to_owned();
        if query.is_empty() {
            return;
        }
        registry.generation += 1;
        registry.state = RegistryState::Loading;
        let generation = registry.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { search_registry(&query) })
                .await;
            let _ = this.update(cx, |page, cx| {
                if let Some(registry) = &mut page.registry
                    && registry.generation == generation
                {
                    registry.state = match result {
                        Ok(entries) => RegistryState::Found(entries),
                        Err(error) => RegistryState::Failed(error),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_mcp(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let panel_open = self.editor.is_some()
            || !matches!(self.import, Import::Closed)
            || self.registry.is_some();
        let actions = div()
            .flex()
            .gap(px(METRICS.space_2))
            .child(
                Button::new(&theme, "mcp-registry")
                    .label("Browse registry")
                    .left_icon(Icon::Search.path())
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .disabled(self.registry.is_some())
                    .on_click({
                        let entity = entity.clone();
                        move |_, window, app| {
                            entity.update(app, |page, cx| page.open_registry(window, cx))
                        }
                    }),
            )
            .child(
                Button::new(&theme, "mcp-import")
                    .label("Import")
                    .left_icon(Icon::Download.path())
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .disabled(!matches!(self.import, Import::Closed))
                    .on_click({
                        let entity = entity.clone();
                        move |_, _, app| entity.update(app, |page, cx| page.search_imports(cx))
                    }),
            )
            .child(
                Button::new(&theme, "mcp-add")
                    .label("Add server")
                    .left_icon(Icon::Plus.path())
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Neutral)
                    .disabled(self.editor.is_some())
                    .on_click({
                        let entity = entity.clone();
                        move |_, window, app| {
                            entity.update(app, |page, cx| page.open_editor(None, None, window, cx))
                        }
                    }),
            );
        let mut children: Vec<AnyElement> = Vec::new();
        if let Some((tone, text)) = &self.message {
            children.push(
                Notice::new("mcp-message", *tone, text.clone())
                    .on_dismiss({
                        let entity = entity.clone();
                        move |_, app| {
                            entity.update(app, |page, cx| {
                                page.message = None;
                                cx.notify();
                            })
                        }
                    })
                    .into_any_element(),
            );
        }
        if self.editor.is_some() {
            children.push(self.render_editor(cx));
        }
        if self.registry.is_some() {
            children.push(self.render_registry(cx));
        }
        match &self.import {
            Import::Closed => {}
            Import::Searching => children.push(
                setting_card(&theme)
                    .child(self.muted("Looking in Claude Code, Claude Desktop and Codex…"))
                    .into_any_element(),
            ),
            Import::Found(rows) => children.push(self.render_import(rows.clone(), cx)),
        }
        if self.settings.mcp_servers.is_empty() && !panel_open {
            children.push(
                EmptyState::new(&theme, "No MCP servers yet")
                    .body(
                        "Add a server, browse the MCP registry, or import the ones Claude Code, \
                         Claude Desktop or Codex already use.",
                    )
                    .into_any_element(),
            );
        }
        let shown = self.shown();
        for index in 0..self.settings.mcp_servers.len() {
            children.push(self.render_server(index, &shown, cx));
        }
        children.push(
            setting_note(
                &theme,
                "Servers start with each agent task, beside Studio's own server. Claude and \
                 Codex ask before every call to their tools; ACP agents follow their own \
                 approval rules. Secrets stay in your environment: list the names of the \
                 variables a server needs and Studio passes their values on when a task starts.",
            )
            .into_any_element(),
        );
        let builtin = SettingRow::new(&theme, STUDIO_SERVER_NAME)
            .icon(Icon::Hammer)
            .description(format!(
                "Built in. Gives the agent {} of the {} project tools; choose them under Tools.",
                shown.enabled_tools().len(),
                TOOL_NAMES.len()
            ))
            .control(Badge::new("Always on").tone(Tone::Info));
        let mut column = div().flex().flex_col().gap(px(METRICS.space_6));
        if let Some(scope) = self.render_scope(cx) {
            column = column.child(scope);
        }
        column
            .child(
                PanelSection::new(&theme, "Your MCP servers")
                    .description(
                        "Programs or URLs that give the agent more tools, such as a GitHub, \
                         database or docs server.",
                    )
                    .trailing(actions)
                    .children(children),
            )
            .child(PanelSection::new(&theme, "Studio's server").children([builtin]))
            .into_any_element()
    }

    fn muted(&self, text: impl Into<SharedString>) -> gpui::Div {
        text_style(div(), TextStyle::Small)
            .text_color(self.theme.colors.text_2)
            .child(text.into())
    }

    fn mono(&self, text: impl Into<SharedString>) -> gpui::Div {
        text_style(div(), TextStyle::Small)
            .font_family(MONO_FAMILY)
            .text_color(self.theme.colors.text_1)
            .child(text.into())
    }

    /// A command or URL on one line, shortened in the middle when long: the whole of it in
    /// the tooltip and the Copy button beside it.
    fn command_line(
        &self,
        id: impl Into<ElementId>,
        copy_id: impl Into<ElementId>,
        server: &UserMcpServer,
    ) -> AnyElement {
        let theme = self.theme;
        let full = server.summary();
        let short = short_command(server, COMMAND_CHARS);
        let tip = SharedString::from(full.clone());
        div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_1))
            .min_w_0()
            .child(
                self.mono(short)
                    .id(id)
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .tooltip(move |_, cx| Tooltip::new(&theme, tip.clone()).view(cx)),
            )
            .child(
                IconButton::new(&theme, copy_id, Icon::Copy.path())
                    .size(IconButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .label("Copy")
                    .tooltip("Copy")
                    .on_click(move |_, _, app| {
                        app.write_to_clipboard(ClipboardItem::new_string(full.clone()))
                    }),
            )
            .into_any_element()
    }

    fn render_server(
        &self,
        index: usize,
        shown: &AgentToolSettings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let colors = theme.colors;
        let entity = cx.entity();
        let server = &self.settings.mcp_servers[index];
        let enabled = shown
            .mcp_servers
            .get(index)
            .map_or(server.enabled, |s| s.enabled);
        let name = server.name.clone();
        let missing = self.missing(server);
        let probe = self.probes.get(&server.name);
        let status = match (enabled, probe) {
            (false, _) => Badge::new("Off"),
            (true, _) if missing.len() == 1 => {
                Badge::new(format!("Needs {}", missing[0])).tone(Tone::Warning)
            }
            (true, _) if !missing.is_empty() => {
                Badge::new(format!("Needs {} variables", missing.len())).tone(Tone::Warning)
            }
            (true, Some(Probe::Checking)) => Badge::new("Testing…").tone(Tone::Info),
            (true, Some(Probe::Ready(tools))) => Badge::new(if tools.len() == 1 {
                "1 tool".to_owned()
            } else {
                format!("{} tools", tools.len())
            })
            .tone(Tone::Success),
            (true, Some(Probe::Failed(_))) => Badge::new("Failed").tone(Tone::Danger),
            (true, None) => Badge::new("Not tested"),
        };
        let mut header = div().flex().items_center().gap(px(METRICS.space_2)).child(
            text_style(div(), TextStyle::BodyMedium)
                .flex_1()
                .min_w_0()
                .text_color(colors.text_1)
                .child(name.clone()),
        );
        if server.is_http() {
            header = header.child(Badge::new("HTTP"));
        }
        let header = header.child(status).child(
            Toggle::new(&theme, ("mcp-enabled", index), enabled)
                .label(format!("Use {name}"))
                .on_change({
                    let entity = entity.clone();
                    move |on, _, app| entity.update(app, |page, cx| page.set_enabled(index, on, cx))
                }),
        );
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(self.command_line(("mcp-command", index), ("mcp-copy", index), server));
        let variables = server.variables();
        if !variables.is_empty() {
            let vars = variables
                .iter()
                .map(|var| {
                    let set = !missing.iter().any(|m| m == var);
                    format!("{var} {}", if set { "(set)" } else { "(not set)" })
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let label = if server.is_http() {
                "Headers from Studio's environment"
            } else {
                "Variables from Studio's environment"
            };
            body = body.child(self.muted(format!("{label}: {vars}")));
        }
        if enabled && !missing.is_empty() {
            body = body.child(
                text_style(div(), TextStyle::Small)
                    .text_color(colors.warning)
                    .child(format!(
                        "Left out of agent tasks until {} {} set in Studio's environment. Set {}, then restart Studio.",
                        and_list(&missing),
                        if missing.len() == 1 { "is" } else { "are" },
                        if missing.len() == 1 { "it" } else { "them" },
                    )),
            );
        }
        let tools = match probe {
            Some(Probe::Ready(tools)) => Some(tools.clone()),
            _ => None,
        };
        match probe {
            Some(Probe::Ready(tools)) if tools.is_empty() => {
                body = body.child(self.muted("It started and lists no tools."));
            }
            Some(Probe::Failed(failure)) if missing.is_empty() => {
                body = body
                    .child(
                        text_style(div(), TextStyle::Small)
                            .text_color(colors.text_1)
                            .child(failure.hint(server.is_http())),
                    )
                    .child(
                        text_style(div(), TextStyle::Small)
                            .font_family(MONO_FAMILY)
                            .text_color(colors.danger)
                            .child(crate::kit::selectable(failure.detail.clone())),
                    );
            }
            _ => {}
        }
        let checking = probe == Some(&Probe::Checking);
        let open = self.expanded.contains(&server.name);
        let mut actions = div().flex().items_center().gap(px(METRICS.space_1)).child(
            Button::new(&theme, ("mcp-test", index))
                .label(if checking { "Testing…" } else { "Test" })
                .left_icon(Icon::RefreshCw.path())
                .size(ButtonSize::Sm)
                .emphasis(Emphasis::Ghost)
                .disabled(checking || !missing.is_empty())
                .on_click({
                    let entity = entity.clone();
                    let name = name.clone();
                    move |_, _, app| entity.update(app, |page, cx| page.test_server(&name, cx))
                }),
        );
        if tools.as_ref().is_some_and(|tools| !tools.is_empty()) {
            actions = actions.child(
                Button::new(&theme, ("mcp-tools", index))
                    .label(if open { "Hide tools" } else { "Show tools" })
                    .left_icon(if open {
                        Icon::ChevronDown.path()
                    } else {
                        Icon::ChevronRight.path()
                    })
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .on_click({
                        let entity = entity.clone();
                        let name = name.clone();
                        move |_, _, app| {
                            entity.update(app, |page, cx| {
                                if !page.expanded.remove(&name) {
                                    page.expanded.insert(name.clone());
                                }
                                cx.notify();
                            })
                        }
                    }),
            );
        }
        actions = actions
            .child(
                Button::new(&theme, ("mcp-edit", index))
                    .label("Edit")
                    .left_icon(Icon::Pencil.path())
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .disabled(self.editor.is_some())
                    .on_click({
                        let entity = entity.clone();
                        move |_, window, app| {
                            entity.update(app, |page, cx| {
                                page.open_editor(Some(index), None, window, cx)
                            })
                        }
                    }),
            )
            .child(
                Button::new(&theme, ("mcp-remove", index))
                    .label("Remove")
                    .left_icon(Icon::Trash.path())
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .on_click({
                        let entity = entity.clone();
                        move |_, _, app| {
                            entity.update(app, |page, cx| page.remove_server(index, cx))
                        }
                    }),
            );
        if self.scope == Scope::Project
            && let Some(project) = &self.project
            && project.overrides.changes_server(&name)
        {
            let reset = name.clone();
            actions = actions.child(div().flex_1()).child(self.project_change(
                ("mcp-project-reset", index),
                move |overrides| overrides.reset_server(&reset),
                cx,
            ));
        }
        let mut card = setting_card(&theme)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .child(header)
            .child(body)
            .child(actions);
        if open && let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
            card = card.child(self.render_server_tools(index, shown, &tools, cx));
        }
        if self.highlighted.as_deref() == Some(name.as_str()) {
            card = card.relative().shadow(focus_ring(&theme));
            if self.reveal {
                let page = entity.downgrade();
                card = card.child(reveal_probe(self.scroll.clone(), move |cx| {
                    let _ = page.update(cx, |page, _| page.reveal = false);
                }));
            }
        }
        card.into_any_element()
    }

    /// The tools a tested server lists, a switch each.
    fn render_server_tools(
        &self,
        index: usize,
        shown: &AgentToolSettings,
        tools: &[String],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let server = &self.settings.mcp_servers[index];
        let off = shown
            .mcp_servers
            .get(index)
            .map_or(&server.disabled_tools, |s| &s.disabled_tools);
        let on = tools.iter().filter(|tool| !off.contains(tool)).count();
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .pt(px(METRICS.space_2))
            .border_t(px(METRICS.size.divider))
            .border_color(theme.colors.line)
            .child(self.muted(format!(
                "{on} of {} on. Claude and Codex leave out the tools that are off. ACP agents \
                 get every tool of this server.",
                tools.len()
            )));
        for (tool_index, tool) in tools.iter().enumerate() {
            let entity = entity.clone();
            let server_name = server.name.clone();
            let tool_name = tool.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(METRICS.space_2))
                    .child(self.mono(tool.clone()).flex_1().min_w_0())
                    .child(
                        Toggle::new(
                            &theme,
                            ElementId::Name(format!("mcp-tool-{index}-{tool_index}").into()),
                            !off.contains(tool),
                        )
                        .label(format!("Offer {tool}"))
                        .on_change(move |on, _, app| {
                            entity.update(app, |page, cx| {
                                page.set_server_tool(&server_name, &tool_name, on, cx)
                            })
                        }),
                    ),
            );
        }
        list.into_any_element()
    }

    fn render_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let Some(editor) = &self.editor else {
            return div().into_any_element();
        };
        let field = |label: &'static str, id: &'static str, input: &Entity<TextInputState>| {
            FormField::stacked(&theme, label, TextInput::new(&theme, id, input))
        };
        let title = if editor.replacing.is_some() {
            "Edit server"
        } else {
            "Add a server"
        };
        let kind = SegmentedControl::new(
            &theme,
            "mcp-form-kind",
            vec![
                SegmentOption::new("Program", !editor.http),
                SegmentOption::new("URL", editor.http),
            ],
        )
        .on_select({
            let entity = entity.clone();
            move |index, _, app| {
                entity.update(app, |page, cx| {
                    if let Some(editor) = &mut page.editor {
                        editor.http = *index == 1;
                        editor.problem = None;
                    }
                    cx.notify();
                })
            }
        });
        let mut card = setting_card(&theme)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(METRICS.space_2))
                    .child(
                        text_style(div(), TextStyle::BodyMedium)
                            .flex_1()
                            .text_color(theme.colors.text_1)
                            .child(title),
                    )
                    .child(kind),
            )
            .child(field("Name", "mcp-form-name", &editor.name).hint(
                "Letters, digits, dashes and underscores. The agent sees its tools under this name.",
            ));
        if editor.http {
            card = card
                .child(field("URL", "mcp-form-url", &editor.url).hint(
                    "The server's streamable HTTP address, often ending in /mcp.",
                ))
                .child(field("Token variable", "mcp-form-bearer", &editor.bearer).hint(
                    "Optional. Its value is sent as Authorization: Bearer when a task starts and is never stored.",
                ))
                .child(field("Other headers", "mcp-form-headers", &editor.headers).hint(
                    "Optional. Header=VARIABLE pairs, separated by commas. Values come from Studio's environment.",
                ));
        } else {
            let program = div()
                .flex()
                .gap(px(METRICS.space_2))
                .child(div().flex_1().min_w_0().child(TextInput::new(
                    &theme,
                    "mcp-form-command",
                    &editor.command,
                )))
                .child(
                    Button::new(&theme, "mcp-form-choose")
                        .label("Choose…")
                        .emphasis(Emphasis::Neutral)
                        .on_click({
                            let entity = entity.clone();
                            move |_, _, app| entity.update(app, |page, cx| page.choose_program(cx))
                        }),
                );
            card = card
                .child(FormField::stacked(&theme, "Program", program).hint(
                    "A command on your PATH, such as npx or uvx, or the full path of the program.",
                ))
                .child(field("Arguments", "mcp-form-args", &editor.args).hint(
                    "Separated by spaces; quote an argument that has spaces.",
                ))
                .child(field("Environment variables", "mcp-form-env", &editor.env).hint(
                    "Names only, separated by commas. Values are read from Studio's environment when a task starts and are never stored.",
                ));
        }
        if let Some(problem) = &editor.problem {
            card = card.child(Notice::new(
                "mcp-form-problem",
                Tone::Danger,
                problem.clone(),
            ));
        }
        card.child(
            div()
                .flex()
                .gap(px(METRICS.space_2))
                .child(
                    Button::new(&theme, "mcp-form-save")
                        .label("Save and test")
                        .emphasis(Emphasis::Primary)
                        .on_click({
                            let entity = entity.clone();
                            move |_, _, app| entity.update(app, |page, cx| page.save_editor(cx))
                        }),
                )
                .child(
                    Button::new(&theme, "mcp-form-cancel")
                        .label("Cancel")
                        .emphasis(Emphasis::Ghost)
                        .on_click(move |_, _, app| {
                            entity.update(app, |page, cx| {
                                page.editor = None;
                                cx.notify();
                            })
                        }),
                ),
        )
        .into_any_element()
    }

    fn choose_program(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose the server program".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |page, cx| {
                    if let Some(editor) = &page.editor {
                        let text = path.display().to_string();
                        editor
                            .command
                            .update(cx, |input, cx| input.set_text(text, cx));
                    }
                });
            }
        })
        .detach();
    }

    /// A titled panel with a Close button, for the import and registry panels.
    fn panel(
        &self,
        title: &'static str,
        description: &'static str,
        close_id: &'static str,
        close: impl Fn(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme;
        let entity = cx.entity();
        setting_card(&theme)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_3))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(METRICS.space_2))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap(px(METRICS.space_1))
                            .child(
                                text_style(div(), TextStyle::BodyMedium)
                                    .text_color(theme.colors.text_1)
                                    .child(title),
                            )
                            .child(self.muted(description)),
                    )
                    .child(
                        IconButton::new(&theme, close_id, Icon::X.path())
                            .size(IconButtonSize::Md)
                            .emphasis(Emphasis::Ghost)
                            .label("Close")
                            .on_click(move |_, _, app| {
                                entity.update(app, |page, cx| {
                                    close(page);
                                    cx.notify();
                                })
                            }),
                    ),
            )
    }

    fn render_import(&self, rows: Vec<ImportRow>, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let colors = theme.colors;
        let entity = cx.entity();
        let mut panel = self.panel(
            "Import servers",
            "Found in Claude Code, Claude Desktop and Codex. Only the names of variables come \
             along, never their values.",
            "mcp-import-close",
            |page| page.import = Import::Closed,
            cx,
        );
        if rows.is_empty() {
            return panel
                .child(self.muted(
                    "No servers found in Claude Code (~/.claude.json), Claude Desktop or Codex \
                     (~/.codex/config.toml).",
                ))
                .into_any_element();
        }
        let picked = rows.iter().filter(|row| row.picked).count();
        for (index, row) in rows.iter().enumerate() {
            let server = &row.found.server;
            let taken = self.taken(&server.name);
            let mut title = div()
                .flex()
                .items_center()
                .gap(px(METRICS.space_2))
                .child(
                    Toggle::new(&theme, ("mcp-import-pick", index), row.picked)
                        .label(format!("Import {}", server.name))
                        .on_change({
                            let entity = entity.clone();
                            move |on, _, app| {
                                entity.update(app, |page, cx| {
                                    if let Import::Found(rows) = &mut page.import
                                        && let Some(row) = rows.get_mut(index)
                                    {
                                        row.picked = on;
                                        if on && row.on_duplicate == OnDuplicate::Skip {
                                            row.on_duplicate = OnDuplicate::Overwrite;
                                        }
                                    }
                                    cx.notify();
                                })
                            }
                        }),
                )
                .child(
                    text_style(div(), TextStyle::BodyMedium)
                        .flex_1()
                        .min_w_0()
                        .text_color(colors.text_1)
                        .child(server.name.clone()),
                )
                .child(Badge::new(row.found.source));
            if server.is_http() {
                title = title.child(Badge::new("HTTP"));
            }
            let mut item = div()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_1))
                .py(px(METRICS.space_2))
                .border_t(px(METRICS.size.divider))
                .border_color(colors.line)
                .child(title)
                .child(self.command_line(
                    ("mcp-import-command", index),
                    ("mcp-import-copy", index),
                    server,
                ));
            let variables = server.variables();
            item = item.child(self.muted(if variables.is_empty() {
                "No variables.".to_owned()
            } else {
                format!("Variables: {}", variables.join(", "))
            }));
            if row.found.had_values {
                item = item.child(self.muted(format!(
                    "Its values stay in {}; set them in Studio's environment.",
                    row.found.source
                )));
            }
            if taken {
                let choice = SegmentedControl::new(
                    &theme,
                    ("mcp-import-duplicate", index),
                    vec![
                        SegmentOption::new(
                            "Skip",
                            !row.picked || row.on_duplicate == OnDuplicate::Skip,
                        ),
                        SegmentOption::new(
                            "Overwrite",
                            row.picked && row.on_duplicate == OnDuplicate::Overwrite,
                        ),
                    ],
                )
                .on_select({
                    let entity = entity.clone();
                    move |choice, _, app| {
                        entity.update(app, |page, cx| {
                            if let Import::Found(rows) = &mut page.import
                                && let Some(row) = rows.get_mut(index)
                            {
                                let overwrite = *choice == 1;
                                row.picked = overwrite;
                                row.on_duplicate = if overwrite {
                                    OnDuplicate::Overwrite
                                } else {
                                    OnDuplicate::Skip
                                };
                            }
                            cx.notify();
                        })
                    }
                });
                item = item.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(METRICS.space_2))
                        .child(Badge::new("Already added").tone(Tone::Warning))
                        .child(self.muted("A server has this name.").flex_1())
                        .child(choice),
                );
            }
            panel = panel.child(item);
        }
        panel
            .child(
                div()
                    .flex()
                    .gap(px(METRICS.space_2))
                    .child(
                        Button::new(&theme, "mcp-import-selected")
                            .label(if picked == 0 {
                                "Import selected".to_owned()
                            } else {
                                format!("Import selected ({picked})")
                            })
                            .emphasis(Emphasis::Primary)
                            .disabled(picked == 0)
                            .on_click({
                                let entity = entity.clone();
                                move |_, _, app| {
                                    entity.update(app, |page, cx| page.import_selected(cx))
                                }
                            }),
                    )
                    .child(
                        Button::new(&theme, "mcp-import-cancel")
                            .label("Cancel")
                            .emphasis(Emphasis::Ghost)
                            .on_click(move |_, _, app| {
                                entity.update(app, |page, cx| {
                                    page.import = Import::Closed;
                                    cx.notify();
                                })
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_registry(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let colors = theme.colors;
        let entity = cx.entity();
        let Some(registry) = &self.registry else {
            return div().into_any_element();
        };
        let loading = matches!(registry.state, RegistryState::Loading);
        let mut panel = self
            .panel(
                "Browse the MCP registry",
                "Servers listed at registry.modelcontextprotocol.io. Choosing one fills in the \
                 form; nothing runs until you test or save it.",
                "mcp-registry-close",
                |page| page.registry = None,
                cx,
            )
            .child(
                div()
                    .flex()
                    .gap(px(METRICS.space_2))
                    .child(div().flex_1().min_w_0().child(TextInput::new(
                        &theme,
                        "mcp-registry-query",
                        &registry.query,
                    )))
                    .child(
                        Button::new(&theme, "mcp-registry-search")
                            .label("Search")
                            .left_icon(Icon::Search.path())
                            .emphasis(Emphasis::Neutral)
                            .disabled(loading)
                            .on_click({
                                let entity = entity.clone();
                                move |_, _, app| {
                                    entity.update(app, |page, cx| page.search_registry(cx))
                                }
                            }),
                    ),
            );
        match &registry.state {
            RegistryState::Idle => {
                panel = panel.child(self.muted("Search by name, such as github or postgres."));
            }
            RegistryState::Loading => {
                panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(METRICS.space_2))
                        .child(Spinner::new(&theme, "mcp-registry-spinner"))
                        .child(self.muted("Searching the registry…")),
                );
            }
            RegistryState::Failed(error) => {
                panel = panel.child(
                    Notice::new(
                        "mcp-registry-error",
                        Tone::Danger,
                        "The MCP registry could not be searched",
                    )
                    .body("Check your connection, then search again. You can still add a server yourself.")
                    .details(error.clone()),
                );
            }
            RegistryState::Found(entries) if entries.is_empty() => {
                panel = panel
                    .child(EmptyState::new(&theme, "No servers found").body("Try another name."));
            }
            RegistryState::Found(entries) => {
                for (index, entry) in entries.iter().enumerate() {
                    let mut item = div()
                        .flex()
                        .flex_col()
                        .gap(px(METRICS.space_1))
                        .py(px(METRICS.space_2))
                        .border_t(px(METRICS.size.divider))
                        .border_color(colors.line)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(METRICS.space_2))
                                .child(
                                    text_style(div(), TextStyle::BodyMedium)
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(colors.text_1)
                                        .child(entry.name.clone()),
                                )
                                .child(Badge::new(entry.version.clone())),
                        );
                    if let Some(description) = &entry.description {
                        item = item.child(self.muted(description.clone()));
                    }
                    if entry.options.is_empty() {
                        item = item.child(self.muted("It lists no package or URL to run."));
                    }
                    for (option_index, option) in entry.options.iter().enumerate() {
                        let mut row = div().flex().items_center().gap(px(METRICS.space_2)).child(
                            self.mono(shorten_middle(&option.label, COMMAND_CHARS))
                                .flex_1()
                                .min_w_0(),
                        );
                        match &option.draft {
                            Ok(draft) => {
                                let draft = draft.clone();
                                let entity = entity.clone();
                                row = row.child(
                                    Button::new(
                                        &theme,
                                        ElementId::Name(
                                            format!("mcp-registry-use-{index}-{option_index}")
                                                .into(),
                                        ),
                                    )
                                    .label("Use")
                                    .size(ButtonSize::Sm)
                                    .emphasis(Emphasis::Neutral)
                                    .on_click(move |_, window, app| {
                                        let draft = draft.clone();
                                        entity.update(app, |page, cx| {
                                            page.open_editor(None, Some(draft), window, cx);
                                            page.say(
                                                Tone::Info,
                                                "Check the server, then save and test it. Nothing ran yet.",
                                                cx,
                                            );
                                        })
                                    }),
                                );
                            }
                            Err(reason) => {
                                row = row.child(self.muted(reason.clone()).flex_1().min_w_0());
                            }
                        }
                        item = item.child(row);
                    }
                    panel = panel.child(item);
                }
            }
        }
        panel.into_any_element()
    }

    // ---- Tools ---------------------------------------------------------------------------------

    fn render_tools(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let shown = self.shown();
        let enabled = shown.enabled_tools();
        let changed = |tool: &str| {
            self.scope == Scope::Project
                && self
                    .project
                    .as_ref()
                    .is_some_and(|project| project.overrides.tools.contains_key(tool))
        };
        let rows: Vec<AnyElement> = TOOL_NAMES
            .iter()
            .enumerate()
            .map(|(index, tool)| {
                let (label, description) = tool_copy(tool);
                let on = enabled.contains(tool);
                let entity = entity.clone();
                // The name the agent's transcript shows, beside the switch.
                let mut control = div().flex().items_center().gap(px(METRICS.space_3));
                if changed(tool) {
                    control = control.child(Badge::new("Changed").tone(Tone::Info));
                }
                let control = control
                    .child(self.mono(*tool).text_color(theme.colors.text_3))
                    .child(
                        Toggle::new(&theme, ("agent-tool", index), on)
                            .label(format!("Offer {label}"))
                            .on_change(move |on, _, app| {
                                entity.update(app, |page, cx| page.set_tools(&[*tool], on, cx))
                            }),
                    );
                SettingRow::new(&theme, label)
                    .description(SharedString::from(description))
                    .control(control)
                    .into_any_element()
            })
            .collect();
        let all_on = enabled.len() == TOOL_NAMES.len();
        let mut trailing = div().flex().items_center().gap(px(METRICS.space_2));
        if self.scope == Scope::Project
            && self
                .project
                .as_ref()
                .is_some_and(|project| project.overrides.changes_tools())
        {
            trailing = trailing.child(self.project_change(
                "agent-tools-project-reset",
                ProjectToolOverride::reset_tools,
                cx,
            ));
        }
        let trailing = trailing.child(
            Button::new(&theme, "agent-tools-all")
                .label("Turn all on")
                .size(ButtonSize::Sm)
                .emphasis(Emphasis::Ghost)
                .disabled(all_on)
                .on_click(move |_, _, app| {
                    entity.update(app, |page, cx| page.set_tools(&TOOL_NAMES, true, cx))
                }),
        );
        let mut section = PanelSection::new(&theme, "Project tools")
            .description(format!(
                "What Studio's own server lets the agent do in this project: {} of {} on. A \
                 tool that is off is not offered and is refused if the agent calls it. A \
                 running task keeps the tools it started with.",
                enabled.len(),
                TOOL_NAMES.len()
            ))
            .trailing(trailing)
            .children(rows);
        if enabled.is_empty() {
            section = section.child(setting_note(
                &theme,
                "With every tool off the agent works blind: it cannot build the project or \
                 look at a frame before it hands in its change.",
            ));
        }
        let mut column = div().flex().flex_col().gap(px(METRICS.space_6));
        if let Some(scope) = self.render_scope(cx) {
            column = column.child(scope);
        }
        column.child(section).into_any_element()
    }
}

impl Render for AgentToolsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme.refresh(window, cx);
        match self.view {
            ToolsView::Mcp => self.render_mcp(cx),
            ToolsView::Tools => self.render_tools(cx),
        }
    }
}

/// The gallery's registry answer.
const REGISTRY_FIXTURE: &str = r#"{"servers": [
  {"server": {"name": "io.github.example/papers", "version": "1.2.3",
    "description": "Search and read research papers.",
    "packages": [
      {"registryType": "npm", "identifier": "@example/papers-mcp", "version": "1.2.3",
       "transport": {"type": "stdio"}, "environmentVariables": [{"name": "PAPERS_API_KEY"}]},
      {"registryType": "oci", "identifier": "docker.io/example/papers", "version": "1.2.3"}
    ],
    "remotes": [{"type": "streamable-http", "url": "https://papers.example.com/mcp",
      "headers": [{"name": "Authorization"}]}]}},
  {"server": {"name": "io.github.example/papers-archive", "version": "0.4.0",
    "description": "Older papers from the archive, over SSE.",
    "remotes": [{"type": "sse", "url": "https://archive.example.com/sse"}]}}
]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_round_trip_through_the_one_line_field() {
        let args = vec![
            "-y".to_owned(),
            "@scope/server".to_owned(),
            "C:\\My Files".to_owned(),
        ];
        let line = join_args(&args);
        assert_eq!(line, "-y @scope/server \"C:\\My Files\"");
        assert_eq!(split_args(&line), args);
        assert_eq!(split_args("  a   b "), vec!["a", "b"]);
        assert_eq!(split_args("\"\""), vec![""]);
        assert_eq!(split_names("A_TOKEN, B  C"), vec!["A_TOKEN", "B", "C"]);
    }

    #[test]
    fn headers_round_trip_through_their_field() {
        let headers = split_headers(" X-Team = TEAM_ID, X-Key=KEY ,").unwrap();
        assert_eq!(
            headers,
            BTreeMap::from([
                ("X-Key".to_owned(), "KEY".to_owned()),
                ("X-Team".to_owned(), "TEAM_ID".to_owned()),
            ])
        );
        assert_eq!(split_headers(&join_headers(&headers)).unwrap(), headers);
        assert!(split_headers("X-Team").is_err());
        assert!(split_headers("").unwrap().is_empty());
    }

    #[test]
    fn long_commands_keep_their_start_and_end() {
        let command = r"C:\Program Files\nodejs\npx.cmd -y @modelcontextprotocol/server-filesystem C:\Users\Administrator\Documents";
        let short = shorten_middle(command, 64);
        assert!(short.chars().count() <= 66, "{short}");
        assert!(
            short.starts_with(r"C:\Program Files\nodejs\npx.cmd"),
            "{short}"
        );
        assert!(short.ends_with("Documents"), "{short}");
        assert!(short.contains('…'));

        // A server's command drops the program's folder before its package name.
        let server = UserMcpServer::stdio(
            "github",
            if cfg!(windows) {
                r"C:\Program Files\nodejs\npx.cmd"
            } else {
                "/opt/program files/nodejs/npx.cmd"
            },
            vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            vec![],
        );
        let short = short_command(&server, 64);
        assert_eq!(
            short,
            format!(
                "…{}npx.cmd -y @modelcontextprotocol/server-github",
                std::path::MAIN_SEPARATOR
            )
        );
        let fits = UserMcpServer::stdio("x", r"C:\Tools\uvx.exe", vec!["s".into()], vec![]);
        assert_eq!(short_command(&fits, 64), r"C:\Tools\uvx.exe s");
        assert_eq!(shorten_middle("npx -y x", 64), "npx -y x");
        assert_eq!(
            and_list(&["A".into(), "B".into(), "C".into()]),
            "A, B and C"
        );
    }

    #[test]
    fn the_gallery_s_registry_answer_parses() {
        let entries = parse_registry(REGISTRY_FIXTURE.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].options[0].draft.is_ok());
        assert!(entries[1].options[0].draft.is_err());
    }
}
