//! The composer's Tools popover and approval selector, after Oleafly's `AiToolManager` and
//! `ApprovalModeSelector`.
//!
//! Both follow the kit [`Menu`](crate::kit::Menu)'s contract: they open against their
//! trigger, a press outside or Escape closes them, Up and Down move a highlight (over
//! [`MenuState`]) and Enter or Space acts on the highlighted row. Every switch and row is
//! also a tab stop that Enter and Space activate; Tab and Shift+Tab keep to the popover's own
//! stops and wrap at its ends, and the tool list scrolls to the one Tab reaches. The views hold no state: the panel owns
//! the open popover and the settings, and the shell saves a change the way Settings → Agent
//! does.

use std::rc::Rc;

use gpui::{
    Anchor, AnyElement, App, Bounds, Div, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, RenderOnce, Role, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, point, prelude::FluentBuilder, px,
};
use studio_engine::agent_settings::AgentToolSettings;
use studio_engine::agent_tools::TOOL_NAMES;
use studio_proto::PermissionPolicy;
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use crate::kit::{
    Icon, MenuItem, MenuKey, MenuState, Popover, States, Toggle, focus_within, focusable, icon,
    reveal_focused,
};
use crate::shell::AgentTab;

/// The Tools popover's and the approval selector's widths (Oleafly's `w-72`).
const TOOLS_W: f32 = 296.;
const APPROVAL_W: f32 = 288.;
/// The tallest the tool list grows before it scrolls; it shrinks further when there is less
/// room above the trigger, down to a few rows.
const LIST_MAX_H: f32 = 480.;
/// The gap between a popover and its trigger, and the least margin to the window's edge.
const GAP: f32 = METRICS.space_1;
const MARGIN: f32 = METRICS.space_3;
const ROW_H: f32 = METRICS.size.list_row_h;
const ROW_CORNER: Pixels = px(METRICS.radius.item);
/// The least height a popover keeps when the room above its trigger is short.
const MIN_H: f32 = 4. * ROW_H;
/// The margins above and below a popover opened without a trigger.
const EDGES: f32 = 2. * MARGIN;

type Handler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;
type DismissHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A project tool's name and what it gives the agent: the one copy the composer's Tools
/// popover and Settings → Agent → Tools both show.
pub(crate) fn tool_copy(tool: &str) -> (&'static str, &'static str) {
    match tool {
        "project_context" => (
            "Project context",
            "The manifest, the source files and the revision a call describes.",
        ),
        "timeline" => (
            "Timeline",
            "Size, frame rate, scenes and audio tracks of the current build.",
        ),
        "render_frame" => (
            "Render frame",
            "Renders one frame to an image the agent can look at.",
        ),
        "render_strip" => (
            "Render strip",
            "Renders up to 24 frames side by side, to check motion.",
        ),
        "inspect" => (
            "Inspect",
            "Finds missing assets and unsupported shaders across frames.",
        ),
        "build_status" => (
            "Build status",
            "Whether the project compiles, without starting a build.",
        ),
        "selection_context" => (
            "Selection",
            "The objects on the frame you selected in the player.",
        ),
        "source_lookup" => (
            "Source lookup",
            "Finds the Rust source of a symbol or of a selected object.",
        ),
        "style_context" => (
            "Style",
            "The active preset's design tokens and where they are used.",
        ),
        _ => ("Tool", ""),
    }
}

/// A project tool's name in the popover. Settings → Agent → Tools describes each one.
pub fn tool_label(tool: &str) -> &'static str {
    tool_copy(tool).0
}

/// The policy's name on the composer's trigger.
pub fn policy_short_label(policy: PermissionPolicy) -> &'static str {
    match policy {
        PermissionPolicy::AskEveryTime => "Ask",
        PermissionPolicy::AllowInWorkingCopy => "Working copy",
        PermissionPolicy::AllowEverything => "Everything",
    }
}

/// What the policy does, in a line under its name.
fn policy_description(policy: PermissionPolicy) -> &'static str {
    match policy {
        PermissionPolicy::AskEveryTime => "The agent asks before every action.",
        PermissionPolicy::AllowInWorkingCopy => {
            "Edits in its private copy without asking. Commands and the network still ask."
        }
        PermissionPolicy::AllowEverything => {
            "Also runs commands and uses the network without asking."
        }
    }
}

/// The trigger's accessible name and tooltip.
pub fn policy_name(policy: PermissionPolicy) -> String {
    format!("Approval: {}", policy.label())
}

/// A row of the Tools popover, in the order Up and Down walk them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolsRow {
    /// A project tool of Studio's own server, by its name in [`TOOL_NAMES`].
    Tool(&'static str),
    /// One of the user's MCP servers, by name.
    Server(String),
    /// A link to Settings → Agent at a tab.
    Manage(AgentTab),
}

/// The rows of the Tools popover: the project tools, the user's servers, then the links to
/// MCP servers and Tools in Settings.
pub fn tools_rows(settings: &AgentToolSettings) -> Vec<ToolsRow> {
    TOOL_NAMES
        .iter()
        .map(|tool| ToolsRow::Tool(tool))
        .chain(
            settings
                .mcp_servers
                .iter()
                .map(|server| ToolsRow::Server(server.name.clone())),
        )
        .chain([
            ToolsRow::Manage(AgentTab::Mcp),
            ToolsRow::Manage(AgentTab::Tools),
        ])
        .collect()
}

/// Whether `row` is on in `settings` (a link is never on).
pub fn row_on(settings: &AgentToolSettings, row: &ToolsRow) -> bool {
    match row {
        ToolsRow::Tool(tool) => !settings.disabled_tools.iter().any(|off| off == tool),
        ToolsRow::Server(name) => settings
            .mcp_servers
            .iter()
            .any(|server| &server.name == name && server.enabled),
        ToolsRow::Manage(_) => false,
    }
}

/// `settings` with `row` switched, or `None` for a link or a server that is gone.
pub fn toggled(settings: &AgentToolSettings, row: &ToolsRow) -> Option<AgentToolSettings> {
    let mut next = settings.clone();
    match row {
        ToolsRow::Tool(tool) => next.set_tool(tool, !row_on(settings, row)),
        ToolsRow::Server(name) => {
            let server = next.mcp_servers.iter_mut().find(|s| &s.name == name)?;
            server.enabled = !server.enabled;
        }
        ToolsRow::Manage(_) => return None,
    }
    Some(next)
}

/// The keyboard state of a freshly opened Tools popover: nothing highlighted, so Enter does
/// nothing until an arrow key picks a row (Down picks the first).
pub fn tools_menu(settings: &AgentToolSettings, trigger: Option<Bounds<Pixels>>) -> MenuState {
    let items: Vec<MenuItem> = tools_rows(settings)
        .iter()
        .map(|row| MenuItem::new(format!("{row:?}"), false))
        .collect();
    match trigger {
        Some(bounds) => MenuState::anchored(bounds, &items),
        None => MenuState::new(gpui::Point::default(), &items),
    }
}

/// The rows of the approval selector: the three policies, then the link to Permissions.
fn approval_rows() -> [Option<PermissionPolicy>; 4] {
    let [a, b, c] = PermissionPolicy::ALL;
    [Some(a), Some(b), Some(c), None]
}

/// The keyboard state of a freshly opened approval selector: on the current policy.
pub fn approval_menu(policy: PermissionPolicy, trigger: Option<Bounds<Pixels>>) -> MenuState {
    let items: Vec<MenuItem> = approval_rows()
        .iter()
        .map(|row| MenuItem::new(format!("{row:?}"), *row == Some(policy)))
        .collect();
    match trigger {
        Some(bounds) => MenuState::anchored(bounds, &items),
        None => MenuState::new(gpui::Point::default(), &items),
    }
}

/// Places `panel` above `trigger` in window coordinates, over a layer that closes it on a
/// press outside, as the kit's menu does. Without a trigger the panel opens upward from
/// where the caller lays it out (the gallery's fixtures).
///
/// A popover opens upward and is no taller than the room above the point it rises from;
/// its list scrolls for the rest. Taller, `anchored` would snap it down over that point.
fn layer(
    id: &'static str,
    trigger: Option<Bounds<Pixels>>,
    panel: Stateful<Div>,
    on_dismiss: Option<DismissHandler>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(trigger) = trigger else {
        // Where the caller laid the popover out is known only after layout: it is measured
        // each frame, and the next frame sizes the panel to the room above it.
        let anchor =
            window.use_keyed_state(SharedString::from(format!("{id}-anchor")), cx, |_, _| {
                None::<Pixels>
            });
        let room = match *anchor.read(cx) {
            Some(top) => top - px(MARGIN),
            None => window.viewport_size().height - px(EDGES),
        };
        let measure = gpui::canvas(
            move |bounds, window, cx| {
                let top = Some(bounds.top());
                if *anchor.read(cx) != top {
                    anchor.update(cx, |anchor, _| *anchor = top);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        );
        return div()
            .child(measure)
            .child(
                gpui::deferred(
                    gpui::anchored()
                        .anchor(Anchor::BottomLeft)
                        .snap_to_window_with_margin(px(MARGIN))
                        .child(panel.max_h(room.max(px(MIN_H)))),
                )
                .with_priority(1),
            )
            .into_any_element();
    };
    let room = trigger.top() - px(GAP + MARGIN);
    let panel = panel.max_h(room.max(px(MIN_H)));
    let viewport = window.viewport_size();
    let layer = div()
        .relative()
        .w(viewport.width)
        .h(viewport.height)
        .child(
            div()
                .id(SharedString::from(format!("{id}-backdrop")))
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .when_some(on_dismiss, |this, handler| {
                    this.on_click(move |_, window, cx| handler(window, cx))
                }),
        )
        .child(
            gpui::anchored()
                .position(trigger.origin - point(Pixels::ZERO, px(GAP)))
                .anchor(Anchor::BottomLeft)
                .snap_to_window_with_margin(px(MARGIN))
                .child(panel),
        );
    gpui::deferred(
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .position(gpui::Point::default())
            .child(layer),
    )
    .with_priority(1)
    .into_any_element()
}

/// What a popover's keys act on.
struct Keys {
    state: MenuState,
    focus: Option<FocusHandle>,
    on_highlight: Option<Handler<usize>>,
    /// Enter or Space on the highlighted row.
    activate: Handler<usize>,
    on_dismiss: Option<DismissHandler>,
}

/// The floating surface both popovers draw on; Escape closes it from anywhere inside, and
/// the arrow keys, Enter and Space act on the highlight while the surface itself has the
/// focus (a focused switch or row answers its own Enter and Space).
fn surface(theme: &Theme, id: &'static str, width: f32, keys: Keys) -> Stateful<Div> {
    let Keys {
        state,
        focus,
        on_highlight,
        activate,
        on_dismiss,
    } = keys;
    let own = focus.clone();
    let keys = move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
        let key = event.keystroke.key.as_str();
        if key != "escape" && !own.as_ref().is_some_and(|focus| focus.is_focused(window)) {
            return;
        }
        let mut next = state.clone();
        match next.on_key(key) {
            MenuKey::Moved => {
                if let (Some(handler), Some(index)) = (&on_highlight, next.highlighted) {
                    handler(index, window, cx);
                }
            }
            MenuKey::Commit(index) => activate(index, window, cx),
            MenuKey::Dismiss => {
                if let Some(handler) = &on_dismiss {
                    handler(window, cx);
                }
            }
            MenuKey::Ignored => return,
        }
        cx.stop_propagation();
    };
    Popover::new(theme)
        .relative()
        .id(id)
        .role(Role::Dialog)
        .occlude()
        .when_some(focus, |this, focus| {
            let trap = focus.clone();
            this.track_focus(&focus)
                .capture_key_down(move |event: &KeyDownEvent, window, cx| {
                    let keystroke = &event.keystroke;
                    let mods = keystroke.modifiers;
                    if keystroke.key == "tab" && !mods.control && !mods.alt && !mods.platform {
                        focus_within(&trap, mods.shift, window, cx);
                        cx.stop_propagation();
                    }
                })
        })
        .on_key_down(keys)
        .w(px(width))
        .overflow_hidden()
        .text_color(theme.colors.text_1)
}

/// A section heading inside a popover: an overline with an optional count.
fn heading(theme: &Theme, title: &str, count: Option<String>) -> Div {
    text_style(div(), TextStyle::Overline)
        .flex()
        .items_center()
        .justify_between()
        .px(px(METRICS.space_2))
        .pt(px(METRICS.space_2))
        .pb(px(METRICS.space_1))
        .text_color(theme.colors.text_2)
        .child(title.to_uppercase())
        .children(count.map(|count| {
            text_style(div(), TextStyle::Small)
                .text_color(theme.colors.text_3)
                .child(count)
        }))
}

/// A row that links to Settings: a glyph and a label, a tab stop.
fn link_row(
    theme: &Theme,
    id: SharedString,
    label: &'static str,
    highlighted: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let states = States::of(theme);
    let row = focusable(div().id(id), theme, false)
        .role(Role::Link)
        .aria_label(label)
        .flex()
        .items_center()
        .gap(px(METRICS.space_2))
        .h(px(ROW_H))
        .px(px(METRICS.space_2))
        .rounded(ROW_CORNER)
        .cursor_pointer()
        .when(highlighted, |this| this.bg(states.rest))
        .hover(move |style| style.bg(states.rest))
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(icon(
            Icon::Settings,
            METRICS.size.icon_sm,
            theme.colors.text_2,
        ))
        .child(label);
    text_style(row, TextStyle::Label).text_color(theme.colors.text_2)
}

/// The Tools popover: the project tools and the user's MCP servers, a switch each.
#[derive(IntoElement)]
pub struct ToolsPopover {
    theme: Theme,
    settings: AgentToolSettings,
    state: MenuState,
    running: bool,
    focus: Option<FocusHandle>,
    on_change: Option<Handler<AgentToolSettings>>,
    on_manage: Option<Handler<AgentTab>>,
    on_highlight: Option<Handler<usize>>,
    on_dismiss: Option<DismissHandler>,
}

impl ToolsPopover {
    pub fn new(theme: &Theme, settings: AgentToolSettings, state: &MenuState) -> Self {
        Self {
            theme: *theme,
            settings,
            state: state.clone(),
            running: false,
            focus: None,
            on_change: None,
            on_manage: None,
            on_highlight: None,
            on_dismiss: None,
        }
    }

    /// A task is running: it keeps the tools it started with, and the popover says so.
    pub fn running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    /// Routes the keys to the popover while `focus` is focused.
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// Called with the settings after a switch flipped.
    pub fn on_change(
        mut self,
        handler: impl Fn(AgentToolSettings, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Called with the Settings tab a link opens.
    pub fn on_manage(
        mut self,
        handler: impl Fn(AgentTab, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_manage = Some(Rc::new(handler));
        self
    }

    /// The row an arrow key moved to; apply it with [`MenuState::set_highlighted`].
    pub fn on_highlight(
        mut self,
        handler: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(handler));
        self
    }

    /// A press outside, or Escape.
    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ToolsPopover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ToolsPopover {
            theme,
            settings,
            state,
            running,
            focus,
            on_change,
            on_manage,
            on_highlight,
            on_dismiss,
        } = self;
        let colors = theme.colors;
        let states = States::of(&theme);
        // The list's scrolling, kept while the popover stays open.
        let scroll = window
            .use_keyed_state("composer-tools-scroll", cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        let rows = tools_rows(&settings);
        let highlighted = state.visible_highlight();
        let trigger = state.trigger_bounds;
        let activate: Handler<usize> = {
            let rows = rows.clone();
            let settings = settings.clone();
            Rc::new(move |index, window, cx| match rows.get(index) {
                Some(ToolsRow::Manage(tab)) => {
                    if let Some(handler) = &on_manage {
                        handler(*tab, window, cx);
                    }
                }
                Some(row) => {
                    if let (Some(next), Some(handler)) = (toggled(&settings, row), &on_change) {
                        handler(next, window, cx);
                    }
                }
                None => {}
            })
        };
        let enabled = settings.enabled_tools().len();
        let mut tools = div().flex().flex_col().child(heading(
            &theme,
            "Project tools",
            Some(format!("{enabled} of {} on", TOOL_NAMES.len())),
        ));
        let mut servers = div().flex().flex_col().child(heading(
            &theme,
            "MCP servers",
            (!settings.mcp_servers.is_empty()).then(|| {
                let on = settings.mcp_servers.iter().filter(|s| s.enabled).count();
                format!("{on} of {} on", settings.mcp_servers.len())
            }),
        ));
        // The rule above the links is a border: it snaps to a whole device pixel, where a
        // half-pixel fill is not drawn at all.
        let mut footer = div()
            .flex_none()
            .flex()
            .flex_col()
            .p(px(METRICS.space_1))
            .border_t(px(METRICS.size.divider))
            .border_color(colors.line);
        for (index, row) in rows.iter().enumerate() {
            let lit = highlighted == Some(index);
            let act = activate.clone();
            let element = match row {
                ToolsRow::Manage(tab) => link_row(
                    &theme,
                    SharedString::from(format!("composer-tools-manage-{}", manage_key(*tab))),
                    if *tab == AgentTab::Mcp {
                        "Manage servers"
                    } else {
                        "Manage in Settings"
                    },
                    lit,
                    move |window, cx| act(index, window, cx),
                )
                .into_any_element(),
                ToolsRow::Tool(_) | ToolsRow::Server(_) => {
                    let (label, id): (SharedString, SharedString) = match row {
                        ToolsRow::Tool(tool) => (
                            tool_label(tool).into(),
                            format!("composer-tool-{tool}").into(),
                        ),
                        ToolsRow::Server(name) => (
                            name.clone().into(),
                            format!("composer-server-{name}").into(),
                        ),
                        ToolsRow::Manage(_) => unreachable!("handled above"),
                    };
                    let on = row_on(&settings, row);
                    let name = format!("Offer {label}");
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(METRICS.space_2))
                        .h(px(ROW_H))
                        .px(px(METRICS.space_2))
                        .rounded(ROW_CORNER)
                        .when(lit, |this| this.bg(states.rest))
                        .child(
                            text_style(div().min_w_0().truncate(), TextStyle::Body)
                                .text_color(if on { colors.text_1 } else { colors.text_2 })
                                .child(label),
                        )
                        .child(
                            Toggle::new(&theme, id, on)
                                .label(name)
                                .on_change(move |_, window, cx| act(index, window, cx)),
                        )
                        .into_any_element()
                }
            };
            match row {
                ToolsRow::Tool(_) => tools = tools.child(element),
                ToolsRow::Server(_) => servers = servers.child(element),
                ToolsRow::Manage(AgentTab::Mcp) => servers = servers.child(element),
                ToolsRow::Manage(_) => footer = footer.child(element),
            }
        }
        if settings.mcp_servers.is_empty() {
            servers = servers.child(
                text_style(div(), TextStyle::Small)
                    .px(px(METRICS.space_2))
                    .pb(px(METRICS.space_1))
                    .text_color(colors.text_2)
                    .child("No servers added."),
            );
        }
        let header = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .px(px(METRICS.space_3))
            .py(px(METRICS.space_2))
            .child(text_style(div(), TextStyle::BodyMedium).child("Tools"))
            .child(
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_2)
                    .child(if running {
                        "A task is running. Changes apply to the next task."
                    } else {
                        "What the agent may use in the next task."
                    }),
            );
        let panel = surface(
            &theme,
            "composer-tools-popover",
            TOOLS_W,
            Keys {
                state,
                focus,
                on_highlight,
                activate,
                on_dismiss: on_dismiss.clone(),
            },
        )
        .aria_label("Tools")
        .child(header)
        .child(
            div()
                .id("composer-tools-list")
                .track_scroll(&scroll)
                .on_action(reveal_focused(scroll.clone(), METRICS.space_1))
                .border_t(px(METRICS.size.divider))
                .border_color(colors.line)
                .max_h(px(LIST_MAX_H))
                .flex_shrink(1.)
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(px(METRICS.space_1))
                .child(tools)
                .child(servers),
        )
        .child(footer);
        layer("composer-tools", trigger, panel, on_dismiss, window, cx)
    }
}

fn manage_key(tab: AgentTab) -> &'static str {
    match tab {
        AgentTab::Agents => "agents",
        AgentTab::Mcp => "mcp",
        AgentTab::Tools => "tools",
        AgentTab::Permissions => "permissions",
        AgentTab::Review => "review",
    }
}

/// The approval selector: the three permission policies, the current one checked.
#[derive(IntoElement)]
pub struct ApprovalPopover {
    theme: Theme,
    policy: PermissionPolicy,
    state: MenuState,
    running: bool,
    focus: Option<FocusHandle>,
    on_choose: Option<Handler<PermissionPolicy>>,
    on_manage: Option<Handler<AgentTab>>,
    on_highlight: Option<Handler<usize>>,
    on_dismiss: Option<DismissHandler>,
}

impl ApprovalPopover {
    pub fn new(theme: &Theme, policy: PermissionPolicy, state: &MenuState) -> Self {
        Self {
            theme: *theme,
            policy,
            state: state.clone(),
            running: false,
            focus: None,
            on_choose: None,
            on_manage: None,
            on_highlight: None,
            on_dismiss: None,
        }
    }

    /// A task is running: it keeps the policy it started with, and the popover says so.
    pub fn running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    pub fn on_choose(
        mut self,
        handler: impl Fn(PermissionPolicy, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_choose = Some(Rc::new(handler));
        self
    }

    pub fn on_manage(
        mut self,
        handler: impl Fn(AgentTab, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_manage = Some(Rc::new(handler));
        self
    }

    pub fn on_highlight(
        mut self,
        handler: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(handler));
        self
    }

    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ApprovalPopover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ApprovalPopover {
            theme,
            policy,
            state,
            running,
            focus,
            on_choose,
            on_manage,
            on_highlight,
            on_dismiss,
        } = self;
        let colors = theme.colors;
        let states = States::of(&theme);
        let rows = approval_rows();
        let highlighted = state.visible_highlight();
        let trigger = state.trigger_bounds;
        let activate: Handler<usize> = Rc::new(move |index, window, cx| match rows.get(index) {
            Some(Some(policy)) => {
                if let Some(handler) = &on_choose {
                    handler(*policy, window, cx);
                }
            }
            Some(None) => {
                if let Some(handler) = &on_manage {
                    handler(AgentTab::Permissions, window, cx);
                }
            }
            None => {}
        });
        let mut list = div().flex().flex_col().p(px(METRICS.space_1));
        // The rule above the links is a border: it snaps to a whole device pixel, where a
        // half-pixel fill is not drawn at all.
        let mut footer = div()
            .flex_none()
            .flex()
            .flex_col()
            .p(px(METRICS.space_1))
            .border_t(px(METRICS.size.divider))
            .border_color(colors.line);
        for (index, row) in rows.iter().enumerate() {
            let lit = highlighted == Some(index);
            let act = activate.clone();
            let Some(each) = row else {
                footer = footer.child(link_row(
                    &theme,
                    "composer-approval-manage".into(),
                    "Permission settings",
                    lit,
                    move |window, cx| act(index, window, cx),
                ));
                continue;
            };
            let checked = *each == policy;
            let item =
                focusable(
                    div().id(SharedString::from(format!(
                        "composer-approval-{}",
                        manage_policy_key(*each)
                    ))),
                    &theme,
                    false,
                )
                .role(Role::RadioButton)
                .aria_label(each.label())
                .flex()
                .items_start()
                .gap(px(METRICS.space_2))
                .px(px(METRICS.space_2))
                .py(px(METRICS.space_2))
                .rounded(ROW_CORNER)
                .cursor_pointer()
                .when(lit, |this| this.bg(states.rest))
                .hover(move |style| style.bg(states.rest))
                .on_click(move |_, window, cx| act(index, window, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(text_style(div(), TextStyle::Label).child(each.label()))
                        .child(
                            text_style(div(), TextStyle::Small)
                                .text_color(colors.text_2)
                                .child(policy_description(*each)),
                        ),
                )
                .child(div().flex_none().size(px(METRICS.size.icon_sm)).children(
                    checked.then(|| icon(Icon::Check, METRICS.size.icon_sm, colors.accent)),
                ));
            list = list.child(item);
        }
        let mut panel = surface(
            &theme,
            "composer-approval-popover",
            APPROVAL_W,
            Keys {
                state,
                focus,
                on_highlight,
                activate,
                on_dismiss: on_dismiss.clone(),
            },
        )
        .aria_label("Approval");
        if running {
            panel = panel.child(
                text_style(div(), TextStyle::Small)
                    .px(px(METRICS.space_3))
                    .pt(px(METRICS.space_2))
                    .text_color(colors.text_2)
                    .child("A task is running. Changes apply to the next task."),
            );
        }
        let panel = panel.child(list).child(footer);
        layer("composer-approval", trigger, panel, on_dismiss, window, cx)
    }
}

fn manage_policy_key(policy: PermissionPolicy) -> &'static str {
    match policy {
        PermissionPolicy::AskEveryTime => "ask",
        PermissionPolicy::AllowInWorkingCopy => "working-copy",
        PermissionPolicy::AllowEverything => "everything",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_engine::agent_settings::UserMcpServer;

    fn server(name: &str, enabled: bool) -> UserMcpServer {
        let mut server = UserMcpServer::stdio(name, "/bin/true", Vec::new(), Vec::new());
        server.enabled = enabled;
        server
    }

    #[test]
    fn the_rows_are_the_tools_the_servers_then_the_links() {
        let settings = AgentToolSettings {
            mcp_servers: vec![server("github", true)],
            disabled_tools: Vec::new(),
        };
        let rows = tools_rows(&settings);
        assert_eq!(rows.len(), TOOL_NAMES.len() + 3);
        assert_eq!(rows[0], ToolsRow::Tool(TOOL_NAMES[0]));
        assert_eq!(rows[TOOL_NAMES.len()], ToolsRow::Server("github".into()));
        assert_eq!(
            rows[TOOL_NAMES.len() + 1..],
            [
                ToolsRow::Manage(AgentTab::Mcp),
                ToolsRow::Manage(AgentTab::Tools)
            ]
        );
        for tool in TOOL_NAMES {
            assert_ne!(tool_label(tool), "Tool", "{tool} has no label");
            assert!(!tool_copy(tool).1.is_empty(), "{tool} has no description");
        }
    }

    #[test]
    fn a_switch_flips_one_tool_or_server_and_a_link_changes_nothing() {
        let settings = AgentToolSettings {
            mcp_servers: vec![server("github", true), server("sqlite", false)],
            disabled_tools: vec!["render_strip".into()],
        };
        let off = toggled(&settings, &ToolsRow::Tool("timeline")).unwrap();
        assert!(!row_on(&off, &ToolsRow::Tool("timeline")));
        assert!(!row_on(&off, &ToolsRow::Tool("render_strip")));
        let on = toggled(&settings, &ToolsRow::Tool("render_strip")).unwrap();
        assert!(on.disabled_tools.is_empty());
        let server_on = toggled(&settings, &ToolsRow::Server("sqlite".into())).unwrap();
        assert!(server_on.mcp_servers.iter().all(|s| s.enabled));
        assert!(toggled(&settings, &ToolsRow::Server("gone".into())).is_none());
        assert!(toggled(&settings, &ToolsRow::Manage(AgentTab::Tools)).is_none());
    }

    struct Host {
        state: MenuState,
        focus: FocusHandle,
        events: crate::kit::test_support::Log<String>,
    }

    impl gpui::Render for Host {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let entity = cx.entity();
            let (change, manage, dismiss) = (
                self.events.clone(),
                self.events.clone(),
                self.events.clone(),
            );
            div().size_full().child(
                ToolsPopover::new(&theme, AgentToolSettings::default(), &self.state)
                    .focus(&self.focus)
                    .on_highlight(move |index, _, cx| {
                        entity.update(cx, |host, cx| {
                            host.state.set_highlighted(index);
                            cx.notify();
                        })
                    })
                    .on_change(move |settings, _, _| {
                        change
                            .borrow_mut()
                            .push(format!("off {:?}", settings.disabled_tools))
                    })
                    .on_manage(move |tab, _, _| manage.borrow_mut().push(format!("{tab:?}")))
                    .on_dismiss(move |_, _| dismiss.borrow_mut().push("dismiss".into())),
            )
        }
    }

    #[gpui::test]
    fn the_tools_popover_answers_the_keyboard(cx: &mut gpui::TestAppContext) {
        use crate::kit::test_support::{log, press};
        let events = log();
        let shared = events.clone();
        let (host, cx) = cx.add_window_view(|_, cx| Host {
            state: tools_menu(&AgentToolSettings::default(), None),
            focus: cx.focus_handle(),
            events: shared,
        });
        cx.update(|window, cx| {
            let focus = host.read(cx).focus.clone();
            window.focus(&focus, cx);
        });
        // Enter before any arrow does nothing; Down picks the first tool and Space flips it.
        press(cx, "enter");
        press(cx, "down");
        press(cx, "space");
        // Up from the first row wraps to the last: the link to Settings → Agent → Tools.
        press(cx, "up");
        press(cx, "enter");
        press(cx, "escape");
        assert_eq!(
            *events.borrow(),
            [
                format!("off {:?}", [TOOL_NAMES[0]]),
                "Tools".to_owned(),
                "dismiss".to_owned()
            ]
        );
    }

    #[test]
    fn the_approval_selector_opens_on_the_current_policy() {
        let state = approval_menu(PermissionPolicy::AllowInWorkingCopy, None);
        assert_eq!(state.len, 4);
        assert_eq!(state.highlighted, Some(1));
        assert_eq!(policy_short_label(PermissionPolicy::AskEveryTime), "Ask");
    }
}
