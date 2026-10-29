//! The settings frame, shown in a window of its own as in Cap
//! ([`SettingsWindow`](super::settings_window::SettingsWindow)) while the editor stays as it
//! was. The header's Settings button opens the window or brings it to the front. The
//! General page holds the theme, the audio output, setup again and updates; the Agent page
//! the review policy, the permissions and the agent panel's adapter setup; the SDK page the
//! managed SDK; the Presets page the project's style presets; the Export page the defaults
//! every export starts from.

use std::sync::atomic::{AtomicU64, Ordering};

use fframes_studio_protocol::export::ExportSettings;
use gpui::{Context, IntoElement, ParentElement, SharedString, Styled, div, px};
use studio_engine::export_options::{FORMATS, REDUCED_FPS, SCALES, format_label};
use studio_engine::services::audio_service::OutputDevice;
use studio_engine::{ReviewPolicy, app_settings::AppSettings, updates::AvailableUpdate};
use studio_proto::{AppTheme, MotionPreference};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::StudioShell;
use super::sections::box_probe;
use super::settings_window::{self, SettingsWindow};
use crate::kit::{
    FormField, Icon, PanelSection, RadioCard, RadioCards, SegmentOption, SegmentedControl,
    SettingsFrame, SettingsNavItem, Toggle, selectable,
};

/// The check for a newer Studio release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UpdateCheck {
    NotChecked,
    Checking,
    UpToDate,
    Available(AvailableUpdate),
    Failed(String),
}

/// The choices in the order the control shows them; option `i` is `theme-i`.
const THEMES: [(AppTheme, &str); 3] = [
    (AppTheme::System, "System"),
    (AppTheme::Light, "Light"),
    (AppTheme::Dark, "Dark"),
];

/// The motion choices in the order the control shows them; option `i` is `motion-i`.
const MOTIONS: [(MotionPreference, &str); 3] = [
    (MotionPreference::System, "System"),
    (MotionPreference::Reduced, "Reduced"),
    (MotionPreference::Full, "Full"),
];

/// A page of the settings frame, in the order of its nav column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsPage {
    #[default]
    General,
    Agent,
    Sdk,
    Presets,
    Export,
}

impl SettingsPage {
    const ALL: [SettingsPage; 5] = [
        SettingsPage::General,
        SettingsPage::Agent,
        SettingsPage::Sdk,
        SettingsPage::Presets,
        SettingsPage::Export,
    ];

    fn nav_item(self, selected: bool) -> SettingsNavItem {
        let (id, label, icon) = match self {
            SettingsPage::General => ("settings-general", "General", Icon::Settings),
            SettingsPage::Agent => ("settings-agent", "Agent", Icon::Bot),
            SettingsPage::Sdk => ("settings-sdk", "SDK", Icon::Package),
            SettingsPage::Presets => ("settings-presets", "Presets", Icon::Palette),
            SettingsPage::Export => ("settings-export", "Export", Icon::Download),
        };
        SettingsNavItem::new(id, label, icon, selected)
    }
}

/// The frame rates Settings → Export offers as the default: the video's own, or 30 where
/// it divides the video's.
const DEFAULT_FPS: [(Option<u32>, &str); 2] =
    [(None, "The video's own"), (Some(REDUCED_FPS), "30 fps")];

/// Saves the theme choices one at a time, in the order they were made: a save that a
/// later choice overtook is skipped.
#[derive(Default)]
pub(super) struct SettingsWriter {
    latest: AtomicU64,
    latest_motion: AtomicU64,
    latest_export: AtomicU64,
    latest_setup: AtomicU64,
    latest_update_check: AtomicU64,
    write: parking_lot::Mutex<()>,
}

impl SettingsWriter {
    pub(super) fn next_export_generation(&self) -> u64 {
        self.latest_export.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Runs `save` unless a later export choice was made meanwhile. Theme and export
    /// saves share the lock, so neither overwrites the other's field.
    pub(super) fn save_if_latest_export(
        &self,
        generation: u64,
        save: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let _write = self.write.lock();
        if self.latest_export.load(Ordering::Acquire) != generation {
            return Ok(());
        }
        save()
    }
}

impl StudioShell {
    /// Opens the settings window, or brings it to the front when it is open. Not while the
    /// setup shows: it draws the Agent page's panel too.
    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if self.onboarding.is_some() {
            return;
        }
        self.overflow_open = false;
        // The Export page shows them; a render cannot read the file.
        self.export_defaults();
        if let Some(window) = self.settings_window {
            // After this update too: activating fires the platform's focus callbacks.
            cx.defer(move |cx| settings_window::activate(window.into(), cx));
            cx.notify();
            return;
        }
        // After this update: a new window draws at once, and its pages update the shell.
        let shell = cx.entity();
        cx.defer(move |cx| {
            if shell.read(cx).settings_window.is_some() {
                return;
            }
            let window = SettingsWindow::open(shell.clone(), cx);
            shell.update(cx, |shell, cx| {
                if window.is_none() {
                    shell.error = Some("Cannot open the settings window".into());
                }
                shell.settings_window = window;
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Opens the settings window on `page`.
    pub(super) fn open_settings_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.settings_page = page;
        self.open_settings(cx);
    }

    /// Opens the settings window on the Agent page.
    pub(super) fn open_agent_setup(&mut self, cx: &mut Context<Self>) {
        self.open_settings_page(SettingsPage::Agent, cx);
    }

    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(window) = self.settings_window.take() {
            settings_window::close(window.into(), cx);
        }
        cx.notify();
    }

    /// The settings window, while it is open.
    pub fn settings_window(&self) -> Option<gpui::AnyWindowHandle> {
        self.settings_window.map(Into::into)
    }

    /// Applies `choice` to every window and saves it in the background.
    fn choose_theme(&mut self, choice: AppTheme, cx: &mut Context<Self>) {
        if studio_theme::current_preference(cx) == choice {
            return;
        }
        // Refreshes every window, which renders the cached sections again too.
        studio_theme::set_preference(choice, cx);
        let Some(paths) = self.presentation.paths.clone() else {
            self.error = Some("Cannot save the theme: the app's storage is unavailable".into());
            return;
        };
        let writer = self.settings_writer.clone();
        let generation = writer.latest.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { save_theme(&writer, generation, choice, &paths) })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Applies the motion `choice` to every window and saves it in the background, like
    /// the theme.
    fn choose_motion(&mut self, choice: MotionPreference, cx: &mut Context<Self>) {
        if studio_theme::current_motion(cx) == choice {
            return;
        }
        studio_theme::set_motion(choice, cx);
        let Some(paths) = self.presentation.paths.clone() else {
            self.error =
                Some("Cannot save the motion setting: the app's storage is unavailable".into());
            return;
        };
        let writer = self.settings_writer.clone();
        let generation = writer.latest_motion.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { save_motion(&writer, generation, choice, &paths) })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Saves the setup record and the permission policy in the background. Both fields are
    /// written from the shell's current values, so the latest save carries every choice and
    /// an overtaken one is skipped.
    pub(super) fn save_setup_choices(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.presentation.paths.clone() else {
            self.error = Some("Cannot save the setup: the app's storage is unavailable".into());
            return;
        };
        let (completed, policy) = (self.onboarding_completed, self.permission_policy);
        let writer = self.settings_writer.clone();
        let generation = writer.latest_setup.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _write = writer.write.lock();
                    if writer.latest_setup.load(Ordering::Acquire) != generation {
                        return Ok(());
                    }
                    let mut settings = AppSettings::load(&paths);
                    settings.onboarding_completed = completed;
                    settings.permission_policy = policy;
                    settings.save(&paths)
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Asks GitHub for a newer Studio in the background; the answer shows on the
    /// General page and, when there is one, as a notice under the header.
    pub fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if self.update == UpdateCheck::Checking {
            return;
        }
        self.update = UpdateCheck::Checking;
        self.update_dismissed = false;
        cx.notify();
        let version = self.app_version;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { studio_engine::updates::check_for_update(version) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.update = match result {
                    Ok(Some(update)) => UpdateCheck::Available(update),
                    Ok(None) => UpdateCheck::UpToDate,
                    Err(error) => UpdateCheck::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Turns the launch check on or off and saves it in the background.
    fn set_update_check(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update_check_disabled = !enabled;
        cx.notify();
        let Some(paths) = self.presentation.paths.clone() else {
            self.error = Some("Cannot save the setting: the app's storage is unavailable".into());
            return;
        };
        let disabled = self.update_check_disabled;
        let writer = self.settings_writer.clone();
        let generation = writer.latest_update_check.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _write = writer.write.lock();
                    if writer.latest_update_check.load(Ordering::Acquire) != generation {
                        return Ok(());
                    }
                    let mut settings = AppSettings::load(&paths);
                    settings.update_check_disabled = disabled;
                    settings.save(&paths)
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The Updates section: the launch check, its answer, Check now and Download.
    fn render_updates(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let toggle = Toggle::new(&theme, "update-check", !self.update_check_disabled)
            .label("Check for updates at launch")
            .on_change(move |enabled, _, cx| {
                entity.update(cx, |shell, cx| shell.set_update_check(enabled, cx))
            });
        let status = match &self.update {
            UpdateCheck::NotChecked => "Not checked yet".to_owned(),
            UpdateCheck::Checking => "Checking…".to_owned(),
            UpdateCheck::UpToDate => format!("fframes Studio {} is the latest", self.app_version),
            UpdateCheck::Available(update) => format!(
                "fframes Studio {} is available (this is {})",
                update.version, self.app_version
            ),
            UpdateCheck::Failed(error) => error.clone(),
        };
        let mut actions = div()
            .flex()
            .gap(px(METRICS.space_2))
            .child(self.action_button(
                "check-update",
                "Check now",
                self.update != UpdateCheck::Checking,
                false,
                None,
                cx,
                |shell, cx| shell.check_for_update(cx),
            ));
        if let UpdateCheck::Available(update) = &self.update {
            actions = actions.child(self.download_update_button(update.url.clone(), cx));
        }
        PanelSection::new(&theme, "Updates")
            .description(
                "Studio asks GitHub for a newer release. Nothing is downloaded or installed \
                 without you.",
            )
            .card()
            .child(FormField::row(
                &theme,
                "Check for updates at launch",
                toggle,
            ))
            .child(FormField::row(&theme, status, actions))
            .into_any_element()
    }

    /// Opens the release page of `url`, where the installer and the notes are.
    pub(super) fn download_update_button(
        &self,
        url: String,
        cx: &mut Context<Self>,
    ) -> crate::kit::Button {
        self.action_button(
            "download-update",
            "Download…",
            true,
            true,
            None,
            cx,
            move |_, cx| cx.open_url(&url),
        )
    }

    /// The settings window's frame, with `nav_header` above the nav rows.
    pub(super) fn render_settings(
        &self,
        nav_header: Option<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let page = match self.settings_page {
            SettingsPage::General => settings_page()
                .child(self.render_appearance(cx))
                .child(self.render_audio_output(cx))
                .child(
                    PanelSection::new(&theme, "Setup")
                        .description(
                            "Walk through the theme, SDK, first video, agent and permissions \
                             again.",
                        )
                        .card()
                        .child(FormField::row(
                            &theme,
                            "First-run setup",
                            self.action_button(
                                "run-setup-again",
                                "Run setup again",
                                true,
                                false,
                                None,
                                cx,
                                |shell, cx| shell.run_setup_again(cx),
                            ),
                        )),
                )
                .child(self.render_updates(cx))
                .into_any_element(),
            // The two short choices first: the provider setup below them runs long.
            SettingsPage::Agent => settings_page()
                .child(self.render_review_policy(cx))
                .child(
                    PanelSection::new(&theme, "Permissions")
                        .description(
                            "How the agent's permission requests are answered. A running task \
                             keeps the choice it started with.",
                        )
                        .child(self.permission_choice(
                            self.permission_policy,
                            |shell, policy, cx| shell.choose_permission_policy(policy, cx),
                            cx,
                        )),
                )
                .child(self.setup_page.clone())
                .into_any_element(),
            SettingsPage::Sdk => PanelSection::new(&theme, "Managed SDK")
                .description(
                    "Studio builds previews and exports with this SDK. Selecting an installed \
                     one applies to the open project.",
                )
                .card()
                .child(self.sdk_controls(cx))
                .into_any_element(),
            SettingsPage::Presets => self.preset_panel.clone().into_any_element(),
            SettingsPage::Export => self.render_export_defaults(cx),
        };
        let frame = SettingsFrame::new(
            &theme,
            "settings",
            SettingsPage::ALL
                .iter()
                .map(|page| page.nav_item(*page == self.settings_page))
                .collect(),
        )
        .on_select({
            let entity = entity.clone();
            move |index, _, cx| {
                if let Some(page) = SettingsPage::ALL.get(*index) {
                    entity.update(cx, |shell, cx| {
                        shell.settings_page = *page;
                        cx.notify();
                    });
                }
            }
        })
        .nav_header(nav_header)
        .children([page]);
        let frame = if self.qualifying {
            frame.on_item_bounds(move |index, bounds, cx| {
                if let Some(page) = SettingsPage::ALL.get(index) {
                    entity.update(cx, |shell, _| {
                        shell.button_bounds.insert(
                            page.nav_item(false).id.to_string(),
                            crate::probe::bounds_to_record(bounds),
                        );
                    });
                }
            })
        } else {
            frame
        };
        div()
            .relative()
            .size_full()
            .child(frame)
            .child(box_probe("settings", cx))
            .into_any_element()
    }

    /// The settings frame on page `page` (0 General … 4 Export), drawn with `theme` the way
    /// the settings window draws it. For the UI gallery's captures.
    pub fn render_settings_fixture(
        &mut self,
        page: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if let Some(page) = SettingsPage::ALL.get(page) {
            self.settings_page = *page;
        }
        self.export_defaults();
        let main = std::mem::replace(&mut self.theme, theme);
        let frame = self.render_settings(None, cx);
        self.theme = main;
        frame
    }

    /// A settings segmented control whose option `i` reports its bounds as `{id}-{i}`, and
    /// whose clicks call `on_select` with the option's index.
    fn measured_segments(
        &self,
        id: &'static str,
        options: Vec<SegmentOption>,
        on_select: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        let mut control = SegmentedControl::new(&self.theme, id, options).on_select({
            let entity = entity.clone();
            move |index, _, cx| entity.update(cx, |shell, cx| on_select(shell, *index, cx))
        });
        if self.qualifying {
            control = control.on_option_bounds(move |index, bounds, cx| {
                entity.update(cx, |shell, _| {
                    shell.button_bounds.insert(
                        format!("{id}-{index}"),
                        crate::probe::bounds_to_record(bounds),
                    );
                });
            });
        }
        control.into_any_element()
    }

    /// The General page's Audio output: the device the preview plays on and the choice of
    /// another, of the default again, or of none.
    fn render_audio_output(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        // The device opens with the preview's audio.
        let ready = self.displayed.is_some();
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(METRICS.space_2))
            .child(self.action_button(
                "next-audio-output",
                "Next output",
                ready,
                false,
                None,
                cx,
                |s, cx| s.cycle_output(cx),
            ))
            .child(self.action_button(
                "retry-audio-output",
                "Retry audio",
                ready,
                false,
                None,
                cx,
                |s, cx| s.change_output(OutputDevice::Default, cx),
            ))
            .child(self.action_button(
                "no-audio-output",
                "No audio",
                ready,
                false,
                None,
                cx,
                |s, cx| s.change_output(OutputDevice::Unavailable, cx),
            ));
        let status = if ready {
            self.audio_status()
        } else {
            "Open a preview to choose its output".to_owned()
        };
        PanelSection::new(&theme, "Audio output")
            .description(
                "Where the preview plays its sound. Retry audio goes back to the system's \
                 default device.",
            )
            .card()
            .child(FormField::stacked(&theme, status, actions))
            .into_any_element()
    }

    /// The Agent page's Review: whether later tasks apply validated changes themselves or
    /// wait for Apply.
    fn render_review_policy(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let mut children = Vec::new();
        match self.panel.read(cx).review_policy() {
            None => children.push(
                settings_note(
                    &theme,
                    "Open a project to choose how its agent's changes are applied.",
                )
                .into_any_element(),
            ),
            Some((current, running)) => {
                let entity = cx.entity();
                let mut control = RadioCards::new(
                    &theme,
                    "review-policy",
                    REVIEW_POLICIES
                        .iter()
                        .map(|(_, label)| RadioCard::new(*label, None))
                        .collect(),
                    REVIEW_POLICIES
                        .iter()
                        .position(|(policy, _)| *policy == current),
                )
                .on_select({
                    let entity = entity.clone();
                    move |index, _, cx| {
                        if let Some((policy, _)) = REVIEW_POLICIES.get(*index) {
                            entity.update(cx, |shell, cx| {
                                shell
                                    .panel
                                    .update(cx, |panel, cx| panel.set_review_policy(*policy, cx));
                            });
                        }
                    }
                });
                if self.qualifying {
                    control = control.on_option_bounds(move |index, bounds, cx| {
                        entity.update(cx, |shell, _| {
                            shell.button_bounds.insert(
                                format!("review-policy-{index}"),
                                crate::probe::bounds_to_record(bounds),
                            );
                        });
                    });
                }
                children.push(control.into_any_element());
                children.push(settings_note(&theme, ReviewPolicy::SCOPE_NOTICE).into_any_element());
                if let Some(running) = running {
                    children.push(
                        settings_note(
                            &theme,
                            format!("The running task keeps {}.", review_label(running)),
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        PanelSection::new(&theme, "Review")
            .description("A change applies to tasks started afterwards.")
            .children(children)
            .into_any_element()
    }

    /// The Export page: the format, size and frame rate every export starts from. The
    /// export panel can change them for one export; what a project's SDK or video cannot
    /// take falls back to the default there.
    fn render_export_defaults(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let defaults = self.export_panel.saved.unwrap_or_default();
        let format = self.measured_segments(
            "default-format",
            FORMATS
                .iter()
                .map(|format| {
                    SegmentOption::new(
                        format_label(*format),
                        format.container == defaults.container && format.codec == defaults.codec,
                    )
                })
                .collect(),
            move |shell, index, cx| {
                if let Some(format) = FORMATS.get(index) {
                    shell.choose_export_defaults(
                        ExportSettings {
                            container: format.container,
                            codec: format.codec,
                            ..defaults
                        },
                        cx,
                    );
                }
            },
            cx,
        );
        let size = self.measured_segments(
            "default-scale",
            SCALES
                .iter()
                .map(|scale| {
                    SegmentOption::new(
                        format!("{}%", (scale * 100.).round() as u32),
                        defaults.scale == *scale,
                    )
                })
                .collect(),
            move |shell, index, cx| {
                if let Some(scale) = SCALES.get(index) {
                    shell.choose_export_defaults(
                        ExportSettings {
                            scale: *scale,
                            ..defaults
                        },
                        cx,
                    );
                }
            },
            cx,
        );
        let fps = self.measured_segments(
            "default-fps",
            DEFAULT_FPS
                .iter()
                .map(|(fps, label)| SegmentOption::new(*label, defaults.fps == *fps))
                .collect(),
            move |shell, index, cx| {
                if let Some((fps, _)) = DEFAULT_FPS.get(index) {
                    shell.choose_export_defaults(
                        ExportSettings {
                            fps: *fps,
                            ..defaults
                        },
                        cx,
                    );
                }
            },
            cx,
        );
        PanelSection::new(&theme, "Export defaults")
            .description(
                "Every export starts with these. The export panel changes them for that export \
                 only.",
            )
            .card()
            .child(
                FormField::row(&theme, "Format", format)
                    .hint("When the project's SDK lacks it, MP4 · MPEG-4 is used."),
            )
            .child(
                FormField::row(&theme, "Size", size)
                    .hint("Half size renders faster and makes a smaller file."),
            )
            .child(
                FormField::row(&theme, "Frame rate", fps)
                    .hint("30 fps applies where it divides the video's rate."),
            )
            .into_any_element()
    }

    /// The General page's Appearance: the theme.
    pub(super) fn render_appearance(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let current = studio_theme::current_preference(cx);
        let options = THEMES
            .iter()
            .map(|(value, label)| SegmentOption::new(*label, *value == current))
            .collect();
        let control = self.measured_segments(
            "theme",
            options,
            |shell, index, cx| {
                if let Some((choice, _)) = THEMES.get(index) {
                    shell.choose_theme(*choice, cx);
                }
            },
            cx,
        );
        let current_motion = studio_theme::current_motion(cx);
        let motion = self.measured_segments(
            "motion",
            MOTIONS
                .iter()
                .map(|(value, label)| SegmentOption::new(*label, *value == current_motion))
                .collect(),
            |shell, index, cx| {
                if let Some((choice, _)) = MOTIONS.get(index) {
                    shell.choose_motion(*choice, cx);
                }
            },
            cx,
        );
        PanelSection::new(&theme, "Appearance")
            .description("Follow the system's light or dark appearance, or pick one.")
            .card()
            .child(FormField::row(&theme, "Theme", control))
            .child(
                FormField::row(&theme, "Motion", motion)
                    .hint("Reduced makes animations snap into place."),
            )
            .into_any_element()
    }
}

/// The review policies in the order the control shows them.
const REVIEW_POLICIES: [(ReviewPolicy, &str); 2] = [
    (ReviewPolicy::AutoApply, "Apply automatically"),
    (ReviewPolicy::ManualReview, "Review before Apply"),
];

fn review_label(policy: ReviewPolicy) -> &'static str {
    match policy {
        ReviewPolicy::AutoApply => "Automatic Apply",
        ReviewPolicy::ManualReview => "Review before Apply",
    }
}

/// A note on a settings page: `Small` in `text_2`.
fn settings_note(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    text_style(div(), TextStyle::Small)
        .text_color(theme.colors.text_2)
        .child(selectable(text))
}

/// A settings page's column of sections, `space_8` apart.
fn settings_page() -> gpui::Div {
    div().flex().flex_col().gap(px(METRICS.space_8))
}

/// Writes the motion `choice` into the app's settings unless a later choice was made
/// meanwhile.
fn save_motion(
    writer: &SettingsWriter,
    generation: u64,
    choice: MotionPreference,
    paths: &studio_engine::app_paths::AppPaths,
) -> Result<(), String> {
    let _write = writer.write.lock();
    if writer.latest_motion.load(Ordering::Acquire) != generation {
        return Ok(());
    }
    let mut settings = AppSettings::load(paths);
    settings.motion = choice;
    settings.save(paths)
}

/// Writes `choice` into the app's settings unless a later choice was made meanwhile.
fn save_theme(
    writer: &SettingsWriter,
    generation: u64,
    choice: AppTheme,
    paths: &studio_engine::app_paths::AppPaths,
) -> Result<(), String> {
    let _write = writer.write.lock();
    if writer.latest.load(Ordering::Acquire) != generation {
        return Ok(());
    }
    let mut settings = AppSettings::load(paths);
    settings.theme = choice;
    settings.save(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_overtaken_by_a_later_choice_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let paths = studio_engine::app_paths::AppPaths::new(dir.path()).unwrap();
        let writer = SettingsWriter::default();
        writer.latest.store(2, Ordering::Release);
        save_theme(&writer, 2, AppTheme::Dark, &paths).unwrap();
        save_theme(&writer, 1, AppTheme::Light, &paths).unwrap();
        assert_eq!(AppSettings::load(&paths).theme, AppTheme::Dark);
    }

    #[test]
    fn the_pages_are_general_agent_sdk_presets_and_export() {
        let ids: Vec<_> = SettingsPage::ALL
            .iter()
            .map(|page| page.nav_item(false).id)
            .collect();
        assert_eq!(
            ids,
            [
                "settings-general",
                "settings-agent",
                "settings-sdk",
                "settings-presets",
                "settings-export"
            ]
        );
        assert_eq!(SettingsPage::default(), SettingsPage::General);
    }

    #[test]
    fn the_options_are_system_light_and_dark() {
        let order: Vec<_> = THEMES.iter().map(|(theme, _)| *theme).collect();
        assert_eq!(order, [AppTheme::System, AppTheme::Light, AppTheme::Dark]);
    }

    #[test]
    fn motion_is_system_reduced_or_full_and_saved_beside_the_theme() {
        let order: Vec<_> = MOTIONS.iter().map(|(motion, _)| *motion).collect();
        assert_eq!(
            order,
            [
                MotionPreference::System,
                MotionPreference::Reduced,
                MotionPreference::Full
            ]
        );
        let dir = tempfile::tempdir().unwrap();
        let paths = studio_engine::app_paths::AppPaths::new(dir.path()).unwrap();
        let writer = SettingsWriter::default();
        writer.latest.store(1, Ordering::Release);
        save_theme(&writer, 1, AppTheme::Dark, &paths).unwrap();
        writer.latest_motion.store(2, Ordering::Release);
        save_motion(&writer, 2, MotionPreference::Reduced, &paths).unwrap();
        save_motion(&writer, 1, MotionPreference::Full, &paths).unwrap();
        let saved = AppSettings::load(&paths);
        assert_eq!(saved.motion, MotionPreference::Reduced);
        assert_eq!(saved.theme, AppTheme::Dark);
    }
}
