//! Every Studio UI component in every state, in one window, with a System,
//! Light or Dark switch. Used for screenshots and for comparing with the
//! design spec.
//!
//! Run: `cargo run --manifest-path desktop/Cargo.toml -p fframes-studio --example ui_gallery`
//!
//! Hover and keyboard focus cannot be forced, so the gallery labels them: hover
//! a control to see its hover state and press Tab to move the focus ring through
//! them. A row at the end paints the focus ring statically.
//!
//! `--capture <dir> [--appearance light|dark] [--page kit|slides|create]` saves the pages as PNGs
//! instead (Windows only, see `ui_gallery/capture.rs`).

#[path = "ui_gallery/agent.rs"]
mod agent;
#[path = "ui_gallery/capture.rs"]
mod capture;
#[path = "ui_gallery/kit_extras.rs"]
mod kit_extras;
#[path = "ui_gallery/panels.rs"]
mod panels;

use capture::{CaptureArgs, Page};
use fframes_studio_protocol::{
    PREVIEW_CONTRACT_VERSION, PreviewClipTiming, PreviewEnvelope, PreviewIdentity,
    PreviewSceneInfo, PreviewTimelineResponse,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, ScrollHandle, StatefulInteractiveElement, Styled, Window,
    WindowBounds, WindowOptions, div, px, size,
};
use studio_engine::TimelineModel;
use studio_engine::slides::{SlideStop, SlidesDoc, SlidesEditing, SlidesSnapshot, StopAt};
use studio_project::ProjectTemplate;
use studio_theme::{AppTheme, METRICS, TextStyle, Theme, text_style};
use studio_ui::create_panel::CreatePanel;
use studio_ui::export::panel::{ExportPanel, ExportPanelModel};
use studio_ui::export::progress::ExportPhase;
use studio_ui::export::settings::{DeckSummary, ExportSupport, ExportTarget};
use studio_ui::kit::{
    Badge, Button, ButtonSize, Card, CollapsibleSection, Emphasis, FormField, Icon, IconButton,
    IconButtonSize, KbdChip, Menu, MenuItem, MenuState, Notice, PanelSection, SegmentOption,
    SegmentedControl, Select, SettingsFrame, SettingsNavItem, Slider, StatusDot, TabRail,
    TabRailItem, TextInput, TextInputState, Toggle, Tone, bind_text_input_keys, focus_ring,
    heading, label, muted,
};
use studio_ui::shell::StudioShell;
use studio_ui::slides_panel::SlidesPanel;
use studio_ui::timeline::{TimelineView, stop_clips};

const RESOLUTIONS: [&str; 4] = ["1080p", "1440p", "2160p", "Custom"];
const THEMES: [AppTheme; 3] = [AppTheme::System, AppTheme::Light, AppTheme::Dark];

struct Gallery {
    toggle_on: bool,
    segment: usize,
    tab: usize,
    fraction: f32,
    zoom: f32,
    collapsible_open: bool,
    resolution: usize,
    menu: Option<MenuState>,
    menu_focus: FocusHandle,
    notice_visible: bool,
    input: Entity<TextInputState>,
    compact_input: Entity<TextInputState>,
    /// The Slides tab with no stops (the defaults), with stops and notes, and with
    /// problems.
    slides: [Entity<SlidesPanel>; 3],
    /// Stop markers: default stops, and explicit stops with one being dragged.
    markers: [Entity<TimelineView>; 2],
    /// The template chosen in the Create panel.
    create_template: ProjectTemplate,
    /// The page shown in capture mode; `None` shows everything (the interactive gallery).
    page: Option<Page>,
    scroll: ScrollHandle,
    /// The export, settings and onboarding pages: the fixture state shown, and the shell
    /// that draws the settings pages and setup steps (made on first use, in a temporary
    /// app data folder).
    phase7: Phase7,
    /// The agent page's composers, todo lists and history lists (made on first use).
    agent: agent::AgentFixtures,
    /// The props and presets pages' panels (made on first use).
    panels: panels::PanelFixtures,
    /// The kit page's extra components (made on first use).
    extras: kit_extras::KitExtras,
}

#[derive(Default)]
struct Phase7 {
    state: &'static str,
    shell: Option<Entity<StudioShell>>,
}

/// A laid-out preview of four clips at 30 fps: intro, features, demo and outro.
fn deck_report() -> PreviewTimelineResponse {
    let clips = [
        ("intro", 0, 90),
        ("features", 90, 240),
        ("demo", 240, 390),
        ("outro", 390, 480),
    ];
    PreviewTimelineResponse {
        envelope: PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: PreviewIdentity {
                project_id: "gallery".into(),
                open_session: "gallery".into(),
                source_revision: "0".repeat(64),
                worker_generation: 1,
            },
            request_id: 1,
        },
        fps: 30,
        width: 1920,
        height: 1080,
        total_frames: 480,
        duration_seconds: 16.,
        scenes: clips
            .iter()
            .enumerate()
            .map(|(index, (key, start, end))| PreviewSceneInfo {
                instance_id: format!("scene-{index}"),
                editor_instance_key: Some((*key).into()),
                index,
                name: (*key).into(),
                full_name: (*key).into(),
                start_frame: *start,
                end_frame: *end,
                start_seconds: *start as f32 / 30.,
                end_seconds: *end as f32 / 30.,
                clip: Some(PreviewClipTiming {
                    timing_key: (*key).into(),
                    clip_index: 0,
                    trim_start_frames: 0,
                    trim_end_frames: 0,
                    scene_index: Some(index),
                    scene_frames: Some(end - start),
                }),
            })
            .collect(),
        audio_tracks: Vec::new(),
        timing_generation: 0,
    }
}

fn deck_stop(scene: &str, at: StopAt, title: Option<&str>, notes: Option<&str>) -> SlideStop {
    SlideStop {
        scene: scene.into(),
        at,
        title: title.map(Into::into),
        notes: notes.map(Into::into),
    }
}

/// The engine's slides snapshot of `stops` on [`deck_report`].
fn deck_slides(stops: Vec<SlideStop>) -> SlidesSnapshot {
    let doc = SlidesDoc { version: 1, stops };
    let (stops, problems) = studio_engine::slides::resolve::resolve(&doc, &deck_report());
    SlidesSnapshot {
        generation: 1,
        doc: Some(doc),
        stops,
        problems,
        unavailable: None,
        editing: SlidesEditing::Enabled,
        notice: None,
    }
}

fn explicit_stops() -> Vec<SlideStop> {
    vec![
        deck_stop(
            "intro",
            StopAt::End,
            Some("Welcome"),
            Some("Say hello and introduce the team."),
        ),
        deck_stop(
            "features",
            StopAt::Frame(60),
            Some("Three features"),
            Some("Speed, quality, price.\nPause for questions."),
        ),
        deck_stop("demo", StopAt::End, Some("Live demo"), None),
    ]
}

impl Gallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        studio_theme::bind_window(window, cx);
        let input = cx.new(|cx| {
            let mut input = TextInputState::single_line(window, cx);
            input.set_placeholder("Placeholder text");
            input
        });
        let compact_input = cx.new(|cx| {
            let mut input = TextInputState::single_line(window, cx);
            input.set_placeholder("Compact input");
            input.set_text("Typed value", cx);
            input
        });
        let layout = Some((30, stop_clips(&deck_report())));
        let slides = [
            deck_slides(Vec::new()),
            deck_slides(explicit_stops()),
            deck_slides(vec![
                deck_stop("intro", StopAt::End, Some("Welcome"), None),
                deck_stop("closing", StopAt::Frame(10), Some("Thanks"), None),
                deck_stop("features", StopAt::Frame(400), None, None),
                deck_stop("demo", StopAt::Frame(30), Some("Demo"), None),
                deck_stop("demo", StopAt::Frame(30), Some("Demo again"), None),
            ]),
        ]
        .map(|snapshot| {
            let layout = layout.clone();
            cx.new(|cx| {
                let mut panel = SlidesPanel::new(window, cx);
                panel.set_slides(Some(snapshot), layout, cx);
                panel
            })
        });
        slides[1].update(cx, |panel, cx| panel.select_stop(1, cx));
        // Optional worker capability: completed suggestions, a running scan, and the
        // unchanged older-worker panel. Nothing here writes a document.
        slides[0].update(cx, |panel, cx| {
            let mut scan =
                studio_engine::services::preview_coordinator::StopScan::new(deck_report(), 0);
            scan.total = 32;
            scan.completed = 32;
            scan.suggestions.stops = vec![
                fframes_studio_protocol::SuggestedSlideStop {
                    scene: "features".into(),
                    origin: "bullet-list".into(),
                    seconds: 1.0,
                    label: Some("Render once, present anywhere".into()),
                    notes: Some("Explain the portable worker.".into()),
                },
                fframes_studio_protocol::SuggestedSlideStop {
                    scene: "features".into(),
                    origin: "bullet-list".into(),
                    seconds: 2.0,
                    label: Some("One stop per reveal".into()),
                    notes: None,
                },
            ];
            panel.set_scan(Some(scan), cx);
        });
        slides[1].update(cx, |panel, cx| {
            let mut scan =
                studio_engine::services::preview_coordinator::StopScan::new(deck_report(), 0);
            scan.running = true;
            scan.completed = 16;
            scan.total = 32;
            panel.set_scan(Some(scan), cx);
        });
        let markers = [Vec::new(), explicit_stops()].map(|stops| {
            cx.new(|cx| {
                let mut view = TimelineView::new(cx);
                view.install(
                    TimelineModel::new(deck_report()).expect("a valid timeline"),
                    std::sync::Arc::from([]),
                    150,
                    cx,
                );
                view.set_slides(Some(deck_slides(stops)), cx);
                view
            })
        });
        // A drag needs the marker's painted place: start it once the window has drawn.
        let dragged = markers[1].clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            dragged.update(cx, |view, cx| view.show_stop_drag(1, 200, cx));
        })
        .detach();
        Self {
            toggle_on: true,
            segment: 1,
            tab: 0,
            fraction: 0.4,
            zoom: 0.6,
            collapsible_open: true,
            resolution: 0,
            menu: None,
            menu_focus: cx.focus_handle(),
            notice_visible: true,
            input,
            compact_input,
            slides,
            markers,
            create_template: ProjectTemplate::default(),
            page: None,
            scroll: ScrollHandle::new(),
            phase7: Phase7::default(),
            agent: agent::AgentFixtures::default(),
            panels: panels::PanelFixtures::default(),
            extras: kit_extras::KitExtras::default(),
        }
    }

    /// Shows `page` in its fixture `state` for a capture (see [`Page::states`]).
    fn show_capture(&mut self, page: Page, state: &str) {
        self.page = Some(page);
        self.menu = None;
        if page == Page::Create {
            self.create_template = if state == "slide-deck" {
                ProjectTemplate::SlideDeck
            } else {
                ProjectTemplate::Blank
            };
        }
        if let Some(state) = page.states().iter().find(|known| **known == state) {
            self.phase7.state = state;
        }
        debug_assert!(page.states().contains(&state), "unknown state `{state}`");
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        RESOLUTIONS
            .iter()
            .enumerate()
            .map(|(index, name)| MenuItem::new(*name, index == self.resolution))
            .collect()
    }
}

/// A closure for gallery controls that updates the gallery.
fn update<A: ?Sized + 'static>(
    this: &Entity<Gallery>,
    change: impl Fn(&mut Gallery, &A, &mut Window, &mut Context<Gallery>) + 'static,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let this = this.clone();
    move |value, window, cx| {
        this.update(cx, |gallery, cx| {
            change(gallery, value, window, cx);
            cx.notify();
        })
    }
}

/// Like [`update`] for handlers that carry no value.
fn update0(
    this: &Entity<Gallery>,
    change: impl Fn(&mut Gallery, &mut Window, &mut Context<Gallery>) + 'static,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let this = this.clone();
    move |window, cx| {
        this.update(cx, |gallery, cx| {
            change(gallery, window, cx);
            cx.notify();
        })
    }
}

fn row(
    theme: &Theme,
    title: &str,
    children: impl IntoIterator<Item = gpui::AnyElement>,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(METRICS.space_2))
        .child(section_title(theme, title.to_string()))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(METRICS.space_3))
                .children(children),
        )
}

/// A section's title row: Label text in `text_2` with any further child at the end.
fn section_title(theme: &Theme, text: impl Into<gpui::SharedString>) -> gpui::Div {
    text_style(div(), TextStyle::Label)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(METRICS.space_2))
        .text_color(theme.colors.text_2)
        .child(text.into())
}

fn caption(theme: &Theme, text: &str) -> gpui::AnyElement {
    muted(theme, text.to_string()).into_any_element()
}

fn icon_button(theme: &Theme, id: &'static str, glyph: Icon, name: &str) -> IconButton {
    IconButton::new(theme, id, glyph.path())
        .label(name.to_string())
        .tooltip(name.to_string())
}

impl Gallery {
    /// The Slides tab in its states and the timeline's stop markers.
    fn render_slides(&self, theme: &Theme) -> gpui::Div {
        let panel = |title: &str, panel: &Entity<SlidesPanel>| {
            div()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_2))
                .child(muted(theme, title.to_string()))
                .child(
                    Card::raised(theme)
                        .w(px(320.))
                        .h(px(840.))
                        .p(px(METRICS.space_4))
                        .flex()
                        .flex_col()
                        .child(panel.clone()),
                )
        };
        let timeline = |title: &str, view: &Entity<TimelineView>| {
            div()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_2))
                .child(muted(theme, title.to_string()))
                .child(
                    Card::raised(theme)
                        .w(px(1000.))
                        .h(px(170.))
                        .flex()
                        .flex_col()
                        .child(view.clone()),
                )
        };
        Card::raised(theme)
            .p(px(METRICS.space_4))
            .flex()
            .flex_col()
            .gap(px(METRICS.space_4))
            .child(section_title(theme, "Slides tab and stop markers"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(METRICS.space_4))
                    .child(panel(
                        "Suggested stops ready; no silent writes",
                        &self.slides[0],
                    ))
                    .child(panel("Scanning; stop with notes selected", &self.slides[1]))
                    .child(panel(
                        "Problems; older worker (no suggestions)",
                        &self.slides[2],
                    )),
            )
            .child(timeline(
                "Default stops: hollow flags, dashed lines",
                &self.markers[0],
            ))
            .child(timeline(
                "Stops; the second is being dragged",
                &self.markers[1],
            ))
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let colors = theme.colors;
        let this = cx.entity();
        let preference = studio_theme::current_preference(cx);

        let theme_switch = SegmentedControl::new(
            &theme,
            "appearance",
            THEMES
                .iter()
                .map(|choice| {
                    let name = match choice {
                        AppTheme::System => "System",
                        AppTheme::Light => "Light",
                        AppTheme::Dark => "Dark",
                    };
                    SegmentOption::new(name, *choice == preference)
                })
                .collect(),
        )
        .on_select(|index, _, cx| {
            if let Some(choice) = THEMES.get(*index) {
                studio_theme::set_preference(*choice, cx);
            }
        });

        let buttons = row(
            &theme,
            "Button: Ghost (default), Neutral, Primary, Danger; Md 32 and Sm 24",
            [
                Button::new(&theme, "rest").label("Rest").into_any_element(),
                Button::new(&theme, "pressed")
                    .label("Pressed")
                    .pressed(true)
                    .into_any_element(),
                Button::new(&theme, "disabled")
                    .label("Disabled")
                    .disabled(true)
                    .into_any_element(),
                Button::new(&theme, "filled")
                    .label("Filled")
                    .emphasis(Emphasis::Neutral)
                    .into_any_element(),
                Button::new(&theme, "accent")
                    .label("Accent")
                    .emphasis(Emphasis::Primary)
                    .into_any_element(),
                Button::new(&theme, "accent-off")
                    .label("Accent disabled")
                    .emphasis(Emphasis::Primary)
                    .disabled(true)
                    .into_any_element(),
                Button::new(&theme, "destructive")
                    .label("Destructive")
                    .emphasis(Emphasis::Danger)
                    .into_any_element(),
                Button::new(&theme, "icons")
                    .label("Build")
                    .left_icon(Icon::Hammer.path())
                    .right_icon(Icon::ChevronDown.path())
                    .tooltip("Build the preview")
                    .into_any_element(),
                Button::new(&theme, "small-neutral")
                    .label("Small")
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Neutral)
                    .into_any_element(),
                Button::new(&theme, "small-primary")
                    .label("Small primary")
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Primary)
                    .into_any_element(),
                Button::new(&theme, "danger-off")
                    .label("Danger disabled")
                    .emphasis(Emphasis::Danger)
                    .disabled(true)
                    .into_any_element(),
                caption(&theme, "hover for the hover state, press Tab for focus"),
            ],
        );

        let icon_buttons = row(
            &theme,
            "IconButton",
            [
                icon_button(&theme, "ib-small", Icon::Play, "Small")
                    .size(IconButtonSize::Sm)
                    .into_any_element(),
                icon_button(&theme, "ib-medium", Icon::Pause, "Medium")
                    .size(IconButtonSize::Md)
                    .into_any_element(),
                icon_button(&theme, "ib-large", Icon::Play, "Large, round, accent")
                    .size(IconButtonSize::Lg)
                    .emphasis(Emphasis::Primary)
                    .round()
                    .into_any_element(),
                icon_button(&theme, "ib-filled", Icon::Eye, "Filled")
                    .emphasis(Emphasis::Neutral)
                    .into_any_element(),
                icon_button(&theme, "ib-active", Icon::Layers, "Pressed")
                    .pressed(true)
                    .into_any_element(),
                icon_button(&theme, "ib-inverted", Icon::Play, "Inverted (play)")
                    .size(IconButtonSize::Lg)
                    .inverted()
                    .into_any_element(),
                icon_button(&theme, "ib-destructive", Icon::Trash, "Destructive")
                    .emphasis(Emphasis::Danger)
                    .into_any_element(),
                icon_button(&theme, "ib-disabled", Icon::Settings, "Disabled")
                    .disabled(true)
                    .into_any_element(),
            ],
        );

        let toggles = row(
            &theme,
            "Toggle",
            [
                Toggle::new(&theme, "toggle", self.toggle_on)
                    .label("Live toggle")
                    .on_change({
                        let set = update(&this, |gallery, value: &bool, _, _| {
                            gallery.toggle_on = *value
                        });
                        move |value, window, cx| set(&value, window, cx)
                    })
                    .into_any_element(),
                Toggle::new(&theme, "toggle-off", false).into_any_element(),
                Toggle::new(&theme, "toggle-disabled", true)
                    .disabled(true)
                    .into_any_element(),
            ],
        );

        let segment = self.segment;
        let segmented = row(
            &theme,
            "SegmentedControl",
            [
                SegmentedControl::new(
                    &theme,
                    "segmented",
                    ["Fit", "50%", "100%", "200%"]
                        .iter()
                        .enumerate()
                        .map(|(index, name)| {
                            SegmentOption::new(*name, index == segment).disabled(index == 2)
                        })
                        .collect(),
                )
                .on_select(update(&this, |gallery, index: &usize, _, _| {
                    gallery.segment = *index
                }))
                .into_any_element(),
                caption(
                    &theme,
                    "Left and Right move; the disabled option is skipped",
                ),
            ],
        );

        let select = row(
            &theme,
            "Select and Menu",
            [
                div()
                    .w(px(200.))
                    .child(
                        Select::new(&theme, "select", RESOLUTIONS[self.resolution])
                            .stretch_label()
                            .on_open(update(
                                &this,
                                |gallery, bounds: &Bounds<Pixels>, window, cx| {
                                    gallery.menu =
                                        Some(MenuState::anchored(*bounds, &gallery.menu_items()));
                                    gallery.menu_focus.focus(window, cx);
                                },
                            )),
                    )
                    .into_any_element(),
                caption(&theme, "Up, Down, Home, End, Enter, Escape"),
            ],
        );

        let slider = row(
            &theme,
            "Slider",
            [
                div()
                    .w(px(220.))
                    .child(
                        Slider::new(&theme, "slider", self.fraction)
                            .flex()
                            .label("Fraction")
                            .on_change(
                                0.,
                                update(&this, |gallery, value: &f32, _, _| {
                                    gallery.fraction = *value
                                }),
                            ),
                    )
                    .into_any_element(),
                div()
                    .w(px(72.))
                    .px(px(7.))
                    .child(
                        Slider::new(&theme, "zoom", self.zoom)
                            .flex()
                            .label("Zoom")
                            .on_change(
                                0.,
                                update(&this, |gallery, value: &f32, _, _| gallery.zoom = *value),
                            ),
                    )
                    .into_any_element(),
                div()
                    .w(px(120.))
                    .child(Slider::new(&theme, "slider-off", 0.5).flex().disabled(true))
                    .into_any_element(),
            ],
        );

        let tab = self.tab;
        let tabs = row(
            &theme,
            "TabRail",
            [div()
                .w(px(300.))
                .child(
                    TabRail::new(
                        &theme,
                        "tabs",
                        [
                            (Icon::Bot, "Agent"),
                            (Icon::Folder, "Project"),
                            (Icon::Image, "Assets"),
                            (Icon::Palette, "Style"),
                        ]
                        .iter()
                        .enumerate()
                        .map(|(index, (glyph, name))| {
                            // The Agent tab's dot shows while another tab is selected.
                            TabRailItem::new(glyph.path(), *name, index == tab, false)
                                .dot(index == 0 && tab != 0)
                        })
                        .collect(),
                    )
                    .on_select(update(&this, |gallery, index: &usize, _, _| {
                        gallery.tab = *index
                    })),
                )
                .into_any_element()],
        );

        let settings = row(
            &theme,
            "SettingsFrame",
            [div()
                .w(px(560.))
                .h(px(180.))
                .child(
                    SettingsFrame::new(
                        &theme,
                        "settings-frame",
                        [(Icon::Palette, "Appearance"), (Icon::Package, "Setup")]
                            .iter()
                            .enumerate()
                            .map(|(index, (glyph, name))| {
                                SettingsNavItem::new(*name, *name, *glyph, index == tab)
                            })
                            .collect(),
                    )
                    .on_select(update(&this, |gallery, index: &usize, _, _| {
                        gallery.tab = *index
                    }))
                    .on_back(|_, _| {})
                    .children([PanelSection::new(&theme, "Appearance")
                        .description("A section of settings.")
                        .child(FormField::row(
                            &theme,
                            "Theme",
                            Toggle::new(&theme, "settings-row-toggle", true),
                        ))
                        .into_any_element()]),
                )
                .into_any_element()],
        );

        let chips = row(
            &theme,
            "KbdChip, Badge, StatusDot",
            [
                KbdChip::new(&theme, "Space").into_any_element(),
                KbdChip::new(&theme, "Ctrl K").into_any_element(),
                Badge::new("Draft").into_any_element(),
                Badge::new("Ready").tone(Tone::Success).into_any_element(),
                Badge::new("Stale").tone(Tone::Warning).into_any_element(),
                Badge::new("Failed").tone(Tone::Danger).into_any_element(),
                Badge::new("Info").tone(Tone::Info).into_any_element(),
                StatusDot::new(Tone::Neutral).into_any_element(),
                StatusDot::new(Tone::Success).into_any_element(),
                StatusDot::new(Tone::Warning).into_any_element(),
                StatusDot::new(Tone::Danger).into_any_element(),
            ],
        );

        let fields = div()
            .w(px(360.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_title(&theme, "Field rows"))
                    .child(Badge::new("3")),
            )
            .child(FormField::row(&theme, "Width", label(&theme, "1920 px")))
            .child(FormField::row(
                &theme,
                "Hardware",
                Toggle::new(&theme, "field-toggle", true),
            ))
            .child(muted(&theme, "Uses the GPU encoder when available"))
            .child(FormField::row(
                &theme,
                "Quality",
                div()
                    .w(px(160.))
                    .child(Slider::new(&theme, "field-slider", 0.7).flex()),
            ));

        let collapsible = div().w(px(360.)).child(
            CollapsibleSection::new("collapsible", self.collapsible_open)
                .header(section_title(&theme, "Advanced"))
                .content(
                    div()
                        .pt(px(METRICS.space_2))
                        .child(muted(&theme, "Content that slides in over 200 ms.")),
                )
                .on_toggle(update(&this, |gallery, open: &bool, _, _| {
                    gallery.collapsible_open = *open
                })),
        );

        let notices = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .w(px(480.))
            .child(section_title(&theme, "Notice"))
            .child(
                Notice::new("n-info", Tone::Info, "Preview is building")
                    .body("This takes a few seconds."),
            )
            .child(Notice::new("n-success", Tone::Success, "Saved").body("Checkpoint written."))
            .child(
                Notice::new("n-warning", Tone::Warning, "Preview is out of date")
                    .body("The source changed after the last build.")
                    .action(
                        Button::new(&theme, "n-build")
                            .label("Build preview")
                            .emphasis(Emphasis::Primary),
                    ),
            )
            .children(self.notice_visible.then(|| {
                Notice::new("n-danger", Tone::Danger, "Build failed")
                    .body("The project did not compile.")
                    .details(
                        "error[E0425]: cannot find value `x` in this scope\n  --> src/lib.rs:12:5",
                    )
                    .action(Button::new(&theme, "n-retry").label("Retry"))
                    .on_dismiss(update0(&this, |gallery, _, _| {
                        gallery.notice_visible = false
                    }))
            }));

        let inputs = div()
            .w(px(360.))
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .child(section_title(&theme, "TextInput"))
            .child(TextInput::new(&theme, "gallery-input", &self.input))
            .child(TextInput::new(
                &theme,
                "gallery-compact-input",
                &self.compact_input,
            ));

        let focus_sample = row(
            &theme,
            "Focus ring (static sample)",
            [div()
                .rounded(px(METRICS.radius_control))
                .shadow(focus_ring(&theme))
                .child(
                    Button::new(&theme, "ring")
                        .label("Focused")
                        .emphasis(Emphasis::Neutral),
                )
                .into_any_element()],
        );

        let show_kit = matches!(self.page, None | Some(Page::Kit));
        let extras = show_kit.then(|| self.render_kit_extras(&theme, window, cx));
        let show_slides = matches!(self.page, None | Some(Page::Slides));
        let show_create = matches!(self.page, None | Some(Page::Create));
        let choose = update(&this, |gallery, template: &ProjectTemplate, _, _| {
            gallery.create_template = *template;
        });
        let create = div().h(px(380.)).flex().flex_col().child(
            CreatePanel::new(&theme, self.create_template)
                .on_choose(move |template, window, cx| choose(&template, window, cx)),
        );
        let content = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_6))
            .p(px(METRICS.space_6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(heading(&theme, "fframes Studio UI gallery"))
                    .child(theme_switch),
            )
            .when(show_kit, |content| {
                content
                    .child(
                        Card::raised(&theme)
                            .p(px(METRICS.space_4))
                            .flex()
                            .flex_col()
                            .gap(px(METRICS.space_4))
                            .child(buttons)
                            .child(icon_buttons)
                            .child(toggles)
                            .child(segmented)
                            .child(select)
                            .child(slider)
                            .child(tabs)
                            .child(settings)
                            .child(chips)
                            .child(focus_sample),
                    )
                    .child(
                        Card::raised(&theme)
                            .p(px(METRICS.space_4))
                            .flex()
                            .flex_wrap()
                            .gap(px(METRICS.space_6))
                            .child(fields)
                            .child(collapsible)
                            .child(inputs)
                            .child(notices),
                    )
            })
            .children(
                extras.map(|extras| Card::raised(&theme).p(px(METRICS.space_4)).child(extras)),
            )
            .when(show_slides, |content| {
                content.child(self.render_slides(&theme))
            })
            .when(show_create, |content| content.child(create))
            .when(
                matches!(self.page, None | Some(Page::Additions)),
                |content| content.child(render_additions(&theme)),
            )
            .children(self.render_phase7(&theme, window, cx))
            .children(self.render_agent(&theme, window, cx))
            .children(self.render_panels(&theme, window, cx));

        let menu = self.menu.as_ref().map(|state| {
            Menu::new(&theme, "menu", self.menu_items(), state)
                .focus(&self.menu_focus)
                .on_highlight(update(&this, |gallery, index: &usize, _, _| {
                    if let Some(menu) = gallery.menu.as_mut() {
                        menu.set_highlighted(*index)
                    }
                }))
                .on_select(update(&this, |gallery, index: &usize, _, _| {
                    gallery.resolution = *index;
                    gallery.menu = None;
                }))
                .on_dismiss(update0(&this, |gallery, _, _| gallery.menu = None))
        });

        div()
            .id("gallery")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(colors.window)
            .text_color(colors.text_1)
            .font_family(studio_theme::UI_FONT)
            .child(content)
            .children(menu)
    }
}

/// The phase 3 kit additions in their states (capture page `additions`). Hover cannot be
/// forced: a selected row and a hovered field show what hover reveals.
fn render_additions(theme: &Theme) -> gpui::Div {
    use studio_ui::kit::{
        Chip, Divider, EmptyState, FormField, Icon, List, ListRow, PanelSection, Progress,
        ResizeGrip, Spinner, Toast, Tone, icon,
    };

    let colors = theme.colors;
    let text = |value: &str, color: gpui::Rgba| {
        div()
            .text_size(px(12.))
            .text_color(color)
            .child(value.to_string())
    };
    // Stand-ins for kit buttons and inputs, which are not what this page shows.
    let fake_button = |value: &str| {
        div()
            .px(px(METRICS.space_3))
            .h(px(METRICS.size.control_h))
            .flex()
            .items_center()
            .rounded(px(METRICS.radius.control))
            .bg(colors.ctl)
            .text_size(px(13.))
            .text_color(colors.text_1)
            .child(value.to_string())
    };
    let fake_input = |value: &str| {
        div()
            .w(px(160.))
            .px(px(METRICS.space_2))
            .h(px(METRICS.size.input_h))
            .flex()
            .items_center()
            .rounded(px(METRICS.radius.control))
            .border_1()
            .border_color(colors.line)
            .bg(colors.card_2)
            .text_size(px(13.))
            .text_color(colors.text_1)
            .child(value.to_string())
    };
    let action_glyph = |glyph: Icon| icon(glyph, METRICS.size.icon_sm, colors.text_2);
    let column = |width: f32| {
        div()
            .w(px(width))
            .flex()
            .flex_col()
            .gap(px(METRICS.space_3))
    };
    let card = || {
        div()
            .p(px(METRICS.space_4))
            .rounded(px(METRICS.radius.card))
            .border_1()
            .border_color(colors.line)
            .bg(colors.card)
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(METRICS.space_6))
    };

    let rows = PanelSection::new(theme, "ListRow / List")
        .description("plain, selected (actions shown), two-line, muted, disabled; empty list")
        .child(
            List::new()
                .child(
                    ListRow::new(theme, "add-row-plain")
                        .icon(Icon::File)
                        .title("scene.rs")
                        .trailing(text("4 KB", colors.text_3))
                        .action(action_glyph(Icon::Trash))
                        .on_click(|_, _| {}),
                )
                .child(
                    ListRow::new(theme, "add-row-selected")
                        .icon(Icon::Presentation)
                        .title("Intro")
                        .selected(true)
                        .action(action_glyph(Icon::MoreHorizontal))
                        .on_click(|_, _| {}),
                )
                .child(
                    ListRow::new(theme, "add-row-two-line")
                        .icon(Icon::Folder)
                        .title("launch-video")
                        .meta("~/projects/launch-video · 2 h ago")
                        .on_click(|_, _| {}),
                )
                .child(
                    ListRow::new(theme, "add-row-muted")
                        .icon(Icon::Palette)
                        .title("Default preset")
                        .muted(true)
                        .on_click(|_, _| {}),
                )
                .child(
                    ListRow::new(theme, "add-row-disabled")
                        .icon(Icon::Image)
                        .title("missing.png")
                        .disabled(true)
                        .on_click(|_, _| {}),
                ),
        )
        .child(List::new().empty(
            EmptyState::new(theme, "No files yet").body("Files the scene loads appear here."),
        ));

    let empty = PanelSection::new(theme, "EmptyState")
        .description("inline (panels) and centered (stage)")
        .child(
            EmptyState::new(theme, "No presets")
                .body("Save the current style as a preset to reuse it.")
                .action(fake_button("Save preset")),
        )
        .child(
            div()
                .h(px(200.))
                .rounded(px(METRICS.radius.card))
                .bg(colors.stage)
                .child(
                    EmptyState::new(theme, "No project open")
                        .centered()
                        .body("Open a project or create one to start.")
                        .action(fake_button("Open project")),
                ),
        );

    let fields = PanelSection::new(theme, "FormField in a PanelSection.card()")
        .description("row, row with reset (shown on hover) and hint, error, stacked")
        .trailing(text("4 fields", colors.text_3))
        .card()
        .child(FormField::row(theme, "Width", fake_input("1920")))
        .child(
            FormField::row(theme, "Opacity", fake_input("80 %"))
                .reset(|_, _| {})
                .hint("Default 100 %"),
        )
        .child(FormField::row(theme, "Frame rate", fake_input("0")).error("Must be above 0"))
        .child(
            FormField::stacked(theme, "Speaker notes", fake_input("Welcome…"))
                .hint("Shown in present mode only"),
        );

    let dividers = PanelSection::new(theme, "Divider")
        .child(Divider::horizontal(theme))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(METRICS.space_2))
                .child(text("Undo", colors.text_2))
                .child(Divider::vertical(theme))
                .child(text("Redo", colors.text_2)),
        );

    let grip_box = |child: gpui::AnyElement, width: f32, height: f32| {
        div()
            .w(px(width))
            .h(px(height))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(METRICS.radius.item))
            .border_1()
            .border_color(colors.line)
            .child(child)
    };
    let grips = PanelSection::new(theme, "ResizeGrip")
        .description("column and row grips; the third is being dragged")
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(METRICS.space_3))
                .child(grip_box(
                    ResizeGrip::new(theme, "add-grip-column", gpui::Axis::Horizontal)
                        .into_any_element(),
                    48.,
                    80.,
                ))
                .child(grip_box(
                    ResizeGrip::new(theme, "add-grip-row", gpui::Axis::Vertical).into_any_element(),
                    120.,
                    48.,
                ))
                .child(grip_box(
                    ResizeGrip::new(theme, "add-grip-dragging", gpui::Axis::Horizontal)
                        .dragging(true)
                        .into_any_element(),
                    48.,
                    80.,
                )),
        );

    let progress = PanelSection::new(theme, "Progress::linear, Spinner")
        .description("0, 35 % and 100 %; on dark (present mode); spinner (a still ring under reduced motion)")
        .child(Progress::linear(theme, 0.))
        .child(Progress::linear(theme, 0.35))
        .child(Progress::linear(theme, 1.))
        .child(
            div()
                .p(px(METRICS.space_3))
                .rounded(px(METRICS.radius.item))
                .bg(colors.present_bg)
                .child(Progress::linear(theme, 0.6).on_dark()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(METRICS.space_2))
                .child(Spinner::new(theme, "add-spinner"))
                .child(text("Thinking…", colors.text_2)),
        );

    let chips = PanelSection::new(theme, "Chip")
        .description("plain, icon, tones (icon only), removable (button on hover), long label")
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(METRICS.space_2))
                .child(Chip::new(theme, "add-chip-plain", "Selection"))
                .child(Chip::new(theme, "add-chip-icon", "scene.rs").icon(Icon::File))
                .child(
                    Chip::new(theme, "add-chip-success", "Built")
                        .icon(Icon::CircleCheck)
                        .tone(Tone::Success),
                )
                .child(
                    Chip::new(theme, "add-chip-warning", "2 warnings")
                        .icon(Icon::AlertTriangle)
                        .tone(Tone::Warning),
                )
                .child(
                    Chip::new(theme, "add-chip-danger", "Failed")
                        .icon(Icon::AlertTriangle)
                        .tone(Tone::Danger),
                )
                .child(
                    Chip::new(theme, "add-chip-removable", "intro.png")
                        .icon(Icon::Image)
                        .removable(|_, _| {}),
                )
                .child(
                    Chip::new(
                        theme,
                        "add-chip-long",
                        "a very long attachment name that does not fit.mp4",
                    )
                    .icon(Icon::File)
                    .removable(|_, _| {}),
                ),
        );

    let toasts = PanelSection::new(theme, "Toast")
        .description("message; with an action")
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(METRICS.space_4))
                .child(Toast::new(theme, "add-toast", "Copied to the clipboard"))
                .child(
                    Toast::new(theme, "add-toast-action", "Slide deleted")
                        .action(fake_button("Undo")),
                ),
        );

    card()
        .child(column(340.).child(rows).child(dividers).child(grips))
        .child(column(360.).child(empty).child(toasts))
        .child(column(360.).child(fields).child(progress).child(chips))
}

impl Gallery {
    /// The shell that draws the settings pages, setup steps and shell pieces, made on first
    /// use in a temporary app data folder.
    fn fixture_shell(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<StudioShell> {
        self.phase7
            .shell
            .get_or_insert_with(|| {
                let dir = std::env::temp_dir().join("fframes-ui-gallery");
                let _ = std::fs::remove_dir_all(&dir);
                let mut environment = studio_engine::engine::backend::StudioEnvironment::system();
                environment.app_data = Some(dir.join("app"));
                environment.sdk_home = Some(dir.join("sdk"));
                let shell =
                    cx.new(|cx| StudioShell::with_environment(environment, "0.0.0", window, cx));
                // The pages are drawn here, so a change in the shell repaints the gallery.
                cx.observe(&shell, |_, _, cx| cx.notify()).detach();
                shell
            })
            .clone()
    }

    /// The export, settings and onboarding capture pages (phase 7) in their fixture state;
    /// `None` on the other pages and in the interactive gallery.
    fn render_phase7(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let page = self.page?;
        let state = self.phase7.state;
        if page == Page::Export {
            return Some(export_fixture(theme, state));
        }
        let index = page.states().iter().position(|known| *known == state)?;
        if !matches!(page, Page::Settings | Page::Onboarding) {
            return None;
        }
        let shell = self.fixture_shell(window, cx);
        let theme = *theme;
        let colors = theme.colors;
        Some(if page == Page::Settings {
            // The settings dialog's card at its largest, on the dimmed window around it.
            let frame = shell.update(cx, |shell, cx| {
                shell.render_settings_fixture(index, theme, window, cx)
            });
            div()
                .p(px(32.))
                .bg(colors.scrim)
                .child(
                    studio_ui::kit::modal_card(
                        &theme,
                        studio_ui::kit::MODAL_MAX_W,
                        studio_ui::kit::MODAL_MAX_H,
                    )
                    .child(div().flex().flex_1().min_h_0().child(frame)),
                )
                .into_any_element()
        } else {
            let page = shell.update(cx, |shell, cx| shell.render_setup_fixture(index, theme, cx));
            div()
                .h(px(900.))
                .flex()
                .flex_col()
                .bg(colors.window)
                .child(page)
                .into_any_element()
        })
    }
}

/// The export panel for a 60 s 1080p video in the fixture `state`: its settings, a running
/// export, a finished one and a failed one.
fn export_fixture(theme: &Theme, state: &str) -> gpui::AnyElement {
    let capabilities = fframes_studio_protocol::export::ExportCapabilities {
        version: 1,
        width: 1920,
        height: 1080,
        fps: 60,
        timing: true,
        props: false,
        formats: studio_engine::export_options::FORMATS.to_vec(),
    };
    let phase = match state {
        "running" => ExportPhase::Rendering {
            done: 1260,
            total: 3600,
        },
        "done" => ExportPhase::Complete {
            destination: std::env::temp_dir().join("launch-video.mp4"),
        },
        "failed" => ExportPhase::Failed("the encoder stopped: no space left on the disk".into()),
        _ => ExportPhase::Idle,
    };
    let model = ExportPanelModel {
        support: ExportSupport::Ready(Box::new(capabilities)),
        settings: Default::default(),
        phase,
        seconds: Some(60.),
        target: ExportTarget::Video,
        deck: DeckSummary::default(),
    };
    div()
        .h(px(640.))
        .flex()
        .flex_col()
        .child(ExportPanel::new(theme, model))
        .into_any_element()
}

fn main() -> std::process::ExitCode {
    let capture = match CaptureArgs::from_env().transpose() {
        Ok(capture) => capture,
        Err(error) => {
            eprintln!("ui_gallery: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let failed = std::rc::Rc::new(std::cell::Cell::new(false));
    let failed_in_app = failed.clone();
    gpui_platform::application()
        .with_assets(fframes_studio::assets::Assets)
        .run(move |cx: &mut App| {
            fframes_studio::assets::init_fonts(cx).expect("bundled gallery fonts must load");
            bind_text_input_keys(cx);
            studio_theme::init(cx, AppTheme::System);
            let bounds = Bounds::centered(None, size(px(1120.), px(900.)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |window, cx| cx.new(|cx| Gallery::new(window, cx)),
                )
                .expect("failed to open the gallery window");
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
            if let Some(capture) = capture {
                cx.spawn(async move |cx| {
                    match capture::run(capture, window, cx).await {
                        Ok(written) => {
                            for path in written {
                                println!("{}", path.display());
                            }
                        }
                        Err(error) => {
                            eprintln!("ui_gallery: capture failed: {error}");
                            failed_in_app.set(true);
                        }
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
    if failed.get() {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
