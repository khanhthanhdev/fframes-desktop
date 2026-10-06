//! Native compiled tracks. Geometry/selection are engine-owned; textures are window-owned.
use crate::{
    frame_image::create_render_image,
    thumbnail_cache::{ThumbnailCache, ThumbnailKey},
};
use gpui::{
    Bounds, Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, Window, canvas, div, px,
    rgb,
};
use studio_engine::{PreviewFrame, TaskScope, TimelineModel, TimelineSelection, TimelineViewport};

#[derive(Debug, Clone)]
pub enum TimelineEvent {
    Seek(usize),
    Step(isize),
    TogglePlayback,
    ToggleMute,
    BeginScrub,
    EndScrub,
    ScopeChanged(Result<Option<Box<TaskScope>>, String>),
}

pub struct TimelineView {
    model: Option<TimelineModel>,
    viewport: TimelineViewport,
    selection: TimelineSelection,
    position: usize,
    focus: FocusHandle,
    bounds: Option<Bounds<Pixels>>,
    fit_bounds: Option<Bounds<Pixels>>,
    drag: Option<(usize, bool)>,
    cache: ThumbnailCache,
    pending_thumbnail: Option<(ThumbnailKey, PreviewFrame)>,
    clear_cache: bool,
    release_failures: usize,
}
impl EventEmitter<TimelineEvent> for TimelineView {}

impl TimelineView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: None,
            viewport: TimelineViewport::default(),
            selection: TimelineSelection::default(),
            position: 0,
            focus: cx.focus_handle().tab_stop(true),
            bounds: None,
            fit_bounds: None,
            drag: None,
            cache: ThumbnailCache::default(),
            pending_thumbnail: None,
            clear_cache: false,
            release_failures: 0,
        }
    }
    pub fn install(&mut self, model: TimelineModel, position: usize, cx: &mut Context<Self>) {
        let first = self.model.is_none();
        let same_preview = self.model.as_ref().is_some_and(|current| {
            current.report().envelope.identity == model.report().envelope.identity
        });
        if same_preview {
            self.selection.clamp(&model);
        } else {
            self.selection = TimelineSelection::default();
        }
        self.viewport.resize(self.viewport.width, &model);
        if first {
            self.viewport.fit(&model);
        }
        self.position = position.min(model.report().total_frames);
        self.model = Some(model);
        self.drag = None;
        self.clear_cache = true;
        self.pending_thumbnail = None;
        self.emit_scope_changed(cx);
        cx.notify();
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.model = None;
        self.selection = TimelineSelection::default();
        self.drag = None;
        self.position = 0;
        self.clear_cache = true;
        self.pending_thumbnail = None;
        self.emit_scope_changed(cx);
        cx.notify();
    }
    pub fn selected_scope(&self) -> Result<Option<TaskScope>, String> {
        self.model
            .as_ref()
            .map(|model| TaskScope::from_timeline(model.report(), &self.selection).map(Some))
            .unwrap_or(Ok(None))
            .map_err(|error| error.to_string())
    }
    fn emit_scope_changed(&self, cx: &mut Context<Self>) {
        let scope = self.selected_scope().map(|scope| scope.map(Box::new));
        cx.emit(TimelineEvent::ScopeChanged(scope));
    }
    pub fn set_position(&mut self, frame: usize, cx: &mut Context<Self>) {
        if self.position != frame {
            self.position = frame;
            cx.notify();
        }
    }
    pub fn thumbnail_requests(&self) -> Vec<ThumbnailKey> {
        let Some(model) = &self.model else {
            return vec![];
        };
        let scale = thumbnail_scale(model);
        self.viewport
            .thumbnail_frames(model)
            .into_iter()
            .map(|frame| ThumbnailKey::new(model.report().envelope.identity.clone(), scale, frame))
            .filter(|key| !self.cache.contains(key))
            .collect()
    }
    pub fn qualification_metrics(&self) -> serde_json::Value {
        let rect = |b: Bounds<Pixels>, height: f32| {
            [
                f32::from(b.left()),
                f32::from(b.top()),
                f32::from(b.size.width),
                height,
            ]
        };
        serde_json::json!({
            "entries": self.cache.len(), "bytes": self.cache.bytes(),
            "high_water_entries": self.cache.high_water_entries(), "high_water_bytes": self.cache.high_water_bytes(),
            "release_failures": self.release_failures,
            "pixels_per_second": self.viewport.pixels_per_second, "scroll_x": self.viewport.scroll_x,
            "selection": {
                "scene": self.selection.scene_id,
                "range": self.selection.range.as_ref().map(|r| [r.start, r.end]),
                "position": self.position,
                "total_frames": self.model.as_ref().map(|m| m.report().total_frames),
            },
            "ruler_bounds": self.bounds.map(|b| rect(b, 20.)),
            "fit_bounds": self.fit_bounds.map(|b| rect(b, f32::from(b.size.height))),
        })
    }
    pub fn thumbnail(&mut self, key: ThumbnailKey, frame: PreviewFrame, cx: &mut Context<Self>) {
        if self.model.as_ref().is_some_and(|model| {
            key.identity == model.report().envelope.identity
                && key.scale_bits == thumbnail_scale(model).to_bits()
                && key.frame_index == frame.response.frame_index
                && frame.response.scale.to_bits() == key.scale_bits
                && frame.validate(&key.identity).is_ok()
                && self
                    .viewport
                    .thumbnail_frames(model)
                    .contains(&key.frame_index)
        }) {
            self.pending_thumbnail = Some((key, frame));
            cx.notify();
        }
    }
    fn seek(&mut self, frame: usize, cx: &mut Context<Self>) {
        if self
            .model
            .as_ref()
            .is_some_and(|m| m.report().total_frames > 0)
        {
            cx.emit(TimelineEvent::Seek(frame));
        }
    }
    fn pointer_frame(&self, x: Pixels) -> Option<usize> {
        self.viewport.frame_at_x(
            f32::from(x - self.bounds?.left()) as f64,
            self.model.as_ref()?,
        )
    }
    fn pointer_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(frame) = self.pointer_frame(event.position.x) else {
            return;
        };
        window.focus(&self.focus, cx);
        let range = event.modifiers.shift
            || self
                .bounds
                .is_some_and(|b| event.position.y - b.top() > px(24.));
        self.drag = Some((frame, range));
        if range {
            self.selection
                .select_range(frame, frame, self.model.as_ref().unwrap());
            self.emit_scope_changed(cx);
        } else {
            cx.emit(TimelineEvent::BeginScrub);
            self.seek(frame, cx);
        }
        cx.notify();
    }
    fn pointer_move(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        if !event.dragging() {
            return;
        }
        let (Some((start, range)), Some(frame)) = (self.drag, self.pointer_frame(event.position.x))
        else {
            return;
        };
        if range {
            self.selection
                .select_range(start, frame, self.model.as_ref().unwrap());
            self.emit_scope_changed(cx);
            cx.notify();
        } else {
            self.seek(frame, cx);
        }
    }
    fn pointer_up(&mut self, cx: &mut Context<Self>) {
        if let Some((_, false)) = self.drag.take() {
            cx.emit(TimelineEvent::EndScrub);
        }
    }
    fn zoom(&mut self, factor: f64, cx: &mut Context<Self>) {
        if let Some(model) = &self.model {
            self.viewport.zoom(
                factor,
                self.viewport.x_at_frame(self.position, model),
                model,
            );
            cx.notify();
        }
    }
    fn key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let Some(model) = &self.model else {
            return;
        };
        match event.keystroke.key.as_str() {
            "space" => cx.emit(TimelineEvent::TogglePlayback),
            "m" => cx.emit(TimelineEvent::ToggleMute),
            "left" => cx.emit(TimelineEvent::Step(-1)),
            "right" => cx.emit(TimelineEvent::Step(1)),
            "home" => self.seek(0, cx),
            "end" => self.seek(model.report().total_frames, cx),
            "+" | "=" => self.zoom(2., cx),
            "-" => self.zoom(0.5, cx),
            "pageup" => {
                self.viewport.scroll_by(-self.viewport.width * 0.75, model);
                cx.notify();
            }
            "pagedown" => {
                self.viewport.scroll_by(self.viewport.width * 0.75, model);
                cx.notify();
            }
            "s" => {
                self.selection.cycle_scene_at(self.position, model);
                self.emit_scope_changed(cx);
                cx.notify();
            }
            "[" => {
                self.selection.select_range(
                    self.position,
                    self.selection
                        .range
                        .as_ref()
                        .map_or(self.position, |r| r.end),
                    model,
                );
                self.emit_scope_changed(cx);
                cx.notify();
            }
            "]" => {
                self.selection.select_range(
                    self.selection
                        .range
                        .as_ref()
                        .map_or(self.position, |r| r.start),
                    self.position,
                    model,
                );
                self.emit_scope_changed(cx);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }
    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let enabled = self
            .model
            .as_ref()
            .is_some_and(|m| m.report().total_frames > 0);
        let action = std::rc::Rc::new(action);
        let click = action.clone();
        let entity = cx.entity();
        div()
            .id(id)
            .relative()
            .role(gpui::Role::Button)
            .aria_label(label)
            .tab_index(0)
            .tab_stop(enabled)
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(rgb(0x303c4d))
            .opacity(if enabled { 1. } else { 0.45 })
            .focus_visible(|s| s.border_color(rgb(0x78b7fa)))
            .on_click(cx.listener(move |view, _, _, cx| {
                if enabled {
                    click(view, cx);
                }
            }))
            .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
                if enabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    action(view, cx);
                    cx.stop_propagation();
                }
            }))
            .child(label)
            .children((id == "timeline-fit").then(|| {
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |view, _| view.fit_bounds = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            }))
    }
}

fn thumbnail_scale(model: &TimelineModel) -> f64 {
    (160. / model.report().width as f64)
        .min(90. / model.report().height as f64)
        .min(1.)
}

impl Render for TimelineView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_cache {
            for image in self.cache.clear() {
                self.release_failures += usize::from(window.drop_image(image).is_err());
            }
            self.clear_cache = false;
        }
        if let Some((key, frame)) = self.pending_thumbnail.take()
            && let Ok(image) = create_render_image(&frame.response.header, &frame.pixels)
        {
            let bytes =
                frame.response.header.width as usize * frame.response.header.height as usize * 4;
            for image in self.cache.insert(key, image, bytes) {
                self.release_failures += usize::from(window.drop_image(image).is_err());
            }
        }
        let toolbar = div()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .child("Timeline")
            .child(self.button("timeline-zoom-out", "−", cx, |v, cx| v.zoom(0.5, cx)))
            .child(self.button("timeline-zoom-in", "+", cx, |v, cx| v.zoom(2., cx)))
            .child(self.button("timeline-fit", "Fit", cx, |v, cx| {
                if let Some(m) = &v.model {
                    v.viewport.fit(m);
                    cx.notify();
                }
            }))
            .child(
                self.button("timeline-cycle", "Cycle scene (S)", cx, |v, cx| {
                    if let Some(m) = &v.model {
                        v.selection.cycle_scene_at(v.position, m);
                        v.emit_scope_changed(cx);
                        cx.notify();
                    }
                }),
            );
        let mut root = div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(0x303c4d))
            .pt_2()
            .child(toolbar);
        let Some(model) = &self.model else {
            return root.child(
                div()
                    .h(px(130.))
                    .text_sm()
                    .text_color(rgb(0x9aaabd))
                    .child("No compiled timeline available."),
            );
        };
        let geometry = self.viewport.geometry(model);
        let entity = cx.entity().downgrade();
        let mut tracks = div()
            .id("compiled-timeline")
            .track_focus(&self.focus)
            .role(gpui::Role::Group)
            .aria_label("Compiled timeline: ruler scrubs, tracks select ranges")
            .relative()
            .h(px(138.))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(0x303c4d))
            .focus_visible(|s| s.border_color(rgb(0x78b7fa)))
            .on_key_down(cx.listener(|view, event, _, cx| view.key(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e, w, cx| v.pointer_down(e, w, cx)),
            )
            .on_mouse_move(cx.listener(|v, e, _, cx| v.pointer_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| v.pointer_up(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| v.pointer_up(cx)),
            )
            .on_scroll_wheel(cx.listener(|v, e: &gpui::ScrollWheelEvent, _, cx| {
                let Some(model) = &v.model else {
                    return;
                };
                let delta = match e.delta {
                    gpui::ScrollDelta::Pixels(p) => f32::from(p.x) + f32::from(p.y),
                    gpui::ScrollDelta::Lines(p) => (p.x + p.y) * 32.,
                } as f64;
                if e.modifiers.control {
                    let anchor = v
                        .bounds
                        .map_or(0., |b| f32::from(e.position.x - b.left()) as f64);
                    v.viewport.zoom((delta * 0.01).exp(), anchor, model);
                } else {
                    v.viewport.scroll_by(-delta, model);
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |view, cx| {
                            view.bounds = Some(bounds);
                            if let Some(model) = &view.model {
                                let width = f32::from(bounds.size.width) as f64;
                                if (view.viewport.width - width).abs() > 0.5 {
                                    let first = view.viewport.width == 0.;
                                    view.viewport.resize(width, model);
                                    if first {
                                        view.viewport.fit(model);
                                    }
                                    cx.notify();
                                }
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        for tick in geometry.ticks {
            tracks = tracks.child(
                div()
                    .absolute()
                    .left(px(tick.x as f32))
                    .top_0()
                    .text_xs()
                    .text_color(rgb(0x9aaabd))
                    .child(tick.label),
            );
        }
        if let Some(range) = &self.selection.range {
            let start = self.viewport.x_at_frame(range.start, model);
            let end = self.viewport.x_at_frame(range.end, model);
            tracks = tracks.child(
                div()
                    .absolute()
                    .left(px(start as f32))
                    .top(px(22.))
                    .w(px((end - start) as f32))
                    .h(px(114.))
                    .bg(rgb(0x263e57)),
            );
        }
        // Tracks scroll vertically independently of the ruler. Only horizontal visible geometry is rendered.
        let scene_lanes = geometry.scene_lane_count;
        let mut lanes = div()
            .id("timeline-lanes")
            .absolute()
            .top(px(24.))
            .bottom_0()
            .w_full()
            .overflow_y_scroll();
        let mut content = div().relative().h(px(
            ((scene_lanes + model.report().audio_tracks.len()).max(2) * 28) as f32,
        ));
        for rect in geometry.scenes {
            let scene = &model.report().scenes[rect.index];
            if scene.start_frame == scene.end_frame {
                continue;
            }
            let selected = self.selection.scene_id.as_ref() == Some(&scene.instance_id);
            let index = rect.index;
            content = content.child(
                div()
                    .id(format!("scene-{}", scene.instance_id))
                    .role(gpui::Role::Button)
                    .aria_label(format!("Scene {} instance {}", scene.name, scene.index))
                    .tab_index(0)
                    .absolute()
                    .left(px(rect.x as f32))
                    .top(px((rect.lane * 28) as f32))
                    .w(px(rect.width.max(1.) as f32))
                    .h(px(24.))
                    .overflow_hidden()
                    .px_1()
                    .rounded_sm()
                    .bg(rgb(if selected { 0x476b92 } else { 0x2b4057 }))
                    .text_xs()
                    .focus_visible(|s| s.border_1().border_color(rgb(0x78b7fa)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &gpui::MouseDownEvent, w, cx| {
                            if e.modifiers.shift {
                                return;
                            }
                            if let Some(m) = &v.model {
                                v.selection.select_scene(index, m);
                            }
                            v.emit_scope_changed(cx);
                            w.focus(&v.focus, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .on_key_down(cx.listener(move |v, e: &gpui::KeyDownEvent, _, cx| {
                        if e.keystroke.key == "enter" {
                            if let Some(m) = &v.model {
                                v.selection.select_scene(index, m);
                            }
                            v.emit_scope_changed(cx);
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .child(format!("{} #{}", scene.name, scene.index)),
            );
        }
        for rect in geometry.audio {
            let audio = &model.report().audio_tracks[rect.index];
            content = content.child(
                div()
                    .absolute()
                    .left(px(rect.x as f32))
                    .top(px(((scene_lanes + rect.lane) * 28) as f32))
                    .w(px(rect.width.max(1.) as f32))
                    .h(px(24.))
                    .overflow_hidden()
                    .px_1()
                    .rounded_sm()
                    .bg(rgb(0x274a40))
                    .text_xs()
                    .child(format!("♫ {} · {:.3}s", audio.file, audio.start_seconds)),
            );
        }
        lanes = lanes.child(content);
        tracks = tracks.child(lanes).child(
            div()
                .absolute()
                .left(px(self.viewport.x_at_frame(self.position, model) as f32))
                .top_0()
                .bottom_0()
                .w(px(1.))
                .bg(rgb(0x78b7fa)),
        );
        let mut thumbs = div().relative().h(px(38.)).overflow_hidden();
        for frame in self.viewport.thumbnail_frames(model) {
            let key = ThumbnailKey::new(
                model.report().envelope.identity.clone(),
                thumbnail_scale(model),
                frame,
            );
            if let Some(image) = self.cache.get(&key) {
                thumbs = thumbs.child(
                    div()
                        .absolute()
                        .left(px(self.viewport.x_at_frame(frame, model) as f32))
                        .w(px(64.))
                        .h(px(36.))
                        .overflow_hidden()
                        .child(gpui::img(image).w(px(64.)).h(px(36.))),
                );
            }
        }
        let selected = self
            .selection
            .range
            .as_ref()
            .map_or("No range selected".into(), |r| {
                format!(
                    "Range [{}..{}){}",
                    r.start,
                    r.end,
                    self.selection
                        .scene_id
                        .as_ref()
                        .map_or(String::new(), |_| " · scene selected".into())
                )
            });
        root = root.child(tracks).child(thumbs).child(
            div().text_xs().text_color(rgb(0x9aaabd)).child(format!(
                "{} · {selected} · drag tracks / Shift-drag for range",
                model.time_label(self.position)
            )),
        );
        root
    }
}
