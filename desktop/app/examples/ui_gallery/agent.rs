//! The agent panel's capture page (`--page agent`): the chat components of
//! `studio_ui::agent::ui` with the real composer, todo list and history list, in the fixture
//! states of `plans/261027-2015-agent-panel-components/plan.md`, at the sidebar's narrowest and
//! widest (320 and 520).
//!
//! The panel itself needs a running agent workflow, so the page lays the components out the
//! way `ConversationPanel` does: the header, the status line, the transcript and the dock.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, StatefulInteractiveElement as _, Styled, Window, div, px,
};
use studio_engine::agent::PermissionPolicy;
use studio_engine::agent::{OutcomeKind, Row, RowId, RowKind};
use studio_engine::agent_settings::{AgentToolSettings, UserMcpServer};
use studio_theme::{METRICS, TextStyle, Theme, text_style};
use studio_ui::agent::composer::{
    AttachmentDraft, Composer, ComposerControl, ModelChoice, SteerMode,
};
use studio_ui::agent::composer_tools::{
    ApprovalPopover, ToolsPopover, approval_menu, policy_name, policy_short_label, tools_menu,
};
use studio_ui::agent::history::{HistoryList, SessionEntry};
use studio_ui::agent::markdown::{MarkdownRow, Voice};
use studio_ui::agent::todo_panel::{TodoItem, TodoPanel, TodoStatus};
use studio_ui::agent::ui::metrics::{activity, status, transcript as inset};
use studio_ui::agent::ui::{
    ActionCard, ActivityCounts, ActivityHeader, ActivityRow, ActivityState, AssistantMessage,
    ChangeMark, ChangedFile, ChangesCard, JumpPill, PanelHeader, STATUS_MARK, StatusLine,
    TrayHeader, TrayRow, UserMessage, WorkingTrailer, panel_home, summary_parts, tray_frame,
};
use studio_ui::kit::{
    Badge, Button, Card, Chip, Emphasis, Icon, IconButton, IconButtonSize, RadioCard, RadioCards,
    SegmentOption, SegmentedControl, TextInput, TextInputState, Tone, icon,
};

use crate::Gallery;
use crate::capture::Page;

/// The sidebar's narrowest and widest.
const WIDTHS: [f32; 2] = [320., 520.];
/// The panel's height in the fixtures: a sidebar on a 1080 p screen, less the title bar.
const PANEL_H: f32 = 680.;
/// The question's free-text field, as `question_card.rs` sizes it (Oleafly's `h-7`).
const ANSWER_FIELD_H: f32 = 28.;

/// The permission policy the composer's approval trigger shows.
const GALLERY_POLICY: PermissionPolicy = PermissionPolicy::AllowInWorkingCopy;

/// The Tools popover's sample: two project tools off, two servers on and one off.
fn gallery_tools() -> AgentToolSettings {
    let server = |name: &str, enabled| {
        let mut server = UserMcpServer::stdio(
            name,
            r"C:\Program Files\nodejs\npx.cmd",
            Vec::new(),
            Vec::new(),
        );
        server.enabled = enabled;
        server
    };
    AgentToolSettings {
        mcp_servers: vec![
            server("github", true),
            server("context7", true),
            server("sqlite", false),
        ],
        disabled_tools: vec!["render_strip".into(), "source_lookup".into()],
    }
}

/// The entities one panel fixture draws: the composer, the todo list, the history list and
/// the question's text field.
struct Panel {
    composer: Entity<Composer>,
    todo: Entity<TodoPanel>,
    history: Entity<HistoryList>,
    answer: Entity<TextInputState>,
}

/// One panel per width, and the state and theme they were last set up for.
#[derive(Default)]
pub struct AgentFixtures {
    panels: Vec<Panel>,
    shown: Option<(&'static str, Theme)>,
}

impl Gallery {
    /// The agent page in its fixture state; `None` on the other pages.
    pub(crate) fn render_agent(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.page != Some(Page::Agent) {
            return None;
        }
        let state = self.phase7.state;
        let fixtures = &mut self.agent;
        if fixtures.panels.is_empty() {
            fixtures.panels = WIDTHS
                .iter()
                .map(|_| Panel {
                    composer: cx
                        .new(|cx| Composer::new("Describe the change you want", *theme, cx)),
                    todo: cx.new(|_| TodoPanel::new()),
                    history: cx.new(|cx| HistoryList::new(window, cx)),
                    answer: cx.new(|cx| TextInputState::single_line(window, cx)),
                })
                .collect();
        }
        if fixtures.shown != Some((state, *theme)) {
            fixtures.shown = Some((state, *theme));
            for panel in &fixtures.panels {
                set_up(panel, state, theme, cx);
            }
        }
        let panels = fixtures
            .panels
            .iter()
            .zip(WIDTHS)
            .enumerate()
            .map(|(ix, (panel, width))| render_panel(theme, state, ix, width, panel));
        Some(
            div()
                .flex()
                .items_start()
                .gap(px(METRICS.space_6))
                .children(panels)
                .into_any_element(),
        )
    }
}

/// Puts the composer, todo list and history in what `state` shows.
fn set_up(panel: &Panel, state: &str, theme: &Theme, cx: &mut Context<Gallery>) {
    let running = matches!(
        state,
        "working"
            | "permission"
            | "permission-pending"
            | "permission-project"
            | "question"
            | "todo-queue"
            | "steer-boundary"
            | "steer-native"
            | "approval-open"
    );
    // Send shows beside Stop only when the agent takes a message mid-turn.
    let steer = match state {
        "steer-boundary" => SteerMode::AtBoundary,
        "steer-native" => SteerMode::Native,
        _ => SteerMode::None,
    };
    panel.composer.update(cx, |composer, cx| {
        composer.set_theme(*theme, cx);
        composer.set_running(running, cx);
        composer.set_steer_mode(steer, cx);
        composer.clear_attachments(cx);
        composer.set_text("", cx);
        composer.set_model(
            (state != "not-set-up").then(|| ModelChoice {
                label: "Sonnet 4.5".into(),
                on_open: Rc::new(|_, _, _| {}),
            }),
            cx,
        );
        // The Tools popover and the approval selector after the model, as the panel shows
        // them; the last two states open them.
        let tools = gallery_tools();
        let on = tools.enabled_tools().len();
        let total = studio_engine::agent_tools::TOOL_NAMES.len();
        composer.set_controls(
            vec![
                ComposerControl {
                    id: "composer-tools".into(),
                    icon: Icon::Hammer,
                    label: Some(format!("{on}/{total}").into()),
                    name: format!("Tools: {on} of {total} on").into(),
                    open: state == "tools-open",
                    on_open: Rc::new(|_, _, _| {}),
                },
                ComposerControl {
                    id: "composer-approval".into(),
                    icon: Icon::ShieldQuestion,
                    label: Some(policy_short_label(GALLERY_POLICY).into()),
                    name: policy_name(GALLERY_POLICY).into(),
                    open: state == "approval-open",
                    on_open: Rc::new(|_, _, _| {}),
                },
            ],
            cx,
        );
        if state == "todo-queue" {
            composer.add_attachment(AttachmentDraft::file("scenes/outro.rs"), cx);
            composer.set_text("Also make the outro match", cx);
        }
        if matches!(state, "steer-boundary" | "steer-native") {
            composer.set_text("Keep the title's colour as it is", cx);
        }
        if state == "idle-image" {
            composer.add_attachment(
                AttachmentDraft::image("reference.png", Arc::new(reference_image())),
                cx,
            );
            composer.set_text("Make the title card look like this", cx);
        }
    });
    panel.todo.update(cx, |todo, cx| {
        let items = if state == "todo-queue" {
            vec![
                TodoItem::new("Read the intro scene", TodoStatus::Completed),
                TodoItem::new("Scale the title to 1.4×", TodoStatus::Completed),
                TodoItem::new("Slow the fade to 0.6 s", TodoStatus::InProgress),
                TodoItem::new(
                    "Build the preview and compare the first and last frames of the intro",
                    TodoStatus::Pending,
                ),
            ]
        } else {
            Vec::new()
        };
        todo.set_plan("fixture", items, cx);
        todo.set_live(state == "todo-queue", cx);
    });
    panel.history.update(cx, |history, cx| {
        let now = 1_800_000_000;
        let entry = |id: &str, title: &str, ago: u64, outcome: Option<OutcomeKind>| {
            let mut entry = SessionEntry::new(id, title);
            entry.agent = Some("Claude Code".into());
            entry.updated_unix = now - ago;
            entry.outcome = outcome;
            entry
        };
        let mut current = entry("s1", "Make the intro bigger", 60, None);
        current.current = true;
        history.set_entries(
            vec![
                current,
                entry(
                    "s2",
                    "Render the outro at 4K",
                    3_600,
                    Some(OutcomeKind::Failed),
                ),
                entry(
                    "s3",
                    "Add a title card with the episode number and the guest's name",
                    86_400,
                    Some(OutcomeKind::Accepted),
                ),
                entry(
                    "s4",
                    "Swap the music bed",
                    2 * 86_400,
                    Some(OutcomeKind::AwaitingReview),
                ),
                entry(
                    "s5",
                    "Tighten the captions",
                    5 * 86_400,
                    Some(OutcomeKind::Cancelled),
                ),
                entry(
                    "s6",
                    "Try a darker theme",
                    9 * 86_400,
                    Some(OutcomeKind::Undone),
                ),
            ],
            now,
            cx,
        );
    });
}

/// One panel at `width`: header, status line, transcript and dock, or History.
fn render_panel(theme: &Theme, state: &str, ix: usize, width: f32, panel: &Panel) -> AnyElement {
    let colors = theme.colors;
    let history = state == "history";
    let tabs = SegmentedControl::new(
        theme,
        "agent-tab",
        vec![
            SegmentOption::new("Chat", !history),
            SegmentOption::new("History", history),
            SegmentOption::new("Changes", false),
        ],
    );
    let header = PanelHeader::new(theme, tabs)
        .action(
            IconButton::new(theme, "agent-new-chat", Icon::Plus.path())
                .size(IconButtonSize::Md)
                .emphasis(Emphasis::Ghost)
                .label("New chat"),
        )
        .action(
            IconButton::new(theme, "agent-open-setup", Icon::Settings.path())
                .size(IconButtonSize::Md)
                .emphasis(Emphasis::Ghost)
                .label("Agent settings"),
        );
    let body = if history {
        div()
            .flex_1()
            .min_h_0()
            .px(px(inset::INSET_X))
            .py(px(inset::INSET_Y))
            .child(panel.history.clone())
            .into_any_element()
    } else {
        chat(theme, state, ix, panel)
    };
    Card::raised(theme)
        .id(("agent-fixture", ix))
        .w(px(width))
        .h(px(PANEL_H))
        .flex_none()
        .overflow_hidden()
        .flex()
        .flex_col()
        .text_color(colors.text_1)
        .child(
            text_style(div(), TextStyle::Small)
                .px(px(METRICS.space_3))
                .pt(px(METRICS.space_2))
                .text_color(colors.text_3)
                .child(format!("{state} · {width} px")),
        )
        .child(header)
        .child(body)
        .into_any_element()
}

/// The Chat tab: status line, transcript and dock.
fn chat(theme: &Theme, state: &str, ix: usize, panel: &Panel) -> AnyElement {
    let colors = theme.colors;
    let base = (ix as u64 + 1) * 100;
    let (tone, summary) = match state {
        "empty" | "code" | "idle-image" => (Tone::Success, "Claude Code · ready"),
        "not-set-up" => (Tone::Warning, "Agent not set up"),
        "failed" => (Tone::Danger, "Failed: Render the outro at 4K"),
        "changes" => (Tone::Info, "Ready for review: Make the intro bigger"),
        "permission" | "permission-pending" | "permission-project" => (
            Tone::Warning,
            "Waiting for permission: Make the intro bigger",
        ),
        "question" => (Tone::Warning, "Waiting for an answer: Add a title card"),
        _ => (Tone::Info, "Working: Make the intro bigger"),
    };
    let status = StatusLine::new(theme, tone, summary).trailing(Badge::new("Asks every time"));
    let mut top = div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(METRICS.space_2))
        .px(px(status::PAD_X))
        .pt(px(status::PAD_Y))
        .child(status);
    if state == "not-set-up" {
        top = top.child(
            ActionCard::new(
                theme,
                "agent-setup-guide",
                Tone::Warning,
                "Agent not set up",
            )
            .body("Choose an agent in Settings › Agent.")
            .action(
                Button::new(theme, "agent-setup-start")
                    .label("Set up agent")
                    .emphasis(Emphasis::Primary),
            ),
        );
    }

    let mut rows: Vec<AnyElement> = Vec::new();
    let user =
        |id: u64, text: &str| UserMessage::new(theme, base + id, text.to_owned()).meta("2:13 PM");
    let answer = |id: u64, text: &str| {
        AssistantMessage::new(
            theme,
            base + id,
            MarkdownRow::new(&agent_row(base + id, text), Voice::Answer),
        )
        .copy(text.to_owned())
        .meta("2:14 PM")
    };
    match state {
        "empty" | "idle-image" => rows.push(new_chat(theme)),
        "working" | "permission" | "permission-pending" | "permission-project" | "todo-queue"
        | "steer-boundary" | "steer-native" => {
            rows.push(
                user(1, "Make the intro bigger and slow the fade a little.").into_any_element(),
            );
            rows.push(
                answer(
                    2,
                    "Sure. I'll scale the title to **1.4×** and stretch the fade to `0.6 s`.",
                )
                .into_any_element(),
            );
            let open = state == "working";
            let mut counts = ActivityCounts::default();
            counts.add_tool(Some("read"), false);
            counts.add_tool(Some("edit"), false);
            counts.add_tool(Some("edit"), false);
            counts.add_tool(Some("execute"), false);
            counts.thoughts += usize::from(open);
            rows.push(
                ActivityHeader::new(theme, ("activity", base as usize), summary_parts(&counts))
                    .open(open)
                    .into_any_element(),
            );
            if open {
                // Reasoning opened by the reader: its row, then the text behind a rule, laid
                // out like `ConversationPanel::thought_row`.
                rows.push(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            ActivityRow::new(
                                theme,
                                ("thought", base as usize + 7),
                                Icon::Brain,
                                "Thought",
                                "The title is static markup, so only the wrapper scales.",
                            )
                            .on_toggle(true, |_, _| {}),
                        )
                        .child(
                            div()
                                .pl(px(activity::NESTED_INSET + METRICS.space_2))
                                .pr(px(METRICS.space_2))
                                .pb(px(METRICS.space_1))
                                .child(MarkdownRow::new(
                                    &thought_row(base + 7, REASONING),
                                    Voice::Reasoning,
                                )),
                        )
                        .into_any_element(),
                );
                rows.push(tool(
                    theme,
                    base + 3,
                    Icon::Eye,
                    "Read",
                    "scenes/intro.rs",
                    ActivityState::Done,
                ));
                rows.push(tool(
                    theme,
                    base + 4,
                    Icon::File,
                    "Edit",
                    "scenes/intro.rs",
                    ActivityState::Done,
                ));
                rows.push(tool(
                    theme,
                    base + 5,
                    Icon::File,
                    "Edit",
                    "src/theme.rs",
                    ActivityState::Done,
                ));
                rows.push(tool(
                    theme,
                    base + 6,
                    Icon::Hammer,
                    "Run",
                    "cargo build --release -p intro-video",
                    ActivityState::Running,
                ));
            }
            rows.push(
                WorkingTrailer::new(theme, "Working")
                    .elapsed("12s")
                    .into_any_element(),
            );
        }
        "failed" => {
            rows.push(user(1, "Render the outro at 4K.").into_any_element());
            let mut counts = ActivityCounts::default();
            counts.add_tool(Some("read"), false);
            counts.add_tool(Some("execute"), false);
            counts.add_tool(Some("execute"), true);
            rows.push(
                ActivityHeader::new(theme, ("activity", base as usize), summary_parts(&counts))
                    .open(true)
                    .into_any_element(),
            );
            rows.push(tool(
                theme,
                base + 2,
                Icon::Eye,
                "Read",
                "scenes/outro.rs",
                ActivityState::Done,
            ));
            rows.push(tool(
                theme,
                base + 3,
                Icon::Hammer,
                "Run",
                "cargo build",
                ActivityState::Done,
            ));
            rows.push(
                ActivityRow::new(
                    theme,
                    ("tool", base as usize + 4),
                    Icon::Hammer,
                    "Run",
                    "fframes render --size 3840x2160 -o out/outro.mp4",
                )
                .state(ActivityState::Failed)
                .trailing(
                    text_style(div(), TextStyle::Small)
                        .text_color(colors.danger)
                        .child("exit 101"),
                )
                .into_any_element(),
            );
            let detail = "The encoder stopped: no space left on the disk.\nNext: free some space and try again.\nDuring render · code render_failed";
            rows.push(
                ActionCard::new(theme, ("error", ix), Tone::Danger, "The render failed")
                    .body(detail)
                    .copy(detail)
                    .into_any_element(),
            );
        }
        "changes" => {
            rows.push(
                user(1, "Make the intro bigger and slow the fade a little.").into_any_element(),
            );
            let mut counts = ActivityCounts::default();
            counts.add_tool(Some("read"), false);
            counts.add_tool(Some("edit"), false);
            counts.add_tool(Some("edit"), false);
            counts.add_tool(Some("execute"), false);
            rows.push(
                ActivityHeader::new(theme, ("activity", base as usize), summary_parts(&counts))
                    .into_any_element(),
            );
            rows.push(
                answer(
                    2,
                    "Done: the title scales to **1.4×** and the fade takes `0.6 s`. The build passed.",
                )
                .into_any_element(),
            );
            rows.push(
                ChangesCard::new(
                    theme,
                    7,
                    vec![
                        ChangedFile {
                            mark: ChangeMark::Modified,
                            path: "scenes/intro.rs".into(),
                        },
                        ChangedFile {
                            mark: ChangeMark::Added,
                            path: "src/theme.rs".into(),
                        },
                        ChangedFile {
                            mark: ChangeMark::Modified,
                            path: "src/scenes/mod.rs".into(),
                        },
                        ChangedFile {
                            mark: ChangeMark::Deleted,
                            path: "assets/old-title.svg".into(),
                        },
                        ChangedFile {
                            mark: ChangeMark::Mode,
                            path: "scripts/render.sh".into(),
                        },
                    ],
                )
                .action(Button::new(theme, ("review", ix)).label("Review"))
                .into_any_element(),
            );
        }
        "question" => {
            rows.push(user(1, "Add a title card.").into_any_element());
            rows.push(
                answer(2, "I can add one before the intro. One thing first:").into_any_element(),
            );
        }
        "code" => {
            rows.push(
                user(1, "Show me how the intro scene animates the title.").into_any_element(),
            );
            rows.push(answer(2, CODE_ANSWER).into_any_element());
        }
        _ => {}
    }
    let mut transcript = div()
        .id(("agent-transcript", ix))
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(inset::ROW_GAP))
        .px(px(inset::INSET_X))
        .py(px(inset::INSET_Y))
        .children(rows);
    if state == "code" {
        transcript = transcript.child(div().flex().justify_center().child(JumpPill::new(
            theme,
            ("agent-jump-latest", ix),
            2,
        )));
    }

    let mut cards: Vec<AnyElement> = Vec::new();
    if state == "permission" {
        // Refusals first, every allowing choice primary, as `ConversationPanel` orders them.
        cards.push(
            ActionCard::new(
                theme,
                ("perm", ix),
                Tone::Info,
                "Run cargo build --release?",
            )
            .permission()
            .body("The agent asks for permission to go on.")
            .action(Button::new(theme, ("deny", ix)).label("Deny"))
            .action(
                Button::new(theme, ("always", ix))
                    .label("Always allow")
                    .emphasis(Emphasis::Primary),
            )
            .action(
                Button::new(theme, ("allow", ix))
                    .label("Allow once")
                    .emphasis(Emphasis::Primary),
            )
            .into_any_element(),
        );
    }
    if state == "permission-pending" {
        // Answered, waiting for the agent to take it: no buttons until it does.
        cards.push(
            ActionCard::new(
                theme,
                ("perm", ix),
                Tone::Info,
                "Run cargo build --release?",
            )
            .permission()
            .body("Answer sent…")
            .into_any_element(),
        );
    }
    if state == "permission-project" {
        // A request a safe rule describes: "Always in this project" between the refusals
        // and the agent's allows, as `ConversationPanel` places it.
        cards.push(
            ActionCard::new(theme, ("perm", ix), Tone::Info, "Edit src/intro.rs")
                .permission()
                .body("The agent asks for permission to go on.")
                .action(Button::new(theme, ("deny", ix)).label("Deny"))
                .action(Button::new(theme, ("project", ix)).label("Always in this project"))
                .action(
                    Button::new(theme, ("allow", ix))
                        .label("Allow")
                        .emphasis(Emphasis::Primary),
                )
                .into_any_element(),
        );
    }
    if state == "question" {
        let options = [
            ("Episode number", Some("“Episode 12” above the title")),
            ("Guest name", Some("The guest's name under the title")),
            ("Both", None),
        ];
        let mut block = div().flex().flex_col().gap(px(METRICS.space_2)).child(
            text_style(div(), TextStyle::Label)
                .text_color(colors.text_1)
                .child("What should the title card show?"),
        );
        for (k, (label, description)) in options.into_iter().enumerate() {
            block = block.child(RadioCards::new(
                theme,
                format!("question-0-0-opt-{k}-{ix}"),
                vec![RadioCard::new(label, description)],
                (k == 2).then_some(0),
            ));
        }
        block = block.child(
            TextInput::new(theme, ("question-text", ix), &panel.answer).height(px(ANSWER_FIELD_H)),
        );
        cards.push(
            ActionCard::new(
                theme,
                ("question", ix),
                Tone::Info,
                "The agent has a question",
            )
            .question()
            .child(block)
            .action(Button::new(theme, ("q-cancel", ix)).label("Cancel"))
            .action(
                Button::new(theme, ("q-send", ix))
                    .label("Send answers")
                    .emphasis(Emphasis::Primary),
            )
            .into_any_element(),
        );
    }
    let tray = (state == "todo-queue").then(|| {
        let queue = div()
            .border_t_1()
            .border_color(colors.line)
            .child(TrayHeader::new(theme, ("agent-queue", ix), Icon::Clock, "Queued").count("2"))
            .child(
                TrayRow::new(
                    theme,
                    ("queue-item", ix * 10),
                    "Then render a 10 s draft of the intro and compare it with the last render",
                )
                .single_line()
                .mark(icon(Icon::Clock, STATUS_MARK, colors.text_3))
                .note("Intro", colors.text_3),
            )
            .child(
                TrayRow::new(
                    theme,
                    ("queue-item", ix * 10 + 1),
                    "Retime the outro to the new music",
                )
                .single_line()
                .mark(icon(Icon::Clock, STATUS_MARK, colors.text_3))
                .note("Outro · will be refused", colors.warning),
            );
        tray_frame(theme).child(panel.todo.clone()).child(queue)
    });
    let dock = div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(METRICS.space_2))
        .px(px(METRICS.space_3))
        .pt(px(METRICS.space_2))
        .pb(px(METRICS.space_3))
        .children(cards)
        .child(div().flex().flex_col().children(tray).child(
            // The open popover rises from the composer's top edge, as it does from
            // its trigger in the panel.
            div().relative().child(panel.composer.clone()).children(
                composer_popover(theme, state).map(|popover| {
                    div()
                        .absolute()
                        .top_0()
                        .left(px(METRICS.space_4))
                        .child(popover)
                }),
            ),
        ));
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(top)
        .child(transcript)
        .child(dock)
        .into_any_element()
}

/// The composer's open Tools popover or approval selector, in the states that open them.
fn composer_popover(theme: &Theme, state: &str) -> Option<AnyElement> {
    match state {
        "tools-open" => Some(
            ToolsPopover::new(theme, gallery_tools(), &tools_menu(&gallery_tools(), None))
                .into_any_element(),
        ),
        "approval-open" => Some(
            ApprovalPopover::new(theme, GALLERY_POLICY, &approval_menu(GALLERY_POLICY, None))
                .running(true)
                .into_any_element(),
        ),
        _ => None,
    }
}

/// The new-chat state: what to do, and suggestions to start from.
fn new_chat(theme: &Theme) -> AnyElement {
    panel_home(
        theme,
        "What should we change?",
        "Describe an edit; the agent changes the project and you review it.",
    )
    .flex_none()
    .child(
        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(px(METRICS.space_2))
            .children(
                [
                    "Add a title card",
                    "Slow the intro fade",
                    "Match the outro to the music",
                ]
                .iter()
                .enumerate()
                .map(|(k, text)| Chip::new(theme, ("suggestion", k), *text)),
            ),
    )
    .into_any_element()
}

fn tool(
    theme: &Theme,
    id: u64,
    glyph: Icon,
    verb: &str,
    subject: &str,
    state: ActivityState,
) -> AnyElement {
    ActivityRow::new(
        theme,
        ("tool", id as usize),
        glyph,
        verb.to_owned(),
        subject.to_owned(),
    )
    .state(state)
    .into_any_element()
}

/// A pasted reference frame for the image chip: a small PNG made here, so the gallery needs
/// no image file.
fn reference_image() -> gpui::Image {
    let frame = image::RgbaImage::from_fn(160, 90, |x, y| {
        let title = (40..120).contains(&x) && (36..54).contains(&y);
        if title {
            image::Rgba([250, 250, 250, 255])
        } else {
            image::Rgba([40 + (x / 2) as u8, 60 + y as u8, 140, 255])
        }
    });
    let mut png = Vec::new();
    frame
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("a PNG encodes into memory");
    gpui::Image::from_bytes(gpui::ImageFormat::Png, png)
}

fn agent_row(id: u64, text: &str) -> Arc<Row> {
    Arc::new(Row {
        id: RowId(id),
        task: None,
        at_unix: 0,
        kind: RowKind::Agent {
            text: text.to_owned(),
            turn: 1,
            streaming: false,
        },
    })
}

fn thought_row(id: u64, text: &str) -> Arc<Row> {
    Arc::new(Row {
        id: RowId(id),
        task: None,
        at_unix: 0,
        kind: RowKind::Thought {
            text: text.to_owned(),
            turn: 1,
            streaming: false,
        },
    })
}

const REASONING: &str = "The title is static markup, so only the wrapper scales.\n\
Scale it to 1.4 with the spring the intro already uses, then stretch the fade to 0.6 s \
so the two settle together.";

const CODE_ANSWER: &str = r##"The title grows with a spring and the fade runs on its own timeline:

```rust
impl Scene for Intro {
    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(3.0)
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let scale = frame.animate(&fframes::timeline!(
            at 0.0, animate 1.0_f32 => 1.4, Easing::Spring { mass: 1.0, stiffness: 300.0, damping: 26.0 },
        ));
        let opacity = frame.animate(&fframes::timeline!(at 0.0 => 0.6, animate 0.0_f32 => 1.0, Easing::EaseOut));
        // The title is static markup, so the renderer caches it; only the wrapper animates.
        fframes::svgr!(
            <g transform={Transform::scale(scale, scale)} opacity={opacity}>
                <text x="100" y="300" font-family="DM Sans" font-size="150" fill="#fff">
                    "Hello"
                </text>
            </g>
        )
    }
}
```

`scale` settles at **1.4** after about 0.4 s; `opacity` reaches 1 at 0.6 s."##;
