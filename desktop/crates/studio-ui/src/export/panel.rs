//! The export panel: container and codec, size, frame rate, the expected file size, the
//! progress of a running export and its Cancel. It is shown in the window in place of the
//! editor, like the settings frame.

use std::rc::Rc;

use fframes_studio_protocol::export::ExportSettings;
use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_engine::export_options::{
    SCALES, estimate_bytes, format_label, format_size, fps_options, output_size,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::progress::ExportPhase;
use super::settings::{DeckSummary, ExportSupport, ExportTarget, deck_blocker};
use crate::kit::{
    Button, Card, Emphasis, Notice, PanelSection, Progress, SegmentOption, SegmentedControl,
    SettingRow, Tone, selectable, setting_card, workflow_column, workflow_title,
};

type Change = Rc<dyn Fn(ExportSettings, &mut Window, &mut App)>;
type TargetChange = Rc<dyn Fn(ExportTarget, &mut Window, &mut App)>;
type Click = Rc<dyn Fn(&mut Window, &mut App)>;

/// The targets in the order the switch shows them.
const TARGETS: [(ExportTarget, &str); 2] = [
    (ExportTarget::Video, "Video"),
    (ExportTarget::Deck, "Slide deck (HTML)"),
];

/// A rough size of one poster and its thumbnail: JPEG at quality 92 of a frame with
/// `pixels` pixels, plus the 320 px thumbnail.
fn poster_bytes(pixels: u64) -> u64 {
    pixels / 4 + 20_000
}

/// What the panel shows. `settings` is what an export would run with now.
pub struct ExportPanelModel {
    pub support: ExportSupport,
    pub settings: ExportSettings,
    pub phase: ExportPhase,
    /// The length of the exported video, with edited timing applied.
    pub seconds: Option<f64>,
    pub target: ExportTarget,
    /// The slides a deck would have.
    pub deck: DeckSummary,
}

#[derive(IntoElement)]
pub struct ExportPanel {
    theme: Theme,
    model: ExportPanelModel,
    on_change: Option<Change>,
    on_target: Option<TargetChange>,
    on_start: Option<Click>,
    on_cancel: Option<Click>,
    on_back: Option<Click>,
}

impl ExportPanel {
    pub fn new(theme: &Theme, model: ExportPanelModel) -> Self {
        Self {
            theme: *theme,
            model,
            on_change: None,
            on_target: None,
            on_start: None,
            on_cancel: None,
            on_back: None,
        }
    }

    pub fn on_change(
        mut self,
        handler: impl Fn(ExportSettings, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    pub fn on_target(
        mut self,
        handler: impl Fn(ExportTarget, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_target = Some(Rc::new(handler));
        self
    }

    /// Why a deck cannot be exported now; `None` for a video.
    fn deck_blocker(&self) -> Option<String> {
        (self.model.target == ExportTarget::Deck)
            .then(|| deck_blocker(&self.model.support, &self.model.deck))
            .flatten()
    }

    fn target_row(&self, locked: bool) -> AnyElement {
        let theme = &self.theme;
        let current = self.model.target;
        let options = TARGETS
            .iter()
            .map(|(target, label)| SegmentOption::new(*label, *target == current).disabled(locked))
            .collect();
        let on_target = self.on_target.clone();
        let control = SegmentedControl::new(theme, "export-target", options).on_select(
            move |index, window, cx| {
                if let (Some((target, _)), Some(change)) = (TARGETS.get(*index), &on_target) {
                    change(*target, window, cx);
                }
            },
        );
        SettingRow::new(theme, "Export as")
            .description("A video file, or a folder that plays the video as slides in a browser.")
            .control(control)
            .into_any_element()
    }

    fn text_row(
        &self,
        id: &'static str,
        title: &'static str,
        hint: &'static str,
        text: String,
    ) -> AnyElement {
        let theme = &self.theme;
        let value = text_style(div().id(id), TextStyle::Body)
            .text_color(theme.colors.text_1)
            .child(selectable(SharedString::from(text)));
        SettingRow::new(theme, title)
            .description(hint)
            .control(value)
            .into_any_element()
    }

    fn deck_rows(&self) -> Vec<AnyElement> {
        let fps = self.model.support.capabilities().map_or_else(
            || "Video".to_owned(),
            |caps| format!("Video · {} fps", caps.fps),
        );
        let slides = match self.model.deck.slides {
            0 => "None yet".to_owned(),
            1 => "1 slide".to_owned(),
            count => format!("{count} slides"),
        };
        vec![
            self.text_row(
                "export-deck-format",
                "Format",
                "Slide decks play H.264 in MP4 in every browser.",
                "MP4 · H.264".into(),
            ),
            self.text_row(
                "export-deck-fps",
                "Frame rate",
                "A deck plays at the video's frame rate, so every stop is exact.",
                fps,
            ),
            self.text_row(
                "export-deck-slides",
                "Slides",
                "One per stop of the Slides tab; the video's last frame is always one.",
                slides,
            ),
        ]
    }

    pub fn on_start(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_start = Some(Rc::new(handler));
        self
    }

    pub fn on_cancel(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    pub fn on_back(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_back = Some(Rc::new(handler));
        self
    }

    fn format_row(&self, locked: bool) -> AnyElement {
        let theme = &self.theme;
        let settings = self.model.settings;
        let formats = self.model.support.formats();
        let options = formats
            .iter()
            .map(|format| {
                SegmentOption::new(
                    format_label(*format),
                    format.container == settings.container && format.codec == settings.codec,
                )
                .disabled(locked)
            })
            .collect();
        let on_change = self.on_change.clone();
        let control = SegmentedControl::new(theme, "export-format", options).on_select(
            move |index, window, cx| {
                if let (Some(format), Some(change)) = (formats.get(*index), &on_change) {
                    change(
                        ExportSettings {
                            container: format.container,
                            codec: format.codec,
                            ..settings
                        },
                        window,
                        cx,
                    );
                }
            },
        );
        SettingRow::new(theme, "Format")
            .description("The container and the codec the video is encoded with.")
            .control(control)
            .into_any_element()
    }

    fn size_row(&self, locked: bool) -> AnyElement {
        let theme = &self.theme;
        let settings = self.model.settings;
        let capabilities = self.model.support.capabilities().cloned();
        let options = SCALES
            .iter()
            .map(|scale| {
                let percent = (scale * 100.).round() as u32;
                let label = match &capabilities {
                    Some(caps) => {
                        let (width, height) = output_size(caps.width, caps.height, *scale);
                        format!("{percent}% · {width}×{height}")
                    }
                    None => format!("{percent}%"),
                };
                SegmentOption::new(label, settings.scale == *scale).disabled(locked)
            })
            .collect();
        let on_change = self.on_change.clone();
        let control = SegmentedControl::new(theme, "export-scale", options).on_select(
            move |index, window, cx| {
                if let (Some(scale), Some(change)) = (SCALES.get(*index), &on_change) {
                    change(
                        ExportSettings {
                            scale: *scale,
                            ..settings
                        },
                        window,
                        cx,
                    );
                }
            },
        );
        SettingRow::new(theme, "Size")
            .description("Half size renders faster and makes a smaller file.")
            .control(control)
            .into_any_element()
    }

    fn fps_row(&self, locked: bool) -> AnyElement {
        let theme = &self.theme;
        let settings = self.model.settings;
        let video_fps = self.model.support.capabilities().map(|caps| caps.fps);
        let choices = video_fps.map(fps_options).unwrap_or_else(|| vec![None]);
        let options = choices
            .iter()
            .map(|fps| {
                let label = match (fps, video_fps) {
                    (Some(fps), _) => format!("{fps} fps"),
                    (None, Some(video)) => format!("Video · {video} fps"),
                    (None, None) => "Video".to_owned(),
                };
                SegmentOption::new(label, settings.fps == *fps).disabled(locked)
            })
            .collect();
        let on_change = self.on_change.clone();
        let control = SegmentedControl::new(theme, "export-fps", options).on_select(
            move |index, window, cx| {
                if let (Some(fps), Some(change)) = (choices.get(*index), &on_change) {
                    change(
                        ExportSettings {
                            fps: *fps,
                            ..settings
                        },
                        window,
                        cx,
                    );
                }
            },
        );
        SettingRow::new(theme, "Frame rate")
            .description("A lower rate must divide the video's.")
            .control(control)
            .into_any_element()
    }

    /// The expected file size (`export-estimate`), a `Small` line in `text_2`.
    fn estimate_line(&self) -> AnyElement {
        let theme = &self.theme;
        let text: SharedString = match (self.model.support.capabilities(), self.model.seconds) {
            (Some(capabilities), Some(seconds)) => {
                let mut bytes = estimate_bytes(&self.model.settings, capabilities, seconds);
                if self.model.target == ExportTarget::Deck {
                    let (width, height) = output_size(
                        capabilities.width,
                        capabilities.height,
                        self.model.settings.scale,
                    );
                    bytes += self.model.deck.slides as u64
                        * poster_bytes(u64::from(width) * u64::from(height));
                }
                format!(
                    "File size: about {} (estimate) · {seconds:.1} s. From the encoder's \
                     bitrate; the real file differs with the content.",
                    format_size(bytes)
                )
                .into()
            }
            _ => "File size: not known yet".into(),
        };
        text_style(div().id("export-estimate"), TextStyle::Small)
            .text_color(theme.colors.text_2)
            .child(selectable(text))
            .into_any_element()
    }

    /// How the export ended, with what to do next: Show in folder after it finished,
    /// Retry after it failed. `None` while it runs or before it ran.
    fn outcome(&self, can_start: bool) -> Option<AnyElement> {
        let theme = &self.theme;
        match &self.model.phase {
            ExportPhase::Complete { destination } => {
                let shown = studio_bootstrap::external_path(destination);
                let path = shown.to_path_buf();
                Some(
                    Notice::new("export-done", Tone::Success, "Export complete")
                        .body(shown.display().to_string())
                        .action(
                            Button::new(theme, "export-reveal")
                                .emphasis(Emphasis::Neutral)
                                .label("Show in folder")
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        )
                        .into_any_element(),
                )
            }
            ExportPhase::Failed(reason) => {
                let on_start = self.on_start.clone();
                Some(
                    Notice::new("export-failed", Tone::Danger, "Export failed")
                        .body(reason.clone())
                        .action(
                            Button::new(theme, "export-retry")
                                .emphasis(Emphasis::Neutral)
                                .label("Retry")
                                .disabled(!can_start)
                                .on_click(move |_, window, cx| {
                                    if let Some(start) = &on_start {
                                        start(window, cx);
                                    }
                                }),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }
}

impl RenderOnce for ExportPanel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let colors = theme.colors;
        let phase = self.model.phase.clone();
        let busy = phase.is_busy();
        let editable = self.model.support.is_editable() && !busy;
        let deck = self.model.target == ExportTarget::Deck;
        let blocker = self.deck_blocker();
        let can_start = self.model.support.can_start() && !busy && blocker.is_none();
        let mut rows = Vec::new();
        if deck {
            rows.push(self.size_row(!editable));
            rows.extend(self.deck_rows());
        } else {
            rows.push(self.format_row(!editable));
            rows.push(self.size_row(!editable));
            rows.push(self.fps_row(!editable));
        }

        let deck_notices: Vec<AnyElement> = if deck {
            blocker
                .map(|reason| {
                    Notice::new("export-deck-blocked", Tone::Warning, reason).into_any_element()
                })
                .into_iter()
                .chain((!self.model.deck.skipped.is_empty()).then(|| {
                    Notice::new(
                        "export-deck-skipped",
                        Tone::Warning,
                        format!("Skipped stops · {}", self.model.deck.skipped.join(" · ")),
                    )
                    .into_any_element()
                }))
                .collect()
        } else {
            Vec::new()
        };

        let notice = match &self.model.support {
            ExportSupport::Probing | ExportSupport::Unknown => Some(
                Notice::new(
                    "export-probing",
                    Tone::Info,
                    "Checking what this project's renderer can export",
                )
                .into_any_element(),
            ),
            ExportSupport::Unavailable(reason) => Some(
                Notice::new(
                    "export-unavailable",
                    Tone::Warning,
                    format!("Export options unavailable · {reason}"),
                )
                .into_any_element(),
            ),
            ExportSupport::Ready(_) => None,
        };

        // A finished or failed export says so in its notice; the status card is for the
        // rest: a running export with its bar, or one that was cancelled.
        let outcome = self.outcome(can_start);
        let status = phase
            .message()
            .filter(|_| outcome.is_none())
            .map(|message| {
                let mut card = setting_card(&theme)
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_2))
                    .child(
                        text_style(div().id("export-message"), TextStyle::Body)
                            .text_color(colors.text_1)
                            .child(selectable(SharedString::from(message))),
                    );
                if phase.shows_progress() {
                    card = card.child(Progress::linear(
                        &theme,
                        phase.fraction().unwrap_or_default(),
                    ));
                }
                card
            });

        let settings = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_6))
            .child(PanelSection::new(&theme, "Output").child(self.target_row(busy)))
            .child(
                PanelSection::new(&theme, if deck { "Slide deck" } else { "Video" }).children(rows),
            );

        let on_start = self.on_start.clone();
        let on_cancel = self.on_cancel.clone();
        let on_back = self.on_back.clone();
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(
                Button::new(&theme, "export-back")
                    .emphasis(Emphasis::Neutral)
                    .label("Back")
                    .on_click(move |_, window, cx| {
                        if let Some(back) = &on_back {
                            back(window, cx);
                        }
                    }),
            )
            .child(div().flex_1())
            .child(if busy {
                Button::new(&theme, "export-cancel")
                    .emphasis(Emphasis::Neutral)
                    .label("Cancel export")
                    .on_click(move |_, window, cx| {
                        if let Some(cancel) = &on_cancel {
                            cancel(window, cx);
                        }
                    })
            } else {
                Button::new(&theme, "export-start")
                    .emphasis(Emphasis::Primary)
                    .label("Export…")
                    .disabled(!can_start)
                    .on_click(move |_, window, cx| {
                        if let Some(start) = &on_start {
                            start(window, cx);
                        }
                    })
            });

        // The outcome or the running export first, then what keeps the export from its
        // options, then the options and what they make.
        let column = workflow_column()
            .gap(px(METRICS.space_6))
            .pb(px(METRICS.space_6))
            .child(workflow_title(
                &theme,
                "Export",
                "The choices here apply to this export only; Settings → Export holds the \
                 defaults.",
            ))
            .children(outcome)
            .children(status)
            .children(notice)
            .children(deck_notices)
            .child(settings)
            .child(self.estimate_line());

        Card::raised(&theme)
            .id("export-panel")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("export-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pt(px(METRICS.space_6))
                    .px(px(METRICS.space_6))
                    .child(column),
            )
            .child(
                div()
                    .flex_none()
                    .border_t(px(METRICS.size.divider))
                    .border_color(colors.line)
                    .py(px(METRICS.space_3))
                    .px(px(METRICS.space_6))
                    .child(workflow_column().child(actions)),
            )
    }
}
