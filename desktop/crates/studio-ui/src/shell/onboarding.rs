//! First-run setup, shown in the window in place of the workspace as a centred page of its
//! own: the theme, the managed SDK, a first project, the agent and how the agent's permission
//! requests are answered, one step at a time under a "Step N of M" bar. Every step reuses
//! the control the workspace has for it (the Appearance section, the Project drawer's SDK
//! actions, the empty state's project actions, the Agent settings page). The agent comes
//! after the project because an agent is set up through an open project's workflow; the
//! choice is saved for every project.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use studio_engine::app_settings::{AppSettings, needs_onboarding};
use studio_proto::PermissionPolicy;
use studio_theme::{METRICS, TextStyle, text_style};

use super::StudioShell;
use crate::kit::{
    Card, Emphasis, PanelSection, Progress, RadioCard, RadioCards, SettingRow, selectable,
    setting_card,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Welcome,
    Appearance,
    Sdk,
    Project,
    Agent,
    Permissions,
}

impl Step {
    pub(super) const ALL: [Step; 6] = [
        Step::Welcome,
        Step::Appearance,
        Step::Sdk,
        Step::Project,
        Step::Agent,
        Step::Permissions,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }

    /// The step's id (telemetry, and the step's content `onboarding-<name>`).
    pub(super) fn name(self) -> &'static str {
        match self {
            Step::Welcome => "welcome",
            Step::Appearance => "appearance",
            Step::Sdk => "sdk",
            Step::Project => "project",
            Step::Agent => "agent",
            Step::Permissions => "permissions",
        }
    }

    /// The step's title and the text under it.
    fn heading(self) -> (&'static str, &'static str) {
        match self {
            Step::Welcome => (
                "Welcome to fframes Studio",
                "Studio turns a Rust project into a video you can preview, change with an AI \
                 agent and export. This setup takes a minute: the look, the SDK that builds \
                 your videos, a first video, an agent, and how the agent asks for permission. \
                 Every step can be skipped and changed later in Settings.",
            ),
            Step::Appearance => (
                "How Studio looks",
                "Pick a theme and how much the interface moves. Change it anytime in Settings → General.",
            ),
            Step::Sdk => (
                "Video SDK",
                "Studio builds and previews videos with the fframes SDK. Install it once; \
                 every project uses it.",
            ),
            Step::Project => (
                "Your first video",
                "Create a new video project, open one, or import a Rust fframes project. \
                 The agent is set up for the open project next.",
            ),
            Step::Agent => (
                "AI agent",
                "Pick the coding agent that edits your videos. Detect agents only looks for \
                 installed agents; nothing starts until you select one. The choice is saved \
                 for every project.",
            ),
            Step::Permissions => (
                "Agent permissions",
                "Choose how the agent's requests are answered. Change it anytime in \
                 Settings → Agent.",
            ),
        }
    }
}

/// The setup while it is open.
pub(super) struct Onboarding {
    pub(super) step: Step,
    /// The permission policy chosen on the Permissions step, saved by Finish.
    pub(super) policy: PermissionPolicy,
}

/// The choices of the Permissions step and the Settings row, in this order.
const POLICY_TEXT: [(PermissionPolicy, &str); 3] = [
    (
        PermissionPolicy::AskEveryTime,
        "The agent stops and asks before every action.",
    ),
    (
        PermissionPolicy::AllowInWorkingCopy,
        "The agent reads and edits files in its private copy of your project without asking. \
         Running commands or using the network still asks. You review or undo every change.",
    ),
    (
        PermissionPolicy::AllowEverything,
        "The agent also runs commands, such as cargo check, and uses the network without \
         asking. Commands run on this computer and are not sandboxed.",
    ),
];

impl StudioShell {
    /// Decides once, when the app's storage is known, whether the setup opens, and reads
    /// the saved permission policy. An install that already has an SDK and an agent and
    /// never recorded the setup is recorded as set up instead.
    pub(super) fn check_onboarding(&mut self, cx: &mut Context<Self>) {
        if self.setup_checked {
            return;
        }
        let Some(paths) = self.presentation.paths.clone() else {
            return;
        };
        self.setup_checked = true;
        let settings = AppSettings::load(&paths);
        self.permission_policy = settings.permission_policy;
        self.onboarding_completed = settings.onboarding_completed;
        self.update_check_disabled = settings.update_check_disabled;
        let agent_selected = studio_engine::providers::host::load_settings(&paths)
            .ok()
            .flatten()
            .is_some();
        let sdk_ready = self.presentation.agent_sdk.is_some();
        if needs_onboarding(&settings, sdk_ready, agent_selected) {
            self.onboarding = Some(Onboarding {
                step: Step::Welcome,
                // A first setup suggests the working-copy policy; it is saved only by Finish.
                policy: if settings.onboarding_completed.is_none() {
                    PermissionPolicy::AllowInWorkingCopy
                } else {
                    settings.permission_policy
                },
            });
        } else if settings.onboarding_completed.is_none() {
            self.onboarding_completed = Some(true);
            self.save_setup_choices(cx);
        }
        self.apply_permission_policy(cx);
        cx.notify();
    }

    /// Settings → Run setup again: opens the setup at its first step with today's choices.
    pub(super) fn run_setup_again(&mut self, cx: &mut Context<Self>) {
        self.onboarding = Some(Onboarding {
            step: Step::Welcome,
            policy: self.permission_policy,
        });
        self.close_settings(cx);
    }

    fn go_to_step(&mut self, step: Step, cx: &mut Context<Self>) {
        if let Some(onboarding) = &mut self.onboarding {
            onboarding.step = step;
            cx.notify();
        }
    }

    /// Finish: records the setup as done with the permission policy it chose.
    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        let Some(onboarding) = self.onboarding.take() else {
            return;
        };
        self.permission_policy = onboarding.policy;
        self.onboarding_completed = Some(true);
        self.apply_permission_policy(cx);
        self.save_setup_choices(cx);
        cx.notify();
    }

    /// Skip setup: records the setup as done and keeps the saved permission policy.
    fn skip_onboarding(&mut self, cx: &mut Context<Self>) {
        if self.onboarding.take().is_none() {
            return;
        }
        self.onboarding_completed = Some(true);
        self.save_setup_choices(cx);
        cx.notify();
    }

    /// Settings → Agent: a new permission policy, for the next task.
    pub(super) fn choose_permission_policy(
        &mut self,
        policy: PermissionPolicy,
        cx: &mut Context<Self>,
    ) {
        if self.permission_policy == policy {
            return;
        }
        self.permission_policy = policy;
        self.apply_permission_policy(cx);
        self.save_setup_choices(cx);
        cx.notify();
    }

    /// Hands the policy to the open project's agent and its panel.
    pub(super) fn apply_permission_policy(&mut self, cx: &mut Context<Self>) {
        if let Some(workflow) = &self.agent.workflow {
            let _ = workflow.set_permission_policy(self.permission_policy);
        }
        let policy = self.permission_policy;
        self.panel
            .update(cx, |panel, cx| panel.set_permission_policy(policy, cx));
    }

    /// The three policies as radio cards; `on_choose` gets the one clicked.
    pub(super) fn permission_choice(
        &self,
        selected: PermissionPolicy,
        on_choose: impl Fn(&mut Self, PermissionPolicy, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        RadioCards::new(
            &self.theme,
            "permission-policy",
            POLICY_TEXT
                .iter()
                .map(|(policy, text)| RadioCard::new(policy.label(), Some(text)))
                .collect(),
            POLICY_TEXT
                .iter()
                .position(|(policy, _)| *policy == selected),
        )
        .on_select(move |index, _, cx| {
            if let Some((policy, _)) = POLICY_TEXT.get(*index) {
                entity.update(cx, |shell, cx| on_choose(shell, *policy, cx));
            }
        })
        .into_any_element()
    }

    /// The setup on step `step` (0 Welcome … 5 Permissions), drawn with `theme`. For the UI
    /// gallery's captures.
    pub fn render_setup_fixture(
        &mut self,
        step: usize,
        theme: studio_theme::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let policy = self
            .onboarding
            .as_ref()
            .map_or(self.permission_policy, |onboarding| onboarding.policy);
        self.onboarding = Some(Onboarding {
            step: Step::ALL.get(step).copied().unwrap_or(Step::Welcome),
            policy,
        });
        let main = std::mem::replace(&mut self.theme, theme);
        let page = self.render_onboarding(cx);
        self.theme = main;
        page
    }

    /// The setup as its own page: a centred card, at most `start_max_w` wide, with the step
    /// indicator, the step and the footer (Skip, Back, Next).
    pub(super) fn render_onboarding(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let colors = theme.colors;
        let Some(onboarding) = &self.onboarding else {
            return div().into_any_element();
        };
        let step = onboarding.step;
        let number = step.index() + 1;
        let count = Step::ALL.len();
        let last = step == Step::Permissions;
        let previous = Step::ALL.get(step.index().wrapping_sub(1)).copied();
        let next = Step::ALL.get(step.index() + 1).copied();
        let (title, text) = step.heading();

        let indicator = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .child(
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_3)
                    .child(format!("Step {number} of {count}")),
            )
            .child(Progress::linear(&theme, number as f32 / count as f32));

        let heading = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(
                text_style(div(), TextStyle::Display)
                    .text_color(colors.text_1)
                    .child(selectable(title)),
            )
            .child(
                text_style(div(), TextStyle::Body)
                    .text_color(colors.text_2)
                    .child(selectable(text)),
            );

        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(
                self.action_button(
                    "onboarding-skip",
                    "Skip setup",
                    true,
                    false,
                    None,
                    cx,
                    |shell, cx| shell.skip_onboarding(cx),
                )
                .emphasis(Emphasis::Ghost),
            )
            .child(div().flex_1())
            .child(self.action_button(
                "onboarding-back",
                "Back",
                previous.is_some(),
                false,
                None,
                cx,
                move |shell, cx| {
                    if let Some(previous) = previous {
                        shell.go_to_step(previous, cx);
                    }
                },
            ))
            .child(self.action_button(
                "onboarding-next",
                if last { "Finish" } else { "Next" },
                true,
                true,
                None,
                cx,
                move |shell, cx| match next {
                    Some(next) => shell.go_to_step(next, cx),
                    None => shell.finish_onboarding(cx),
                },
            ));

        // A definite width (narrower windows shrink it): with `w_full` and `max_w` the
        // wrapped text is measured at the unclamped width and the footer spills out.
        let card = Card::raised(&theme)
            .flex_none()
            .w(px(METRICS.layout.start_max_w))
            .max_w_full()
            .mx_auto()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_6))
            .p(px(METRICS.space_6))
            .child(indicator)
            .child(heading)
            .child(
                div()
                    .id(SharedString::from(format!("onboarding-{}", step.name())))
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_6))
                    .children(self.onboarding_page(step, onboarding.policy, cx)),
            )
            .child(footer);

        div()
            .id("onboarding")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .py(px(METRICS.space_6))
            .child(card)
            .into_any_element()
    }

    /// What a step shows under its heading: its sections.
    fn onboarding_page(
        &self,
        step: Step,
        policy: PermissionPolicy,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let theme = self.theme;
        match step {
            Step::Welcome => Vec::new(),
            Step::Appearance => vec![self.render_appearance(cx)],
            Step::Sdk => {
                let mut rows = vec![
                    setting_card(&theme)
                        .child(self.sdk_controls(cx))
                        .into_any_element(),
                ];
                if self.presentation.agent_sdk.is_none() {
                    rows.push(
                        note(
                            &theme,
                            "Without the SDK, Studio cannot build previews or export. You can \
                             install it later from Settings → SDK.",
                        )
                        .into_any_element(),
                    );
                }
                vec![
                    PanelSection::new(&theme, "Managed SDK")
                        .children(rows)
                        .into_any_element(),
                ]
            }
            Step::Project => {
                let mut row = SettingRow::new(
                    &theme,
                    if self.presentation.project.is_some() {
                        "Open project"
                    } else {
                        "No project open"
                    },
                );
                if let Some(open) = &self.presentation.project {
                    row = row.description(format!(
                        "{} · {}",
                        open.name,
                        studio_bootstrap::external_path(&open.root).display()
                    ));
                }
                let row = row
                    .below(self.project_actions(false, cx))
                    .into_any_element();
                vec![
                    PanelSection::new(&theme, "Project")
                        .children([row])
                        .into_any_element(),
                ]
            }
            Step::Agent => vec![self.setup_page.clone().into_any_element()],
            Step::Permissions => vec![
                PanelSection::new(&theme, "Permission requests")
                    .children([self.permission_choice(
                        policy,
                        |shell, policy, cx| {
                            if let Some(onboarding) = &mut shell.onboarding {
                                onboarding.policy = policy;
                                cx.notify();
                            }
                        },
                        cx,
                    )])
                    .into_any_element(),
            ],
        }
    }
}

/// A line under a step's controls: `Small` in `text_2`.
fn note(theme: &studio_theme::Theme, text: impl Into<SharedString>) -> gpui::Div {
    text_style(div(), TextStyle::Small)
        .text_color(theme.colors.text_2)
        .child(selectable(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_put_the_project_before_the_agent_and_end_with_permissions() {
        let names: Vec<_> = Step::ALL.iter().map(|step| step.name()).collect();
        assert_eq!(
            names,
            [
                "welcome",
                "appearance",
                "sdk",
                "project",
                "agent",
                "permissions"
            ]
        );
        let policies: Vec<_> = POLICY_TEXT.iter().map(|(policy, _)| *policy).collect();
        assert_eq!(policies, PermissionPolicy::ALL);
    }
}
