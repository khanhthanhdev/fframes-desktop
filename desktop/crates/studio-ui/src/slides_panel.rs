//! The Slides tab: one row per resolved slide stop with its number, title, scene and
//! time, the selected stop's title and speaker notes, and what did not resolve.
//!
//! Like the props panel it never writes `slides.json` itself: it emits [`SlidesEvent`]s
//! and the shell runs them through the engine, which shares one undo history between
//! the slides and the timing. A title or notes edit is sent when the field loses focus,
//! as one undo step.
use gpui::{
    AppContext, Context, Entity, EventEmitter, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_engine::services::preview_coordinator::StopScan;
use studio_engine::slides::suggest;
use studio_engine::slides::{
    SlidesCommand, SlidesEditing, SlidesProblem, SlidesSnapshot, StopAt, StopSource,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use crate::kit::{
    Badge, Button, ButtonSize, Divider, Emphasis, EmptyState, FormField, Icon, List, ListRow,
    Notice, PanelSection, Progress, TextInput, TextInputEvent, TextInputState, Tone,
};
use crate::timeline::{StopClip, clip_at};

#[derive(Debug, Clone, PartialEq)]
pub enum SlidesEvent {
    Run(SlidesCommand),
    ScanStops,
    CancelScan,
    /// A row was selected: show its stop.
    Seek(usize),
    /// "Add stop at playhead": the timeline knows the playhead and the clip under it.
    AddAtPlayhead,
    /// "Present from here": present held at the stop before the slide that ends at this
    /// video frame.
    PresentFrom(usize),
    /// Escape in a field: focus returns to the shell.
    Leave,
}

impl EventEmitter<SlidesEvent> for SlidesPanel {}

/// The selected row: an explicit stop by its position in `slides.json`, a default
/// stop by its video frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selected {
    Explicit(usize),
    Default(usize),
}

pub struct SlidesPanel {
    snapshot: Option<SlidesSnapshot>,
    scan: Option<StopScan>,
    /// Frames per second and clips of the timeline the stops resolved against.
    layout: Option<(usize, Vec<StopClip>)>,
    selected: Option<Selected>,
    title: Entity<TextInputState>,
    notes: Entity<TextInputState>,
    /// The explicit stop the fields show, with the document's title and notes when they
    /// were last put in the fields: a field is refreshed only when the document changed.
    fields_for: Option<(usize, String, String)>,
    /// Refreshed at the top of every render.
    theme: Theme,
}

impl SlidesPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| {
            let mut input = TextInputState::single_line(window, cx);
            input.set_placeholder("Slide title");
            input
        });
        let notes = cx.new(|cx| {
            let mut input = TextInputState::multi_line(window, cx);
            input.set_placeholder("Speaker notes");
            input
        });
        for input in [&title, &notes] {
            cx.subscribe(input, |panel, _, event: &TextInputEvent, cx| match event {
                TextInputEvent::Confirmed | TextInputEvent::Blurred => panel.commit(cx),
                TextInputEvent::Cancelled => {
                    panel.fields_for = None;
                    cx.notify();
                    cx.emit(SlidesEvent::Leave);
                }
                TextInputEvent::Changed => {}
            })
            .detach();
        }
        Self {
            snapshot: None,
            scan: None,
            layout: None,
            selected: None,
            title,
            notes,
            fields_for: None,
            theme: Theme::for_window(window, cx),
        }
    }

    // ---- shell interface ---------------------------------------------------------------

    /// Scan state of the displayed worker; `None` hides the optional controls entirely.
    pub fn set_scan(&mut self, scan: Option<StopScan>, cx: &mut Context<Self>) {
        if self.scan != scan {
            self.scan = scan;
            cx.notify();
        }
    }

    /// The open project's slides (`None` when none is open) and the frames per second
    /// and clips of the timeline they resolved against.
    pub fn set_slides(
        &mut self,
        snapshot: Option<SlidesSnapshot>,
        layout: Option<(usize, Vec<StopClip>)>,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot == snapshot && self.layout == layout {
            return;
        }
        let count = snapshot
            .as_ref()
            .and_then(|s| s.doc.as_ref())
            .map_or(0, |doc| doc.stops.len());
        if let Some(Selected::Explicit(stop)) = self.selected
            && stop >= count
        {
            self.selected = None;
        }
        self.snapshot = snapshot;
        self.layout = layout;
        cx.notify();
    }

    /// Selects the explicit stop at position `stop` of `slides.json`, without seeking.
    pub fn select_stop(&mut self, stop: usize, cx: &mut Context<Self>) {
        self.selected = Some(Selected::Explicit(stop));
        cx.notify();
    }

    // ---- edits -------------------------------------------------------------------------

    fn editable(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.editing == SlidesEditing::Enabled && s.doc.is_some())
    }

    fn run(&mut self, command: SlidesCommand, cx: &mut Context<Self>) {
        cx.emit(SlidesEvent::Run(command));
    }

    fn select(&mut self, selected: Selected, frame: usize, cx: &mut Context<Self>) {
        self.selected = Some(selected);
        cx.emit(SlidesEvent::Seek(frame));
        cx.notify();
    }

    /// Sends the fields' title and notes when they differ from the document's.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some((stop, ..)) = self.fields_for.clone() else {
            return;
        };
        let current = self
            .snapshot
            .as_ref()
            .and_then(|s| s.doc.as_ref())
            .and_then(|doc| doc.stops.get(stop))
            .map(|stop| {
                (
                    stop.title.clone().unwrap_or_default(),
                    stop.notes.clone().unwrap_or_default(),
                )
            });
        let title = self.title.read(cx).text().to_owned();
        let notes = self.notes.read(cx).text().to_owned();
        if !self.editable() || current.as_ref() == Some(&(title.clone(), notes.clone())) {
            return;
        }
        if current.is_some() {
            self.run(
                SlidesCommand::SetStopText {
                    stop,
                    title: Some(title),
                    notes: Some(notes),
                },
                cx,
            );
        }
    }

    /// Puts the selected explicit stop's title and notes in the fields: always when
    /// another stop is selected, otherwise only when the document changed and the field
    /// is not being typed in.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = match self.selected {
            Some(Selected::Explicit(stop)) => self
                .snapshot
                .as_ref()
                .and_then(|s| s.doc.as_ref())
                .and_then(|doc| doc.stops.get(stop))
                .map(|found| {
                    (
                        stop,
                        found.title.clone().unwrap_or_default(),
                        found.notes.clone().unwrap_or_default(),
                    )
                }),
            _ => None,
        };
        let Some((stop, title, notes)) = shown else {
            self.fields_for = None;
            return;
        };
        let other = self.fields_for.as_ref().is_none_or(|(s, ..)| *s != stop);
        let (synced_title, synced_notes) = match &self.fields_for {
            Some((_, title, notes)) if !other => (title.clone(), notes.clone()),
            _ => (String::new(), String::new()),
        };
        for (input, text, synced) in [
            (&self.title, &title, &synced_title),
            (&self.notes, &notes, &synced_notes),
        ] {
            if other || (*synced != *text && !input.focus_handle(cx).is_focused(window)) {
                input.update(cx, |input, cx| input.set_text(text.clone(), cx));
            }
        }
        let disabled = !self.editable();
        for input in [&self.title, &self.notes] {
            input.update(cx, |input, cx| input.set_disabled(disabled, cx));
        }
        self.fields_for = Some((stop, title, notes));
    }

    // ---- rendering ---------------------------------------------------------------------

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: Option<Icon>,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        let entity = cx.entity();
        let mut button = Button::new(&self.theme, id)
            .label(label)
            .emphasis(Emphasis::Neutral)
            .disabled(!enabled)
            .on_click(move |_, _, cx| entity.update(cx, |panel, cx| action(panel, cx)));
        if let Some(glyph) = glyph {
            button = button.left_icon(glyph.path());
        }
        button
    }

    fn suggested(&self, snapshot: &SlidesSnapshot, cx: &mut Context<Self>) -> Option<PanelSection> {
        let scan = self.scan.as_ref()?;
        let theme = self.theme;
        let running = scan.running;
        let mut section = PanelSection::new(&theme, "Suggested")
            .description("Suggestions never change slides.json until you accept them.")
            .child(natural(self.button(
                "slides-scan",
                if scan.running {
                    "Cancel scan"
                } else {
                    "Scan for stops"
                },
                None,
                true,
                cx,
                move |_, cx| {
                    cx.emit(if running {
                        SlidesEvent::CancelScan
                    } else {
                        SlidesEvent::ScanStops
                    })
                },
            )));
        if scan.total > 0 {
            section = section
                .child(Progress::linear(
                    &theme,
                    scan.completed as f32 / scan.total as f32,
                ))
                .child(
                    text_style(div(), TextStyle::Small)
                        .text_color(theme.colors.text_2)
                        .child(format!(
                            "{} / {} frames sampled{}",
                            scan.completed,
                            scan.total,
                            if scan.running { " · scanning" } else { "" }
                        )),
                );
        }
        if scan.sample_limit {
            section = section.child(Notice::new(
                "slides-scan-limit",
                Tone::Warning,
                "Scan reached the 2000-sample limit; some stops may be missing",
            ));
        }
        if scan.suggestions.truncated {
            section = section.child(Notice::new(
                "slides-suggestions-limit",
                Tone::Warning,
                "Suggestions reached the worker's bounds; excess stops or text were dropped",
            ));
        }
        if let Some(notice) = &scan.notice {
            section = section.child(Notice::new(
                "slides-scan-notice",
                Tone::Info,
                notice.clone(),
            ));
        }
        let doc = snapshot.doc.as_ref()?;
        if scan.running {
            return Some(section);
        }
        let proposals = suggest::proposals(&scan.suggestions, &scan.timeline, doc);
        let refusal = suggest::accepting(doc, &proposals).err();
        if let Some(reason) = &refusal {
            section = section.child(Notice::new(
                "slides-accept-refused",
                Tone::Warning,
                format!("Cannot accept all: {reason}"),
            ));
        }
        let command = |stops| SlidesCommand::AcceptSuggestions {
            identity: scan.timeline.envelope.identity.clone(),
            timing_generation: scan.timeline.timing_generation,
            props_generation: scan.props_generation,
            stops,
        };
        if !proposals.is_empty() {
            let all = command(
                proposals
                    .iter()
                    .map(|proposal| proposal.stop.clone())
                    .collect(),
            );
            section = section.child(natural(self.button(
                "slides-accept-all",
                "Accept all",
                None,
                self.editable() && refusal.is_none(),
                cx,
                move |panel, cx| panel.run(all.clone(), cx),
            )));
        } else if scan.total > 0 && scan.completed == scan.total {
            section = section.child(
                EmptyState::new(&theme, "No new stops found")
                    .body("Accepted, trimmed and duplicate stops are omitted."),
            );
        }
        for (index, proposal) in proposals.iter().enumerate() {
            let accept = command(vec![proposal.stop.clone()]);
            let entity = cx.entity();
            let refusal = suggest::accepting(doc, std::slice::from_ref(proposal)).err();
            let title = proposal
                .stop
                .title
                .clone()
                .unwrap_or_else(|| "Untitled".into());
            let row = ListRow::new(&theme, SharedString::from(format!("suggestion-{index}")))
                .tooltip(title.clone())
                .title(title)
                .meta(format!(
                    "{} · {:.2}s · frame {}",
                    proposal.stop.scene,
                    proposal.video_frame as f64 / scan.timeline.fps as f64,
                    proposal.video_frame
                ))
                .trailing(
                    Button::new(&theme, SharedString::from(format!("accept-{index}")))
                        .label("Accept")
                        .size(ButtonSize::Sm)
                        .emphasis(Emphasis::Neutral)
                        .disabled(!self.editable() || refusal.is_some())
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |panel, cx| panel.run(accept.clone(), cx))
                        }),
                );
            section = section.child(row);
            if let Some(notes) = &proposal.stop.notes {
                // Under its row, aligned with the row's text: a quiet label and the notes.
                section = section.child(
                    div()
                        .flex()
                        .flex_col()
                        .px(px(METRICS.space_2))
                        .child(
                            text_style(div(), TextStyle::Label)
                                .text_color(theme.colors.text_3)
                                .child("Proposed speaker notes"),
                        )
                        .child(
                            text_style(div(), TextStyle::Small)
                                .w_full()
                                .min_w_0()
                                .whitespace_normal()
                                .text_color(theme.colors.text_2)
                                .child(notes.clone()),
                        ),
                );
            }
            if let Some(reason) = refusal {
                section = section.child(Notice::new(
                    SharedString::from(format!("accept-limit-{index}")),
                    Tone::Warning,
                    format!("Cannot accept: {reason}"),
                ));
            }
        }
        Some(section)
    }

    /// One stop's row: number, title, scene and time. Default stops are muted.
    fn row(
        &self,
        number: usize,
        frame: usize,
        source: StopSource,
        snapshot: &SlidesSnapshot,
        cx: &mut Context<Self>,
    ) -> ListRow {
        let theme = self.theme;
        let fps = self.layout.as_ref().map_or(0, |(fps, _)| *fps);
        let clips = self.layout.as_ref().map_or(&[][..], |(_, clips)| clips);
        let last = snapshot.stops.last().map(|s| s.video_frame);
        let (selected, title, scene, notes) = match source {
            StopSource::Explicit(index) => {
                let stop = snapshot.doc.as_ref().and_then(|doc| doc.stops.get(index));
                (
                    Selected::Explicit(index),
                    stop.and_then(|s| s.title.clone())
                        .unwrap_or_else(|| "Untitled".into()),
                    stop.map(|s| s.scene.clone()),
                    stop.is_some_and(|s| s.notes.is_some()),
                )
            }
            StopSource::Default => (
                Selected::Default(frame),
                if Some(frame) == last {
                    "Default (video end)".into()
                } else {
                    "Default (clip end)".into()
                },
                clip_at(clips, frame).map(|clip| clip.key.clone()),
                false,
            ),
        };
        let explicit = matches!(selected, Selected::Explicit(_));
        let time = if fps > 0 {
            format!("{:.2}s · frame {frame}", frame as f64 / fps as f64)
        } else {
            format!("frame {frame}")
        };
        let detail = match scene {
            Some(scene) => format!("{scene} · {time}"),
            None => time,
        };
        let entity = cx.entity();
        let mut row = ListRow::new(&theme, SharedString::from(format!("slide-{number}")))
            .leading(
                text_style(div().w(px(METRICS.layout.slide_number_w)), TextStyle::Small)
                    .text_color(theme.colors.text_3)
                    .child(number.to_string()),
            )
            .label(format!("Slide {number}: {title}, {detail}"))
            .tooltip(title.clone())
            .title(title)
            .meta(detail)
            .selected(self.selected == Some(selected))
            .muted(!explicit)
            .on_click(move |_, cx| {
                entity.update(cx, |panel, cx| panel.select(selected, frame, cx));
            });
        if notes {
            row = row.trailing(Badge::new("Notes"));
        }
        row
    }

    /// A problem, titled by the stop it names: unresolved stops offer to move to their
    /// clip's end or to go.
    fn problem(
        &self,
        index: usize,
        problem: &SlidesProblem,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let id = SharedString::from(format!("slides-problem-{index}"));
        let entry = |stop: usize| {
            self.snapshot
                .as_ref()
                .and_then(|s| s.doc.as_ref())
                .and_then(|doc| doc.stops.get(stop))
        };
        let name = |stop: usize| {
            let entry = entry(stop);
            stop_name(
                entry.and_then(|s| s.title.as_deref()),
                entry.map(|s| s.scene.as_str()),
            )
        };
        let (tone, title, body) = problem_text(problem, name);
        let mut notice = Notice::new(id, tone, title);
        if let Some(body) = body {
            notice = notice.body(body);
        }
        let SlidesProblem::Unresolved { stop, .. } = problem else {
            return notice.into_any_element();
        };
        let stop = *stop;
        if let Some(scene) = entry(stop).map(|s| s.scene.clone()) {
            let entity = cx.entity();
            notice = notice.action(
                Button::new(&theme, SharedString::from(format!("{index}-end")))
                    .label("Move to clip end")
                    .emphasis(Emphasis::Neutral)
                    .size(ButtonSize::Sm)
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| {
                        let scene = scene.clone();
                        entity.update(cx, |panel, cx| {
                            panel.run(
                                SlidesCommand::MoveStop {
                                    stop,
                                    scene,
                                    at: StopAt::End,
                                },
                                cx,
                            )
                        });
                    }),
            );
        }
        let entity = cx.entity();
        notice
            .action(
                Button::new(&theme, SharedString::from(format!("{index}-remove")))
                    .label("Remove")
                    .emphasis(Emphasis::Neutral)
                    .size(ButtonSize::Sm)
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |panel, cx| {
                            panel.run(SlidesCommand::RemoveStop { stop }, cx)
                        });
                    }),
            )
            .into_any_element()
    }
}

impl Render for SlidesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme.refresh(window, cx);
        self.sync_fields(window, cx);
        let theme = self.theme;
        // The rows keep their height and the tab scrolls, so the notes are never cut off.
        let scroll = |content: PanelSection| {
            div()
                .id("slides-panel")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(content)
        };
        let section = PanelSection::new(&theme, "Slides");
        let Some(snapshot) = self.snapshot.clone() else {
            return scroll(
                section.child(
                    EmptyState::new(&theme, "No project open")
                        .body("Open a project to mark slide stops."),
                ),
            );
        };
        let mut section = if snapshot.stops.is_empty() {
            section
        } else {
            section.trailing(Badge::new(snapshot.stops.len().to_string()))
        };
        let enabled = self.editable();
        let doc_stops = snapshot.doc.as_ref().map_or(0, |doc| doc.stops.len());
        let resolved = snapshot.unavailable.is_none();
        let removable = match self.selected {
            Some(Selected::Explicit(stop)) => Some(stop),
            _ => None,
        };
        // The video frame the selected slide ends at.
        let present_from = match self.selected {
            Some(Selected::Explicit(index)) => snapshot
                .stops
                .iter()
                .find(|stop| stop.source == StopSource::Explicit(index))
                .map(|stop| stop.video_frame),
            Some(Selected::Default(frame)) => Some(frame),
            None => None,
        };
        // A compact toolbar (small buttons) that fits the narrowest sidebar on one line and
        // wraps rather than clipping when a translation is longer.
        section = section.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(METRICS.space_1))
                .child(
                    self.button(
                        "slides-add",
                        "Add stop",
                        Some(Icon::Plus),
                        enabled && resolved,
                        cx,
                        |_, cx| cx.emit(SlidesEvent::AddAtPlayhead),
                    )
                    .size(ButtonSize::Sm)
                    .tooltip("Add a stop at the playhead"),
                )
                .child(
                    self.button(
                        "slides-remove",
                        "Remove",
                        Some(Icon::Trash),
                        enabled && removable.is_some(),
                        cx,
                        move |panel, cx| {
                            if let Some(stop) = removable {
                                panel.selected = None;
                                panel.run(SlidesCommand::RemoveStop { stop }, cx);
                            }
                        },
                    )
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .tooltip("Remove the selected stop"),
                )
                .child(div().flex_1())
                .child(
                    self.button(
                        "slides-present-from-here",
                        "Present",
                        Some(Icon::Presentation),
                        resolved && present_from.is_some(),
                        cx,
                        move |_, cx| {
                            if let Some(frame) = present_from {
                                cx.emit(SlidesEvent::PresentFrom(frame));
                            }
                        },
                    )
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Ghost)
                    .tooltip("Present from here"),
                ),
        );
        if let SlidesEditing::Disabled(reason) = &snapshot.editing {
            section = section.child(
                Notice::new("slides-disabled", Tone::Warning, "Slide edits are disabled")
                    .body(reason.clone()),
            );
        }
        if let Some(notice) = &snapshot.notice {
            section = section.child(Notice::new("slides-notice", Tone::Info, notice.clone()));
        }
        if let Some(reason) = &snapshot.unavailable {
            section = section.child(Notice::new(
                "slides-unavailable",
                Tone::Info,
                reason.clone(),
            ));
        }
        if resolved && doc_stops == 0 && !snapshot.stops.is_empty() {
            section = section.child(
                EmptyState::new(&theme, "Clip ends are the stops")
                    .body("Every clip end is a stop until you add your own. Use clip ends to give them titles and notes.")
                    .action(self.button(
                        "slides-use-clip-ends",
                        "Use clip ends",
                        None,
                        enabled,
                        cx,
                        |panel, cx| panel.run(SlidesCommand::MaterializeDefaultStops, cx),
                    )),
            );
        }
        let rows = snapshot
            .stops
            .iter()
            .enumerate()
            .map(|(index, stop)| self.row(index + 1, stop.video_frame, stop.source, &snapshot, cx))
            .collect::<Vec<_>>();
        section = section.child(List::new().children(rows));
        for (index, problem) in snapshot.problems.iter().enumerate() {
            section = section.child(self.problem(index, problem, enabled, cx));
        }
        if let Some(suggested) = self.suggested(&snapshot, cx) {
            section = section.child(Divider::horizontal(&theme)).child(suggested);
        }
        match self.selected {
            Some(Selected::Explicit(stop)) if self.fields_for.is_some() => {
                // Numbered like the list (deck order), not by document order.
                let heading = snapshot
                    .stops
                    .iter()
                    .position(|s| s.source == StopSource::Explicit(stop))
                    .map_or_else(
                        || "Stop".to_owned(),
                        |position| format!("Stop {}", position + 1),
                    );
                section = section.child(Divider::horizontal(&theme)).child(
                    PanelSection::new(&theme, heading)
                        .child(FormField::stacked(
                            &theme,
                            "Title",
                            TextInput::new(&theme, "slide-title-input", &self.title),
                        ))
                        .child(FormField::stacked(
                            &theme,
                            "Speaker notes",
                            TextInput::new(&theme, "slide-notes-input", &self.notes)
                                .height(px(METRICS.layout.slides_notes_h))
                                .padding_y(px(METRICS.space_2)),
                        )),
                );
            }
            Some(Selected::Default(_)) => {
                section = section.child(Divider::horizontal(&theme)).child(
                    EmptyState::new(&theme, "A default stop")
                        .body("Default stops have no title or notes. Use clip ends to write them into slides.json."),
                );
            }
            _ => {}
        }
        scroll(section)
    }
}

/// A button at its natural width: a section's column would otherwise stretch it.
fn natural(button: Button) -> gpui::Div {
    div().flex().child(button)
}

/// How a problem names a stop: by its title, else by its scene, never by its position
/// in `slides.json` (which is not the deck's numbering).
fn stop_name(title: Option<&str>, scene: Option<&str>) -> String {
    match (title, scene) {
        (Some(title), _) => format!("\"{title}\""),
        (None, Some(scene)) => format!("A stop in scene \"{scene}\""),
        (None, None) => "A stop".to_owned(),
    }
}

/// A problem's tone, title and body; `name` names the stop at a document position.
fn problem_text(
    problem: &SlidesProblem,
    name: impl Fn(usize) -> String,
) -> (Tone, String, Option<String>) {
    match problem {
        SlidesProblem::WorkerTooOld => (
            Tone::Warning,
            "Slides need a newer preview".to_owned(),
            Some(problem.to_string()),
        ),
        SlidesProblem::Unresolved { stop, reason } => (
            Tone::Warning,
            format!("{} is not on the timeline", name(*stop)),
            Some(reason.clone()),
        ),
        SlidesProblem::Merged { stop, video_frame } => (
            Tone::Info,
            format!("{} was merged into an earlier stop", name(*stop)),
            Some(format!(
                "Both land on video frame {video_frame}; the earlier stop's title and notes win."
            )),
        ),
        SlidesProblem::AtStart { stop } => (
            Tone::Info,
            format!("{} was dropped", name(*stop)),
            Some("It lands on the first frame of the video, where the deck starts.".to_owned()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(stop: usize) -> String {
        match stop {
            0 => stop_name(Some("Closing"), Some("closing")),
            1 => stop_name(None, Some("features")),
            _ => stop_name(None, None),
        }
    }

    #[test]
    fn problems_name_the_stop_not_its_position() {
        let unresolved = SlidesProblem::Unresolved {
            stop: 0,
            reason: "no clip plays scene \"closing\"".into(),
        };
        let (tone, title, body) = problem_text(&unresolved, name);
        assert_eq!(tone, Tone::Warning);
        assert_eq!(title, "\"Closing\" is not on the timeline");
        assert_eq!(body.as_deref(), Some("no clip plays scene \"closing\""));

        let merged = SlidesProblem::Merged {
            stop: 1,
            video_frame: 270,
        };
        let (tone, title, body) = problem_text(&merged, name);
        assert_eq!(tone, Tone::Info);
        assert_eq!(
            title,
            "A stop in scene \"features\" was merged into an earlier stop"
        );
        assert!(body.unwrap().contains("270"));

        let (_, title, _) = problem_text(&SlidesProblem::AtStart { stop: 7 }, name);
        assert_eq!(title, "A stop was dropped");
        for problem in [unresolved, merged, SlidesProblem::AtStart { stop: 1 }] {
            let (_, title, _) = problem_text(&problem, name);
            assert!(
                !title.contains("stop 1") && !title.contains("stop 2"),
                "{title}"
            );
        }
    }
}
