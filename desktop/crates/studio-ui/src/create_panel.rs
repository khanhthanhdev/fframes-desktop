//! The first step of Create: what the new project starts from (Blank or a Slide deck), the
//! style preset it starts with, and its aspect ratio, size and frame rate. It is shown in
//! the window in place of the editor, like the export panel; its Choose folder… asks for
//! the new project's folder.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, relative,
};
use studio_engine::{engine::backend::NewProject, presets::EntryView};
use studio_project::{ProjectTemplate, manifest::VideoHints};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use crate::kit::{
    Button, Card, Emphasis, PanelSection, PictureCard, PictureCards, RadioCard, RadioCards,
    SegmentOption, SegmentedControl, SettingRow, workflow_column, workflow_title,
};

type Change = Rc<dyn Fn(NewProject, &mut Window, &mut App)>;
type Click = Rc<dyn Fn(&mut Window, &mut App)>;

/// The templates in the order the panel lists them, with their descriptions.
pub const TEMPLATES: [(ProjectTemplate, &str, &str); 2] = [
    (
        ProjectTemplate::Blank,
        "Blank",
        "Three title scenes to start a video from.",
    ),
    (
        ProjectTemplate::SlideDeck,
        "Slide deck",
        "Six fframes-kit slides with bullets that come in one by one, a stop after each and \
         speaker notes, ready to present or export as an HTML deck.",
    ),
];

/// The aspect ratios offered: label, width and height parts. A slide deck is 16:9 only.
pub const RATIOS: [(&str, u32, u32); 4] = [
    ("16:9 Landscape", 16, 9),
    ("9:16 Vertical", 9, 16),
    ("1:1 Square", 1, 1),
    ("4:5 Portrait", 4, 5),
];

/// The sizes offered, by the pixels on the frame's short side.
pub const SIZES: [(&str, u32); 3] = [("720p", 720), ("1080p", 1080), ("4K", 2160)];

/// The frame rates offered.
pub const RATES: [u32; 5] = [24, 25, 30, 50, 60];

/// The style cards per row.
const STYLE_COLUMNS: usize = 4;

/// The frame of the ratio `(w, h)` with `short` pixels on its short side, at `fps`.
pub fn frame((w, h): (u32, u32), short: u32, fps: u32) -> VideoHints {
    let (width, height) = if w >= h {
        (short * w / h, short)
    } else {
        (short, short * h / w)
    };
    VideoHints { width, height, fps }
}

fn ratio_index(format: &VideoHints) -> Option<usize> {
    RATIOS
        .iter()
        .position(|(_, w, h)| format.width * h == format.height * w)
}

fn short_side(format: &VideoHints) -> u32 {
    format.width.min(format.height)
}

/// `choice` as `template` asks: a slide deck keeps its size but turns 16:9.
pub fn with_template(choice: &NewProject, template: ProjectTemplate) -> NewProject {
    let mut next = NewProject {
        template,
        ..choice.clone()
    };
    if template == ProjectTemplate::SlideDeck {
        next.format = frame((16, 9), short_side(&choice.format), choice.format.fps);
    }
    next
}

#[derive(IntoElement)]
pub struct CreatePanel {
    theme: Theme,
    choice: NewProject,
    styles: Vec<EntryView>,
    on_change: Option<Change>,
    on_continue: Option<Click>,
    on_back: Option<Click>,
}

impl CreatePanel {
    /// `styles` are the presets a project can start with, besides the template's own.
    pub fn new(theme: &Theme, choice: &NewProject, styles: &[EntryView]) -> Self {
        Self {
            theme: *theme,
            choice: choice.clone(),
            styles: styles.to_vec(),
            on_change: None,
            on_continue: None,
            on_back: None,
        }
    }

    /// Receives the whole choice after any one of its parts changed.
    pub fn on_change(
        mut self,
        handler: impl Fn(NewProject, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Choose folder…: asks for the folder of the project.
    pub fn on_continue(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_continue = Some(Rc::new(handler));
        self
    }

    pub fn on_back(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_back = Some(Rc::new(handler));
        self
    }

    /// Calls `on_change` with what `edit` makes of the choice.
    fn change(
        &self,
        edit: impl Fn(&NewProject, usize) -> Option<NewProject> + 'static,
    ) -> impl Fn(&usize, &mut Window, &mut App) + 'static {
        let on_change = self.on_change.clone();
        let choice = self.choice.clone();
        move |index, window, cx| {
            if let (Some(change), Some(next)) = (&on_change, edit(&choice, *index)) {
                change(next, window, cx);
            }
        }
    }

    fn template_choice(&self) -> AnyElement {
        RadioCards::new(
            &self.theme,
            "create-template",
            TEMPLATES
                .iter()
                .map(|(_, label, text)| RadioCard::new(*label, Some(text)))
                .collect(),
            TEMPLATES
                .iter()
                .position(|(template, _, _)| *template == self.choice.template),
        )
        .on_select(self.change(|choice, index| {
            TEMPLATES
                .get(index)
                .map(|(template, _, _)| with_template(choice, *template))
        }))
        .into_any_element()
    }

    /// The template's own look first, then every preset, as pictures in rows of
    /// [`STYLE_COLUMNS`]; card `i` of row `r` is `create-style-{r}-{i}`.
    fn style_choice(&self) -> AnyElement {
        let theme = &self.theme;
        // The templates' starter tokens: white text on #0D1117.
        let starter = [
            Some(gpui::rgb(0x0D1117).into()),
            Some(gpui::rgb(0xFFFFFF).into()),
            None,
        ];
        let cards: Vec<(SharedString, [Option<gpui::Hsla>; 3], Option<String>)> =
            std::iter::once(("Starter".into(), starter, None))
                .chain(self.styles.iter().map(|entry| {
                    (
                        entry.name.clone().into(),
                        entry.swatch.map(|color| {
                            color.map(|c| {
                                gpui::rgba(u32::from_be_bytes([c.r, c.g, c.b, c.a])).into()
                            })
                        }),
                        Some(entry.key.clone()),
                    )
                }))
                .collect();
        let selected = cards
            .iter()
            .position(|(_, _, key)| *key == self.choice.preset);
        let keys: Vec<Option<String>> = cards.iter().map(|(_, _, key)| key.clone()).collect();
        let rows = cards.chunks(STYLE_COLUMNS).enumerate().map(|(row, chunk)| {
            let first = row * STYLE_COLUMNS;
            let keys = keys.clone();
            let items = chunk
                .iter()
                .map(|(name, swatch, _)| PictureCard::new(name.clone(), style_picture(*swatch)))
                .collect();
            let cards = PictureCards::new(
                theme,
                SharedString::from(format!("create-style-{row}")),
                items,
                selected
                    .filter(|index| (first..first + chunk.len()).contains(index))
                    .map(|index| index - first),
            )
            .on_select(self.change(move |choice, index| {
                keys.get(first + index).map(|key| NewProject {
                    preset: key.clone(),
                    ..choice.clone()
                })
            }));
            // A short last row keeps the cards of a full one.
            div()
                .w(relative(chunk.len() as f32 / STYLE_COLUMNS as f32))
                .child(cards)
        });
        div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_3))
            .children(rows)
            .into_any_element()
    }

    fn ratio_row(&self) -> AnyElement {
        let deck = self.choice.template == ProjectTemplate::SlideDeck;
        let current = ratio_index(&self.choice.format);
        let options = RATIOS
            .iter()
            .enumerate()
            .map(|(index, (label, w, h))| {
                SegmentOption::new(*label, current == Some(index))
                    .disabled(deck && (*w, *h) != (16, 9))
            })
            .collect();
        let control = SegmentedControl::new(&self.theme, "create-ratio", options).on_select(
            self.change(|choice, index| {
                RATIOS.get(index).map(|(_, w, h)| NewProject {
                    format: frame((*w, *h), short_side(&choice.format), choice.format.fps),
                    ..choice.clone()
                })
            }),
        );
        SettingRow::new(&self.theme, "Aspect ratio")
            .description(if deck {
                "Slides are laid out on a 16:9 canvas."
            } else {
                "Wide for screens, vertical for phones and stories."
            })
            .control(control)
            .into_any_element()
    }

    fn size_row(&self) -> AnyElement {
        let short = short_side(&self.choice.format);
        let options = SIZES
            .iter()
            .map(|(label, pixels)| SegmentOption::new(*label, short == *pixels))
            .collect();
        let control = SegmentedControl::new(&self.theme, "create-size", options).on_select(
            self.change(|choice, index| {
                let (_, w, h) = RATIOS[ratio_index(&choice.format).unwrap_or(0)];
                SIZES.get(index).map(|(_, pixels)| NewProject {
                    format: frame((w, h), *pixels, choice.format.fps),
                    ..choice.clone()
                })
            }),
        );
        let VideoHints { width, height, .. } = self.choice.format;
        SettingRow::new(&self.theme, "Size")
            .description(format!("{width} × {height} pixels."))
            .control(control)
            .into_any_element()
    }

    fn rate_row(&self) -> AnyElement {
        let options = RATES
            .iter()
            .map(|fps| SegmentOption::new(format!("{fps} fps"), self.choice.format.fps == *fps))
            .collect();
        let control = SegmentedControl::new(&self.theme, "create-fps", options).on_select(
            self.change(|choice, index| {
                RATES.get(index).map(|fps| NewProject {
                    format: VideoHints {
                        fps: *fps,
                        ..choice.format.clone()
                    },
                    ..choice.clone()
                })
            }),
        );
        SettingRow::new(&self.theme, "Frame rate")
            .description("24 for film, 30 for most video, 60 for smooth motion.")
            .control(control)
            .into_any_element()
    }
}

/// A style's picture: its background with a title line in its text color and an accent
/// rule (the text color when it has no accent).
fn style_picture([background, text, accent]: [Option<gpui::Hsla>; 3]) -> Div {
    let text = text.unwrap_or(gpui::white());
    div()
        .size_full()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(6.))
        .px(px(12.))
        .bg(background.unwrap_or(gpui::black()))
        .child(div().h(px(8.)).w(relative(0.7)).rounded(px(2.)).bg(text))
        .child(
            div()
                .h(px(5.))
                .w(relative(0.45))
                .rounded(px(2.))
                .bg(text.opacity(0.6)),
        )
        .child(
            div()
                .h(px(4.))
                .w(px(28.))
                .rounded(px(2.))
                .bg(accent.unwrap_or(text)),
        )
}

impl RenderOnce for CreatePanel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let colors = theme.colors;
        let video = PanelSection::new(&theme, "Video")
            .child(self.ratio_row())
            .child(self.size_row())
            .child(self.rate_row());
        let on_continue = self.on_continue.clone();
        let on_back = self.on_back.clone();
        let actions = div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(
                Button::new(&theme, "create-back")
                    .emphasis(Emphasis::Neutral)
                    .label("Back")
                    .on_click(move |_, window, cx| {
                        if let Some(back) = &on_back {
                            back(window, cx);
                        }
                    }),
            )
            .child(
                text_style(div().flex_1().text_right(), TextStyle::Small)
                    .text_color(colors.text_2)
                    .child(summary(&self.choice, &self.styles)),
            )
            .child(
                Button::new(&theme, "create-continue")
                    .emphasis(Emphasis::Primary)
                    .label("Choose folder…")
                    .on_click(move |_, window, cx| {
                        if let Some(next) = &on_continue {
                            next(window, cx);
                        }
                    }),
            );
        // Title and the choices under section headings, scrolling, over the actions, Back
        // left and the one primary action right, as on the export panel.
        let column = workflow_column()
            .gap(px(METRICS.space_6))
            .pb(px(METRICS.space_6))
            .child(workflow_title(
                &theme,
                "Create a project",
                "Pick what the new video starts from, its style and its frame; Studio asks \
                 for its folder next.",
            ))
            .child(PanelSection::new(&theme, "Start from").child(self.template_choice()))
            .child(
                PanelSection::new(&theme, "Style")
                    .description("A preset sets colors, type, spacing and motion; change it later in the Style tab.")
                    .child(self.style_choice()),
            )
            .child(video);
        Card::raised(&theme)
            .id("create-panel")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("create-body")
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

/// "Slide deck · Sunset · 1920 × 1080 · 30 fps".
fn summary(choice: &NewProject, styles: &[EntryView]) -> String {
    let template = TEMPLATES
        .iter()
        .find(|(template, _, _)| *template == choice.template)
        .map_or("", |(_, label, _)| label);
    let style = choice
        .preset
        .as_ref()
        .and_then(|key| styles.iter().find(|entry| &entry.key == key))
        .map_or("Starter", |entry| entry.name.as_str());
    let VideoHints { width, height, fps } = choice.format;
    format!("{template} · {style} · {width} × {height} · {fps} fps")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_offered_once_and_blank_is_first() {
        let offered: Vec<_> = TEMPLATES.iter().map(|(template, _, _)| *template).collect();
        assert_eq!(offered, ProjectTemplate::ALL);
        assert_eq!(offered[0], ProjectTemplate::default());
    }

    #[test]
    fn frames_put_the_size_on_the_short_side() {
        let size = |ratio, short| {
            let f = frame(ratio, short, 30);
            (f.width, f.height)
        };
        assert_eq!(size((16, 9), 720), (1280, 720));
        assert_eq!(size((9, 16), 2160), (2160, 3840));
        assert_eq!(size((4, 5), 1080), (1080, 1350));
        assert_eq!(size((1, 1), 720), (720, 720));
        // Every offered frame has even sides, as H.264 needs.
        for (_, w, h) in RATIOS {
            for (_, short) in SIZES {
                let (width, height) = size((w, h), short);
                assert!(width % 2 == 0 && height % 2 == 0, "{width}x{height}");
            }
        }
    }

    #[test]
    fn a_slide_deck_turns_the_frame_16_by_9_and_keeps_size_and_rate() {
        let vertical = NewProject {
            format: frame((9, 16), 2160, 60),
            ..NewProject::default()
        };
        let deck = with_template(&vertical, ProjectTemplate::SlideDeck);
        assert_eq!(
            (deck.format.width, deck.format.height, deck.format.fps),
            (3840, 2160, 60)
        );
        let back = with_template(&deck, ProjectTemplate::Blank);
        assert_eq!(back.format, deck.format);
    }
}
