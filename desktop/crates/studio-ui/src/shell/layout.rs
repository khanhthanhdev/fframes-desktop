//! The window's root layout, after Cap's editor window: header and notices, then the
//! workspace (a row of the player and the tabbed sidebar with its width grip, above the
//! full-width timeline and its height grip), then the status bar. After Oleafly the panes
//! are flat and square and meet at hairlines, with no gutters or shadows between them.

use gpui::{
    Axis, Context, InteractiveElement, IntoElement, ParentElement, Render, StyleRefinement, Styled,
    Window, WindowControlArea, canvas, div, prelude::FluentBuilder, px,
};
use studio_theme::METRICS;

use super::StudioShell;
use super::keymap::WORKSPACE;
use super::sections::{Sections, box_probe};
use crate::frame_image::create_render_image;
use crate::kit::{ResizeGrip, windows_caption_controls};

impl Render for StudioShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        self.viewport = [f32::from(viewport.width), f32::from(viewport.height)];
        if self.theme.refresh(window, cx) {
            // The cached sections paint with the theme too.
            cx.notify();
        }
        if self.overflow_open {
            self.sync_overflow();
        }
        self.sync_slides(cx);
        self.sync_deck(cx);
        self.sync_present_window(window, cx);
        if self.clear_images {
            self.images.clear(window);
            self.clear_images = false;
        }
        if let Some(install) = self.pending_ready.take() {
            let ready = &install.ready.clone();
            let audio_ready = install.audio_ready;
            let mut slot = Some(install);
            // No filesystem work here. The controller's nonblocking guard is the source
            // fence and it is held from the final `can_install` check through the
            // coordinator commit and the preview-state install: a workflow publication
            // or a source reconcile cannot interleave, and every check reads the guard's
            // current state, never a copy taken when preparation finished. Unavailable
            // (a job holds the controller, or a source scan is pending) means deferred.
            let engine = self.engine.clone();
            let outcome = audio_ready.then(|| {
                engine.with_install_fence(|source| {
                    let mut committed = false;
                    if self.presentation.state.as_ref().is_some_and(|s| {
                        s.session() == source.session()
                            && s.source() == source.source()
                            && s.generation() == source.generation()
                    }) && self.preview_state.can_install(ready, source).is_ok()
                    {
                        let image = ready
                            .frame
                            .as_ref()
                            .map(|f| create_render_image(&f.response.header, &f.pixels))
                            .transpose();
                        match image {
                            Ok(image) => {
                                let model =
                                    studio_engine::TimelineModel::new(ready.timeline.clone());
                                if let Err(e) = &model {
                                    self.error = Some(e.clone());
                                }
                                if self.preview.as_ref().is_some_and(|p| {
                                    model.is_ok()
                                        && p.commit(ready.identity().clone(), ready.seek_serial)
                                }) && self.preview_state.install(ready, source).is_ok()
                                {
                                    if let Some(image) = image {
                                        self.images.replace_image(image, window);
                                    } else {
                                        self.images.clear(window);
                                    }
                                    let install = slot.take().expect("taken once");
                                    self.transport = install.clock;
                                    if self.resume_install
                                        && ready.position < ready.timeline.total_frames
                                    {
                                        let _ = self
                                            .transport
                                            .toggle(self.clock_start.elapsed().as_secs_f64());
                                    }
                                    self.resume_install = false;
                                    self.output = install.audio;
                                    self.audio_ready_epoch = Some(self.transport.epoch());
                                    self.requested_clock_epoch = self.transport.epoch();
                                    self.presented_serial = ready.seek_serial;
                                    self.painted_frame =
                                        ready.frame.as_ref().map(|f| f.response.frame_index);
                                    if let Some(frame) = ready.frame.as_ref() {
                                        if let Err(error) = self
                                            .canvas
                                            .install_frame(ready.identity().clone(), frame)
                                        {
                                            self.canvas.clear();
                                            self.canvas.message = Some(format!(
                                                "Canvas metadata unavailable: {error}"
                                            ));
                                        }
                                    } else {
                                        self.canvas.clear();
                                    }
                                    let waveform = ready
                                        .audio_source
                                        .as_ref()
                                        .map(|source| source.waveform().clone())
                                        .unwrap_or_else(|| std::sync::Arc::from([]));
                                    self.timeline.update(cx, |v, cx| {
                                        v.install(model.unwrap(), waveform, ready.position, cx)
                                    });
                                    self.displayed = Some(ready.clone());
                                    if let Some(workflow) = &self.agent.workflow {
                                        let _ = workflow.set_displayed_preview_identity(Some(
                                            ready.identity().clone(),
                                        ));
                                    }
                                    if ready.tag().base_source == *source.source()
                                        || source.is_current_build(ready.tag())
                                    {
                                        self.awaiting_preview = None;
                                    }
                                    // Tell the workflow what is on screen so its accepted-awaiting
                                    // label clears exactly when the matching preview shows.
                                    self.agent.displayed =
                                        Some(ready.identity().source_revision.clone());
                                    self.error = None;
                                    // The commit dropped the queued thumbnails of the old preview.
                                    self.thumbnails_sent = None;
                                    self.request_thumbnails(cx);
                                    self.forward_playhead();
                                    committed = true;
                                }
                            }
                            Err(e) => self.error = Some(e.to_string()),
                        }
                    }
                    committed
                })
            });
            match outcome.flatten() {
                // Deferred, not rejected.
                None => self.pending_ready = slot,
                Some(true) => {
                    self.report_displayed();
                    // A render the player alone asked for can commit the install; the
                    // cached sections render again with what it changed.
                    cx.notify();
                }
                Some(false) => {
                    self.audio.stop(self.transport.epoch());
                    self.discard_install(cx);
                }
            }
        }
        if let Some(f) = self.pending_frame.take()
            && self.preview_state.accepts_frame(&f)
            && self.requested_clock_epoch == self.transport.epoch()
        {
            match create_render_image(&f.response.header, &f.pixels) {
                Ok(image) => {
                    self.images.present(image, f.response.seek_serial, window);
                    if let Some(preview) = self
                        .displayed
                        .as_ref()
                        .map(|ready| ready.identity().clone())
                    {
                        if let Err(error) = self.canvas.install_frame(preview, &f) {
                            self.canvas.clear();
                            self.canvas.message =
                                Some(format!("Canvas metadata unavailable: {error}"));
                        }
                    } else {
                        self.canvas.clear();
                    }
                    self.presented_serial = f.response.seek_serial;
                    self.painted_frame = Some(f.response.frame_index);
                    self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.activate_audio(cx);
        let theme = self.theme;
        let project_open = self.presentation.project.is_some();
        // A drag ends when its grip goes away: only the grip's tracker hears the release.
        let workspace_shown =
            self.create_choice.is_none() && self.onboarding.is_none() && !self.export_panel.open;
        if !workspace_shown
            || self.presenting.is_some()
            || !project_open && matches!(self.resize, Some(Resize::Timeline { .. }))
        {
            self.resize = None;
        }
        let sections = self
            .sections
            .get_or_insert_with(|| Sections::new(cx))
            .clone();
        if self.presenting.is_some() {
            // The rest of the shell is hidden while presenting.
            let player = div()
                .flex_1()
                .min_w_0()
                .flex()
                .child(sections.player.clone())
                .into_any_element();
            return self.render_presenting_root(player, cx);
        }
        // The cached sections keep their boxes here: a cached view is laid out by this
        // style alone.
        let windows = cfg!(target_os = "windows");
        // The header row is flush with the window's edges and stays uncached: on Windows it
        // is the drag area and holds the caption buttons, and a cached view does not replay
        // window control areas.
        let header = div()
            .flex()
            .flex_none()
            .w_full()
            .h(px(METRICS.header_h))
            // The hairline is the row's border: a half-pixel strip of its own would be painted
            // over by the pane below at 100 %.
            .border_b(px(METRICS.size.divider))
            .border_color(theme.colors.line)
            .when(windows, |row| {
                row.window_control_area(WindowControlArea::Drag)
            })
            .child(
                sections.header.clone().cached(
                    StyleRefinement::default()
                        .flex_1()
                        .min_w_0()
                        .h(px(METRICS.header_h)),
                ),
            )
            .when(windows, |row| {
                row.child(
                    windows_caption_controls(
                        &theme,
                        window.is_window_active(),
                        window.is_maximized(),
                        true,
                        true,
                    )
                    .relative()
                    .child(box_probe("caption", cx)),
                )
            });
        let notices = self.render_notices(cx);
        let status_bar = self.render_status_strip(cx);
        let sidebar_w = self.sidebar_column_width();
        let sidebar = div()
            .relative()
            .flex_none()
            .w(px(sidebar_w))
            .flex()
            .child(self.render_sidebar(&sections, cx))
            .child(self.render_agent_grip(sidebar_w, cx));
        let entity = cx.entity();
        // The workspace measures itself so the split can clamp against its height.
        let measure = canvas(
            move |bounds, _, cx| {
                let height = f32::from(bounds.size.height);
                entity.update(cx, |shell, cx| {
                    if shell.editor_height != height {
                        shell.editor_height = height;
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        // The playhead runs between frames while playing; only this layer repaints for it.
        let playing = self.transport.playing();
        let playhead = self.timeline.update(cx, |timeline, _| {
            timeline.set_playing(playing);
            timeline.playhead()
        });
        if playing && playhead.is_some() {
            window.request_animation_frame();
        }
        let mut workspace = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(measure);
        if project_open {
            let split = self.split();
            let player = match self.review.view() {
                Some(review) => div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(review)
                    .into_any_element(),
                None => sections.player.clone().into_any_element(),
            };
            workspace = workspace
                .child(
                    div()
                        .flex_1()
                        .min_h(px(split.player_min_height))
                        .flex()
                        .child(div().flex_1().min_w_0().flex().child(player))
                        .child(sidebar),
                )
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .w_full()
                        .h(px(split.timeline_height))
                        .child(
                            div()
                                .size_full()
                                .overflow_hidden()
                                .border_t(px(METRICS.size.divider))
                                .border_color(theme.colors.line)
                                .bg(theme.colors.window)
                                .child(
                                    self.timeline
                                        .clone()
                                        .cached(StyleRefinement::default().size_full()),
                                ),
                        )
                        // Over the cached timeline: only this moves while playing.
                        .child(crate::timeline::playhead_overlay(&self.theme, playhead))
                        .child(self.render_resize_grip(split.timeline_height, cx)),
                );
        } else {
            // No project: the start screen takes the whole workspace. The sidebar's tabs
            // all need a project (the agent refuses without one), so it is not shown.
            workspace = workspace.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_empty_state(cx)),
            );
        }
        // Settings is Oleafly's dialog over the window: built here, where its search field can
        // take the window, and drawn last so it paints over everything.
        if self.settings_open && self.settings_dialog.is_none() {
            self.settings_dialog = Some(super::settings_dialog::SettingsDialog::open(
                cx.entity(),
                &self.panel,
                window,
                cx,
            ));
        }
        let settings_dialog = self.settings_dialog.clone();
        Self::on_actions(div(), cx)
            .relative()
            .size_full()
            .key_context(WORKSPACE)
            .track_focus(&self.focus)
            .tab_group()
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(self.render_edit_tracking())
            .bg(theme.colors.window)
            .text_color(theme.colors.text_1)
            .font_family(studio_theme::UI_FONT)
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    // The notices are a band of their own above the panes.
                    .when(!notices.is_empty(), |main| {
                        main.child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_col()
                                .gap(px(METRICS.space_2))
                                .p(px(METRICS.space_2))
                                .border_b(px(METRICS.size.divider))
                                .border_color(theme.colors.line)
                                .children(notices),
                        )
                    })
                    // The setup and export replace the whole row: the setup's Agent step
                    // draws the same panel entity, which renders once per frame. They keep
                    // an inset; the workspace's panes run to the window's edges.
                    .child(if let Some(template) = self.create_choice {
                        inset(self.render_create(template, cx))
                    } else if self.onboarding.is_some() {
                        inset(self.render_onboarding(cx))
                    } else if self.export_panel.open {
                        inset(self.render_export(cx))
                    } else {
                        workspace.into_any_element()
                    }),
            )
            .children(status_bar)
            .children(settings_dialog)
            .into_any_element()
    }
}

impl StudioShell {
    /// The split for the timeline height the grip last asked for, or the default.
    pub(super) fn split(&self) -> VerticalSplit {
        self.split_for(self.timeline_height.unwrap_or(METRICS.timeline_default_h))
    }

    fn split_for(&self, preferred: f32) -> VerticalSplit {
        vertical_split(self.editor_height - METRICS.gutter, preferred)
    }

    /// The sidebar's width: the one its grip asked for, or the default, fitted to the
    /// window.
    pub(super) fn sidebar_column_width(&self) -> f32 {
        self.sidebar_width_for(self.sidebar_width.unwrap_or(METRICS.sidebar_w))
    }

    fn sidebar_width_for(&self, preferred: f32) -> f32 {
        sidebar_width(self.viewport[0] - 2. * METRICS.gutter, preferred)
    }

    /// Moves the held grip's split to the pointer's `position` along the grip's axis.
    fn drag_resize(&mut self, position: f32, cx: &mut Context<Self>) {
        match self.resize {
            Some(Resize::Timeline {
                start_y,
                start_height,
            }) => {
                let preferred = start_height - (position - start_y);
                let height = self.split_for(preferred).timeline_height;
                if self.timeline_height != Some(height) {
                    self.timeline_height = Some(height);
                    cx.notify();
                }
            }
            // The grip is on the sidebar's left edge: moving left widens it.
            Some(Resize::Sidebar {
                start_x,
                start_width,
            }) => {
                let preferred = start_width - (position - start_x);
                let width = self.sidebar_width_for(preferred);
                if self.sidebar_width != Some(width) {
                    self.sidebar_width = Some(width);
                    cx.notify();
                }
            }
            None => {}
        }
    }

    /// A `ResizeGrip` wired to the shell's split: `start` turns the press position into the
    /// `Resize` it begins.
    fn resize_grip(
        &self,
        id: &'static str,
        axis: Axis,
        dragging: bool,
        start: impl Fn(f32) -> Resize + 'static,
        cx: &mut Context<Self>,
    ) -> ResizeGrip {
        let (started, moved, ended) = (cx.entity(), cx.entity(), cx.entity());
        ResizeGrip::new(&self.theme, id, axis)
            .dragging(dragging)
            .on_drag_start(move |position, _, cx| {
                started.update(cx, |shell, cx| {
                    shell.resize = Some(start(f32::from(position)));
                    // Paints the grip's tracker that follows the pointer.
                    cx.notify();
                });
            })
            .on_drag(move |position, _, cx| {
                moved.update(cx, |shell, cx| shell.drag_resize(f32::from(position), cx));
            })
            .on_drag_end(move |_, cx| {
                ended.update(cx, |shell, cx| {
                    if shell.resize.take().is_some() {
                        cx.notify();
                    }
                });
            })
    }

    /// The handle on the sidebar's left edge that drags its width, centred on the seam.
    fn render_agent_grip(&self, width: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let dragging = matches!(self.resize, Some(Resize::Sidebar { .. }));
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(GRIP_OFFSET))
            .flex()
            .child(self.resize_grip(
                "agent-resize-handle",
                Axis::Horizontal,
                dragging,
                move |start_x| Resize::Sidebar {
                    start_x,
                    start_width: width,
                },
                cx,
            ))
            .child(grip_probe(cx, |shell, measured| {
                shell.agent_grip_bounds = Some(measured)
            }))
            .into_any_element()
    }

    /// The handle above the timeline that drags its height, as in Cap's editor.
    fn render_resize_grip(&self, timeline_height: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let dragging = matches!(self.resize, Some(Resize::Timeline { .. }));
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(GRIP_OFFSET))
            .flex()
            .child(self.resize_grip(
                "timeline-resize-handle",
                Axis::Vertical,
                dragging,
                move |start_y| Resize::Timeline {
                    start_y,
                    start_height: timeline_height,
                },
                cx,
            ))
            .child(grip_probe(cx, |shell, measured| {
                shell.grip_bounds = Some(measured)
            }))
            .into_any_element()
    }
}

/// Where a grip's hit area starts relative to the edge it drags: centred in the gutter
/// before that edge, which is none, so on the hairline between the panes.
const GRIP_OFFSET: f32 = -(METRICS.layout.gutter + METRICS.size.grip_hit) / 2.;

/// The setup, create and export flows in the workspace's place, kept off the window's sides
/// and foot.
fn inset(content: gpui::AnyElement) -> gpui::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .px(px(METRICS.space_2))
        .pb(px(METRICS.space_2))
        .child(content)
        .into_any_element()
}

/// Records where a grip was painted, `[left, top, width, height]`, for the telemetry.
fn grip_probe(
    cx: &mut Context<StudioShell>,
    store: impl Fn(&mut StudioShell, [f32; 4]) + 'static,
) -> impl IntoElement {
    let entity = cx.entity();
    canvas(
        move |bounds, _, cx| {
            let measured = [
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ];
            entity.update(cx, |shell, _| store(shell, measured));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The grip being dragged and what it held at the press.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Resize {
    Timeline { start_y: f32, start_height: f32 },
    Sidebar { start_x: f32, start_width: f32 },
}

/// The sidebar's width in a workspace row `available` wide. The gap beside the player (none,
/// the panes meet at a hairline) is fixed; the sidebar takes `preferred`, at most
/// `sidebar_max_w` and at least `sidebar_min_w`, and leaves the player `editor_min_w`. When
/// both minimums do not fit, they shrink in proportion, as the vertical split does.
pub(super) fn sidebar_width(available: f32, preferred: f32) -> f32 {
    let space = (available - METRICS.gutter).max(0.);
    let scale = (space / (METRICS.sidebar_min_w + METRICS.editor_min_w)).min(1.);
    let sidebar_min = METRICS.sidebar_min_w * scale;
    let sidebar_max = (space - METRICS.editor_min_w * scale)
        .min(METRICS.sidebar_max_w)
        .max(sidebar_min);
    preferred.clamp(sidebar_min, sidebar_max)
}

/// How the workspace splits between the player and sidebar row and the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct VerticalSplit {
    pub player_min_height: f32,
    pub timeline_height: f32,
}

/// Splits `available` (the workspace's height less the gap between the row and the
/// timeline) the way Cap's `EditorVerticalLayout` does: the timeline takes `preferred`,
/// clamped so neither side drops below its minimum; when both minimums do not fit,
/// they shrink in proportion.
pub(super) fn vertical_split(available: f32, preferred: f32) -> VerticalSplit {
    let available = available.max(0.);
    let player_min_h = METRICS.layout.player_min_h;
    let scale = (available / (player_min_h + METRICS.timeline_min_h)).min(1.);
    let player_min_height = player_min_h * scale;
    let timeline_min_height = METRICS.timeline_min_h * scale;
    let timeline_max_height = (available - player_min_height).max(timeline_min_height);
    VerticalSplit {
        player_min_height,
        timeline_height: preferred.clamp(timeline_min_height, timeline_max_height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_timeline_fits_a_full_height_window() {
        let split = vertical_split(700., METRICS.timeline_default_h);
        assert_eq!(split.timeline_height, 260.);
        assert_eq!(split.player_min_height, 320.);
    }

    #[test]
    fn the_timeline_is_clamped_between_its_minimum_and_the_players() {
        assert_eq!(vertical_split(700., 100.).timeline_height, 240.);
        assert_eq!(vertical_split(700., 600.).timeline_height, 380.);
    }

    #[test]
    fn both_minimums_shrink_together_when_they_do_not_fit() {
        let split = vertical_split(280., 260.);
        assert_eq!(split.player_min_height, 160.);
        assert_eq!(split.timeline_height, 120.);
    }

    /// The workspace row of a window `width` wide, inside the side gutters (none).
    fn row(width: f32) -> f32 {
        width - 2. * METRICS.gutter
    }

    #[test]
    fn the_sidebar_keeps_its_default_in_common_windows() {
        for width in [1280., 1536., 1920.] {
            assert_eq!(sidebar_width(row(width), METRICS.sidebar_w), 416.);
        }
    }

    #[test]
    fn the_sidebar_is_clamped_to_its_range_and_the_player_minimum() {
        assert_eq!(sidebar_width(row(1920.), 100.), 320.);
        assert_eq!(sidebar_width(row(1920.), 900.), 520.);
        // The player keeps 480 of 1280, so 520 fits.
        assert_eq!(sidebar_width(row(1280.), 900.), 520.);
        // The player keeps 480 of 1024.
        assert_eq!(sidebar_width(row(1024.), 900.), 520.);
        assert_eq!(sidebar_width(row(960.), 900.), 480.);
        assert_eq!(sidebar_width(row(900.), 900.), 420.);
    }

    #[test]
    fn narrow_windows_shrink_the_sidebar_and_player_minimums_together() {
        // 780 of 800: both minimums at 97.5 %.
        let width = sidebar_width(row(780.), METRICS.sidebar_w);
        assert!((width - 312.).abs() < 0.01, "{width}");
        assert_eq!(sidebar_width(0., METRICS.sidebar_w), 0.);
    }

    #[test]
    fn no_space_leaves_no_timeline() {
        let split = vertical_split(-10., 260.);
        assert_eq!(split.player_min_height, 0.);
        assert_eq!(split.timeline_height, 0.);
    }
}
