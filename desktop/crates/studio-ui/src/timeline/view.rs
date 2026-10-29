//! The timeline entity: Cap's transform (engine-owned), ruler, clip track, overlay and
//! audio rows, the mix waveform and the minimap, and the clip edits: trim handles, drag to
//! reorder and split at the playhead. Geometry and selection are engine-owned, thumbnail
//! textures window-owned. The shell mounts it cached and paints the playhead.
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    Bounds, Context, EventEmitter, FocusHandle, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, Window, canvas, div,
    prelude::FluentBuilder, px,
};
use studio_engine::services::thumbnail_cache::{ThumbnailCache, ThumbnailKey};
use studio_engine::slides::{SlidesCommand, SlidesDoc, SlidesEditing, SlidesSnapshot, StopSource};
use studio_engine::timing::{ResolvedClip, TimelineCommand, TimelineSnapshot, TrimEdge};
use studio_engine::{
    PreviewFrame, TaskScope, TimelineModel, TimelineSelection, TimelineTransform, TimingEditing,
};
use studio_theme::{METRICS, TextStyle, Theme, TrackKind, text_style, track_color};

use super::edit::{
    DRAG_THRESHOLD_PX, SnapTargets, drop_marker, drop_position, split_at, trim_for_edge,
};
use super::minimap::{MinimapDrag, render_minimap, zoomed_in};
use super::model::{ClipSegment, clip_segments};
use super::playhead::PlayheadGeometry;
use super::ruler::{flag_left, render_ruler};
use super::segment::{lane_name, row_top, segment, track_row};
use super::stops::{StopClip, clip_of, ends_before, hit_marker, new_stop, stop_at, stop_clips};
use super::waveform::waveform_path;
use crate::frame_image::create_render_image;
use crate::kit::{EmptyState, Icon, Toast, focus_ring, selectable};

/// The tracks' key context: the shell binds the timeline's keys in it.
pub const KEY_CONTEXT: &str = "Timeline";

#[derive(Debug, Clone)]
pub enum TimelineEvent {
    Seek(usize),
    BeginScrub,
    EndScrub,
    ScopeChanged(Result<Option<Box<TaskScope>>, String>),
    /// An edit of the timing document, for the engine.
    Edit(TimelineCommand),
    /// An edit of the slide stops (`slides.json`) from a marker: add, move or remove.
    /// A marker drag is one gesture, so one undo step in the history the timing edits
    /// share.
    Stop(SlidesCommand),
    /// The thumbnails the visible track wants changed: scrolled, zoomed, resized, replaced
    /// or one arrived.
    ThumbnailsWanted,
    /// What the timeline's controls in the transport and the header show changed.
    Controls(TimelineControls),
}

/// The state of the timeline's controls, which the shell draws: zoom, Fit, Split and Cycle
/// in the transport, undo and redo in the header.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TimelineControls {
    /// A timeline with frames is shown: zoom, Fit and Cycle work.
    pub enabled: bool,
    /// The zoom as the slider's 0..=1 fraction, Fit to the closest zoom.
    pub zoom_fraction: f32,
    /// The timing document can be edited: Split works.
    pub editable: bool,
    pub can_undo: bool,
    pub can_redo: bool,
}

pub struct TimelineView {
    model: Option<TimelineModel>,
    /// The engine's clips; the clip track follows them when present.
    snapshot: Option<TimelineSnapshot>,
    clips: Vec<ClipSegment>,
    /// Peaks of the displayed preview's mix, see `studio_engine::waveform`.
    waveform: Arc<[f32]>,
    /// The thumbnails last announced with `ThumbnailsWanted`.
    wanted: Vec<ThumbnailKey>,
    viewport: TimelineTransform,
    selection: TimelineSelection,
    /// The playhead frame. Stored without a notify: the shell paints the playhead.
    position: usize,
    /// Whether the playhead runs between frames, and when it last moved.
    playing: bool,
    anchor: Instant,
    focus: FocusHandle,
    bounds: Option<Bounds<Pixels>>,
    /// The controls state last reported with `TimelineEvent::Controls`.
    controls: TimelineControls,
    minimap_bounds: Option<Bounds<Pixels>>,
    minimap_drag: Option<MinimapDrag>,
    drag: Option<(usize, bool)>,
    /// A trim or reorder drag of a clip.
    edit: Option<EditDrag>,
    /// The engine's slide stops, drawn as markers.
    slides: Option<SlidesSnapshot>,
    /// A drag of a stop marker.
    stop_drag: Option<StopDrag>,
    /// The stop (its position in `slides.json`) whose marker was pressed last; Delete
    /// removes it.
    selected_stop: Option<usize>,
    /// Segments built by the last render, after culling.
    painted_segments: usize,
    /// How often the view rendered; playback must not render it.
    renders: u64,
    cache: ThumbnailCache<gpui::RenderImage>,
    /// Images that left the cache; the window releases them at the next render.
    retired: Vec<Arc<gpui::RenderImage>>,
    release_failures: usize,
}
impl EventEmitter<TimelineEvent> for TimelineView {}

impl TimelineView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: None,
            snapshot: None,
            clips: Vec::new(),
            waveform: Arc::from([]),
            wanted: Vec::new(),
            viewport: TimelineTransform::default(),
            selection: TimelineSelection::default(),
            position: 0,
            playing: false,
            anchor: Instant::now(),
            focus: cx.focus_handle().tab_stop(true),
            bounds: None,
            controls: TimelineControls::default(),
            minimap_bounds: None,
            minimap_drag: None,
            drag: None,
            edit: None,
            slides: None,
            stop_drag: None,
            selected_stop: None,
            painted_segments: 0,
            renders: 0,
            cache: ThumbnailCache::default(),
            retired: Vec::new(),
            release_failures: 0,
        }
    }
    pub fn install(
        &mut self,
        model: TimelineModel,
        waveform: Arc<[f32]>,
        position: usize,
        cx: &mut Context<Self>,
    ) {
        let first = self.model.is_none();
        let same_preview = self.model.as_ref().is_some_and(|current| {
            current.report().envelope.identity == model.report().envelope.identity
        });
        if same_preview {
            self.selection.clamp(&model);
        } else {
            self.selection = TimelineSelection::default();
            // Another build: the dragged clip belongs to a layout that is gone.
            self.abandon_edit_drag(cx);
        }
        self.viewport.resize(self.viewport.width, &model);
        if first {
            self.viewport.fit(&model);
        }
        self.position = position.min(model.report().total_frames);
        self.anchor = Instant::now();
        self.model = Some(model);
        self.waveform = waveform;
        self.refresh_clips();
        self.drag = None;
        self.minimap_drag = None;
        self.retired.extend(self.cache.clear());
        self.emit_scope_changed(cx);
        cx.notify();
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.abandon_edit_drag(cx);
        self.model = None;
        self.waveform = Arc::from([]);
        self.refresh_clips();
        self.selection = TimelineSelection::default();
        self.drag = None;
        self.minimap_drag = None;
        self.position = 0;
        self.playing = false;
        self.retired.extend(self.cache.clear());
        self.emit_scope_changed(cx);
        cx.notify();
    }
    /// The engine's timing document and clips, from the presentation.
    pub fn set_snapshot(&mut self, snapshot: Option<TimelineSnapshot>, cx: &mut Context<Self>) {
        if self.snapshot != snapshot {
            self.snapshot = snapshot;
            self.refresh_clips();
            cx.notify();
        }
    }
    /// The engine's slide stops, from the presentation: the markers follow them.
    pub fn set_slides(&mut self, slides: Option<SlidesSnapshot>, cx: &mut Context<Self>) {
        if self.slides != slides {
            let count = slides
                .as_ref()
                .and_then(|s| s.doc.as_ref())
                .map_or(0, |doc| doc.stops.len());
            self.selected_stop = self.selected_stop.filter(|stop| *stop < count);
            self.slides = slides;
            cx.notify();
        }
    }

    /// The frames per second and clips the slide stops resolve against, from the
    /// displayed report; `None` before a preview.
    pub fn stop_layout(&self) -> Option<(usize, Vec<StopClip>)> {
        let report = self.model.as_ref()?.report();
        Some((report.fps, stop_clips(report)))
    }

    fn refresh_clips(&mut self) {
        self.clips = self
            .model
            .as_ref()
            .map(|model| clip_segments(model, self.snapshot.as_ref()))
            .unwrap_or_default();
    }
    /// The length of the timeline in seconds, edited timing included.
    pub(crate) fn duration_seconds(&self) -> Option<f64> {
        let report = self.model.as_ref()?.report();
        (report.fps > 0).then(|| report.total_frames as f64 / report.fps as f64)
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
    /// Moves the playhead. The timeline does not render again for it; the shell paints
    /// the playhead from [`Self::playhead`].
    pub fn set_position(&mut self, frame: usize) {
        if self.position != frame {
            self.anchor = Instant::now();
        }
        self.position = frame;
    }
    /// While playing the playhead moves on between frames, see [`Self::playhead`].
    pub fn set_playing(&mut self, playing: bool) {
        if self.playing != playing {
            self.anchor = Instant::now();
        }
        self.playing = playing;
    }
    pub fn playing(&self) -> bool {
        self.playing
    }
    /// The playhead in frames: the shown frame plus the part of the next one elapsed since it
    /// was shown while playing, never past the end of the timing-aware duration.
    fn playhead_frame(&self, total: usize, fps: f64) -> f64 {
        let ahead = if self.playing {
            (self.anchor.elapsed().as_secs_f64() * fps).min(1.)
        } else {
            0.
        };
        (self.position as f64 + ahead).min(total as f64)
    }
    pub fn renders(&self) -> u64 {
        self.renders
    }
    /// Where the shell paints the playhead, once the tracks have been laid out.
    pub fn playhead(&self) -> Option<PlayheadGeometry> {
        let (model, bounds) = (self.model.as_ref()?, self.bounds?);
        let inset = px(axis_inset());
        let axis = Bounds::new(
            bounds.origin + gpui::point(inset, Pixels::ZERO),
            gpui::size(
                (bounds.size.width - inset).max(Pixels::ZERO),
                bounds.size.height,
            ),
        );
        let seconds = self.playhead_frame(model.report().total_frames, model.report().fps as f64)
            / model.report().fps as f64;
        let x = f32::from(axis.left()) + self.viewport.x_at_seconds(seconds) as f32;
        Some(PlayheadGeometry { axis, x })
    }
    pub fn thumbnail_requests(&self) -> Vec<ThumbnailKey> {
        let Some(model) = &self.model else {
            return vec![];
        };
        let scale = thumbnail_scale(model);
        self.viewport
            .thumbnail_frames(model)
            .into_iter()
            .map(|frame| {
                ThumbnailKey::new(
                    model.report().envelope.identity.clone(),
                    model.report().timing_generation,
                    scale,
                    frame,
                )
            })
            .filter(|key| !self.cache.contains(key))
            .collect()
    }
    pub fn qualification_metrics(&self) -> serde_json::Value {
        // The ruler covers the time axis only: the track gutter sits to its left.
        let ruler = |b: Bounds<Pixels>| {
            [
                f32::from(b.left()) + axis_inset(),
                f32::from(b.top()),
                (f32::from(b.size.width) - axis_inset()).max(0.),
                METRICS.ruler_h,
            ]
        };
        let rect = |b: Bounds<Pixels>| {
            [
                f32::from(b.left()),
                f32::from(b.top()),
                f32::from(b.size.width),
                f32::from(b.size.height),
            ]
        };
        serde_json::json!({
            "entries": self.cache.len(), "bytes": self.cache.bytes(),
            "high_water_entries": self.cache.high_water_entries(), "high_water_bytes": self.cache.high_water_bytes(),
            "release_failures": self.release_failures,
            "pixels_per_second": self.viewport.pixels_per_second(), "scroll_x": self.viewport.scroll_x(),
            "zoom": self.viewport.zoom, "position_seconds": self.viewport.position,
            "clips": self.clips.len(), "painted_segments": self.painted_segments,
            "waveform_peaks": self.waveform.len(),
            "selection": {
                "scene": self.selection.scene_id,
                "range": self.selection.range.as_ref().map(|r| [r.start, r.end]),
                "position": self.position,
                "total_frames": self.model.as_ref().map(|m| m.report().total_frames),
            },
            "ruler_bounds": self.bounds.map(ruler),
            "minimap_bounds": self.minimap_bounds.map(rect),
            "edit": self.edit_metrics(),
            "stops": {
                "markers": self.markers().into_iter().map(|(source, frame, x)| serde_json::json!({
                    "stop": match source {
                        StopSource::Explicit(stop) => Some(stop),
                        StopSource::Default => None,
                    },
                    "frame": frame,
                    "x": x,
                })).collect::<Vec<_>>(),
                "selected": self.selected_stop,
                "drag": self.stop_drag.as_ref().map(|d| d.stop),
                "snapped": self.stop_drag.as_ref().and_then(|d| d.snapped),
            },
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
        }) && let Ok(image) = create_render_image(&frame.response.header, &frame.pixels)
        {
            // Cached on arrival, not at the next render: the timeline is not drawn while the
            // export panel or settings cover the editor, and a thumbnail that is not cached
            // is asked for again.
            let bytes =
                frame.response.header.width as usize * frame.response.header.height as usize * 4;
            self.retired.extend(self.cache.insert(key, image, bytes));
            cx.notify();
        }
    }
    /// Moves the keyboard focus to the tracks, as a click on them does.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }
    /// Moves the playhead to `frame` (as a click on the ruler does).
    pub fn seek(&mut self, frame: usize, cx: &mut Context<Self>) {
        if self
            .model
            .as_ref()
            .is_some_and(|m| m.report().total_frames > 0)
        {
            cx.emit(TimelineEvent::Seek(frame));
        }
    }
    fn pointer_frame(&self, x: Pixels) -> Option<usize> {
        let axis_x = f32::from(x - self.bounds?.left()) - axis_inset();
        if axis_x < 0. {
            return None;
        }
        self.viewport
            .frame_at_x(f64::from(axis_x), self.model.as_ref()?)
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
        // On the ruler a press near a marker grabs it, a double click adds a stop.
        let on_ruler = self.bounds.is_some_and(|b| {
            hit_zone(f32::from(event.position.y), f32::from(b.top())) == HitZone::Ruler
        });
        let mut frame = frame;
        if on_ruler && !event.modifiers.shift {
            let x = f32::from(event.position.x);
            let markers = self.markers();
            let hit = hit_marker(
                markers.iter().filter_map(|&(source, _, at)| match source {
                    StopSource::Explicit(stop) => Some((stop, at)),
                    StopSource::Default => None,
                }),
                x,
            );
            if let Some(stop) = hit {
                self.selected_stop = Some(stop);
                self.begin_stop_drag(stop, x, cx);
                cx.notify();
                return;
            }
            if event.click_count == 2 {
                self.add_stop(frame, cx);
                return;
            }
            // A press on a default stop's flag scrubs from the stop itself.
            frame = default_stop_hit(&markers, x).unwrap_or(frame);
        }
        let Some(model) = &self.model else {
            return;
        };
        let range = event.modifiers.shift
            || self.bounds.is_some_and(|b| {
                hit_zone(f32::from(event.position.y), f32::from(b.top())) == HitZone::Lanes
            });
        self.drag = Some((frame, range));
        if range {
            self.selection.select_range(frame, frame, model);
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
            if let Some(model) = &self.model {
                self.selection.select_range(start, frame, model);
            }
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
    fn minimap_down(&mut self, event: &gpui::MouseDownEvent, cx: &mut Context<Self>) {
        let (Some(model), Some(bar)) = (&self.model, self.minimap_bounds) else {
            return;
        };
        self.minimap_drag = MinimapDrag::begin(
            f32::from(event.position.x),
            f32::from(bar.left()),
            f32::from(bar.size.width),
            &mut self.viewport,
            model,
        );
        if self.minimap_drag.is_some() {
            cx.stop_propagation();
            cx.notify();
        }
    }
    fn minimap_move(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        if let (Some(drag), Some(model)) = (&self.minimap_drag, &self.model)
            && event.dragging()
        {
            self.viewport = drag.update(f32::from(event.position.x), model);
            cx.notify();
        }
    }
    /// Moves the playhead past the last frame.
    pub fn seek_to_end(&mut self, cx: &mut Context<Self>) {
        if let Some(total) = self.model.as_ref().map(|m| m.report().total_frames) {
            self.seek(total, cx);
        }
    }
    /// Selects the next scene under the playhead.
    pub fn cycle_scene(&mut self, cx: &mut Context<Self>) {
        let Some(model) = &self.model else {
            return;
        };
        self.selection.cycle_scene_at(self.position, model);
        self.emit_scope_changed(cx);
        cx.notify();
    }
    fn select_scene(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(model) = &self.model {
            self.selection.select_scene(index, model);
        }
        self.emit_scope_changed(cx);
        cx.notify();
    }
    /// Starts the selected range at the playhead (keeping its end, if any).
    pub fn mark_range_start(&mut self, cx: &mut Context<Self>) {
        let Some(model) = &self.model else {
            return;
        };
        let end = self
            .selection
            .range
            .as_ref()
            .map_or(self.position, |r| r.end);
        self.selection.select_range(self.position, end, model);
        self.emit_scope_changed(cx);
        cx.notify();
    }
    /// Ends the selected range at the playhead (keeping its start, if any).
    pub fn mark_range_end(&mut self, cx: &mut Context<Self>) {
        let Some(model) = &self.model else {
            return;
        };
        let start = self
            .selection
            .range
            .as_ref()
            .map_or(self.position, |r| r.start);
        self.selection.select_range(start, self.position, model);
        self.emit_scope_changed(cx);
        cx.notify();
    }
    /// Zooms by `factor` (`2.` doubles the pixels per second), keeping the playhead
    /// where it is on screen.
    pub fn zoom(&mut self, factor: f64, cx: &mut Context<Self>) {
        if let Some(model) = &self.model {
            self.viewport.zoom(
                factor,
                self.viewport.x_at_frame(self.position, model),
                model,
            );
            cx.notify();
        }
    }
    /// Shows the whole video.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        if let Some(model) = &self.model {
            self.viewport.fit(model);
            cx.notify();
        }
    }
    /// Scrolls the tracks by three quarters of their width, back (`-1.`) or forward (`1.`).
    pub fn page(&mut self, direction: f64, cx: &mut Context<Self>) {
        if let Some(model) = &self.model {
            self.viewport
                .scroll_by(direction * self.viewport.width * 0.75, model);
            cx.notify();
        }
    }
    /// Zoom to the slider `fraction`, keeping the playhead where it is on screen.
    pub fn set_zoom_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let target = fraction_to_zoom(fraction, self.fit_pixels_per_second());
        self.zoom(target / self.viewport.pixels_per_second(), cx);
    }
    /// The scale at which the whole video fills the axis.
    fn fit_pixels_per_second(&self) -> f64 {
        self.model
            .as_ref()
            .filter(|model| model.duration() > 0.)
            .map_or(ZOOM_MAX, |model| self.viewport.width / model.duration())
    }
}

/// What a drag on a clip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Trim(TrimEdge),
    Reorder,
}

/// A drag on a clip. The boxes stay where they were when it started while the preview
/// retimes under it, and only the dragged clip follows the pointer (Cap's ghost trim).
#[derive(Debug, Clone)]
struct EditDrag {
    kind: EditKind,
    /// The clip's position in the snapshot.
    clip: usize,
    down_x: f32,
    /// Set once the pointer moved [`DRAG_THRESHOLD_PX`]; a press that never moves is a click.
    moved: bool,
    layout: Vec<ClipSegment>,
    /// The clip's frames when the drag started.
    segment: (usize, usize),
    original: ResolvedClip,
    targets: SnapTargets,
    /// The trim last sent, or the position a reorder drops the clip at.
    value: Option<usize>,
    /// The time the dragged edge snapped to, in seconds.
    snapped: Option<f64>,
}

/// A drag of a slide stop's marker along the ruler. Like a clip drag it holds the clip
/// it started in: the stop stays inside that clip while the engine catches up.
#[derive(Debug, Clone)]
struct StopDrag {
    /// The stop's position in `slides.json`.
    stop: usize,
    clip: StopClip,
    /// `"end"` stops of the same key before this one, which decides whether the clip's
    /// end frame is written as `"end"`.
    ends_before: usize,
    down_x: f32,
    /// Set once the pointer moved [`DRAG_THRESHOLD_PX`]; a press that never moves seeks.
    moved: bool,
    /// The stop's video frame when the drag started.
    origin: usize,
    /// Where the marker is drawn now.
    frame: usize,
    targets: SnapTargets,
    /// The time the marker snapped to, in seconds.
    snapped: Option<f64>,
    /// The place last sent to the engine.
    sent: Option<studio_engine::slides::StopAt>,
}

impl EditDrag {
    /// The layout as drawn: the dragged clip at its trim, the others where they were.
    fn draft(&self) -> Vec<ClipSegment> {
        let mut layout = self.layout.clone();
        if let (EditKind::Trim(edge), Some(trim)) = (self.kind, self.value)
            && let Some(segment) = layout.iter_mut().find(|s| s.clip == Some(self.clip))
        {
            let (start, clip) = (self.segment.0, &self.original);
            match edge {
                TrimEdge::Start => {
                    segment.start = (start + trim).saturating_sub(clip.trim_start_frames);
                }
                TrimEdge::End => {
                    segment.end = start + clip.scene_frames - clip.trim_start_frames - trim;
                }
            }
        }
        layout
    }

    fn name(&self) -> &'static str {
        match self.kind {
            EditKind::Trim(TrimEdge::Start) => "trim-start",
            EditKind::Trim(TrimEdge::End) => "trim-end",
            EditKind::Reorder => "reorder",
        }
    }
}

impl TimelineView {
    /// The snapshot when the timeline can be edited now, else why it cannot.
    fn edit_state(&self) -> Result<&TimelineSnapshot, String> {
        match (&self.model, &self.snapshot) {
            (Some(_), Some(snapshot)) => match &snapshot.editing {
                TimingEditing::Enabled => Ok(snapshot),
                TimingEditing::Disabled(reason) => Err(reason.clone()),
            },
            _ => Err("Build the preview to edit the timeline".into()),
        }
    }

    /// Whether a clip or stop marker drag is live; the shell then follows the pointer
    /// through the window.
    pub fn edit_dragging(&self) -> bool {
        self.edit.is_some() || self.stop_drag.is_some()
    }

    /// Starts a drag of `clip` pressed at window x `x`. The timing document changes only
    /// once the pointer has moved [`DRAG_THRESHOLD_PX`].
    fn begin_edit(&mut self, kind: EditKind, clip: usize, x: f32, cx: &mut Context<Self>) -> bool {
        let Ok(snapshot) = self.edit_state() else {
            return false;
        };
        let (Some(model), Some(original)) = (&self.model, snapshot.clips.get(clip).cloned()) else {
            return false;
        };
        let Some(segment) = self
            .clips
            .iter()
            .find(|segment| segment.clip == Some(clip) && segment.scene.is_some())
        else {
            return false;
        };
        let fps = model.report().fps as f64;
        let audio = model
            .report()
            .audio_tracks
            .iter()
            .map(|track| (track.start_seconds, track.end_seconds));
        let drag = EditDrag {
            kind,
            clip,
            down_x: x,
            moved: false,
            layout: self.clips.clone(),
            segment: (segment.start, segment.end),
            original,
            targets: SnapTargets::for_trim(
                clip,
                &self.clips,
                audio,
                self.position as f64 / fps,
                fps,
            ),
            value: None,
            snapped: None,
        };
        self.edit = Some(drag);
        cx.notify();
        true
    }

    /// A pointer move while a clip drag is live, anywhere in the window: `x` is the window
    /// x, `snap_disabled` while Shift is held. A trim opens its gesture on the first move
    /// and sends each new trim; a reorder only tracks where the clip would land.
    pub fn edit_drag_move(&mut self, x: f32, snap_disabled: bool, cx: &mut Context<Self>) {
        if self.stop_drag.is_some() {
            self.stop_drag_move(x, snap_disabled, cx);
            return;
        }
        let pixels_per_second = self.viewport.pixels_per_second();
        let axis_x = self
            .bounds
            .map_or(0., |b| f32::from(px(x) - b.left()) - axis_inset());
        let pointer_seconds = self.viewport.seconds_at_x(f64::from(axis_x));
        let (Some(model), Some(drag)) = (&self.model, &mut self.edit) else {
            return;
        };
        if pixels_per_second <= 0. {
            return;
        }
        if !drag.moved {
            if (x - drag.down_x).abs() < DRAG_THRESHOLD_PX {
                return;
            }
            drag.moved = true;
            if matches!(drag.kind, EditKind::Trim(_)) {
                cx.emit(TimelineEvent::Edit(TimelineCommand::BeginGesture));
            }
        }
        let fps = model.report().fps as f64;
        match drag.kind {
            EditKind::Trim(edge) => {
                let edge_frame = match edge {
                    TrimEdge::Start => drag.segment.0,
                    TrimEdge::End => drag.segment.1,
                };
                let raw = edge_frame as f64 / fps + f64::from(x - drag.down_x) / pixels_per_second;
                // Once a trim lands only the end edge stays where it was dragged (a start
                // trim moves the clip's content, not its box), so only it snaps, as in Cap.
                let (time, snapped) = match edge {
                    TrimEdge::End => drag
                        .targets
                        .snap(raw, 1. / pixels_per_second, snap_disabled),
                    TrimEdge::Start => (raw, None),
                };
                drag.snapped = snapped;
                let to = (time * fps).round().max(0.) as usize;
                let trim = trim_for_edge(&drag.original, drag.segment.0, edge, to);
                if drag.value != Some(trim) {
                    drag.value = Some(trim);
                    cx.emit(TimelineEvent::Edit(TimelineCommand::Trim {
                        clip: drag.clip,
                        edge,
                        frames: trim,
                    }));
                }
            }
            EditKind::Reorder => {
                let at = (pointer_seconds * fps).max(0.) as usize;
                drag.value = Some(drop_position(&drag.layout, drag.clip, at));
            }
        }
        cx.notify();
    }

    /// Ends a clip drag: a trim closes its gesture, which makes it one undo step, and a
    /// reorder moves the clip. Runs on the release anywhere, on a move with no button held
    /// (a release outside the window), when the window loses focus and when the preview is
    /// replaced.
    pub fn end_edit_drag(&mut self, cx: &mut Context<Self>) {
        self.finish_edit_drag(true, cx);
    }

    /// A trim still closes its gesture, but a reorder is dropped: its indices belong to a
    /// layout that was replaced.
    fn abandon_edit_drag(&mut self, cx: &mut Context<Self>) {
        self.finish_edit_drag(false, cx);
    }

    fn finish_edit_drag(&mut self, commit_reorder: bool, cx: &mut Context<Self>) {
        self.finish_stop_drag(cx);
        let Some(drag) = self.edit.take() else {
            return;
        };
        if drag.moved {
            match (drag.kind, drag.value) {
                (EditKind::Trim(_), _) => {
                    cx.emit(TimelineEvent::Edit(TimelineCommand::EndGesture));
                }
                (EditKind::Reorder, Some(to)) if commit_reorder && to != drag.clip => {
                    cx.emit(TimelineEvent::Edit(TimelineCommand::Move {
                        clip: drag.clip,
                        to,
                    }));
                }
                (EditKind::Reorder, _) => {}
            }
        }
        cx.notify();
    }

    /// Drops a drag whose gesture the engine refused, asking nothing more of it.
    pub fn cancel_edit_drag(&mut self, cx: &mut Context<Self>) {
        if self.edit.take().is_some() || self.stop_drag.take().is_some() {
            cx.notify();
        }
    }

    /// The slides document when stops can be edited now, else why they cannot.
    fn slides_state(&self) -> Result<&SlidesDoc, String> {
        let slides = self
            .slides
            .as_ref()
            .filter(|_| self.model.is_some())
            .ok_or("Build the preview to place slide stops")?;
        match (&slides.editing, &slides.doc) {
            (SlidesEditing::Disabled(reason), _) => Err(reason.clone()),
            (SlidesEditing::Enabled, Some(doc)) => Ok(doc),
            (SlidesEditing::Enabled, None) => Err("slides.json cannot be read".into()),
        }
    }

    /// Every resolved stop with its frame and window x; a dragged stop is where the
    /// pointer put it.
    fn markers(&self) -> Vec<(StopSource, usize, f32)> {
        let (Some(model), Some(bounds)) = (&self.model, self.bounds) else {
            return Vec::new();
        };
        let axis = f32::from(bounds.left()) + axis_inset();
        self.marker_frames()
            .map(|(source, frame)| {
                let x = axis + self.viewport.x_at_frame(frame, model) as f32;
                (source, frame, x)
            })
            .collect()
    }

    /// Every resolved stop with the frame its marker is drawn at.
    fn marker_frames(&self) -> impl Iterator<Item = (StopSource, usize)> + '_ {
        self.slides
            .iter()
            .flat_map(|slides| &slides.stops)
            .map(|stop| match (&self.stop_drag, stop.source) {
                (Some(drag), StopSource::Explicit(index)) if index == drag.stop => {
                    (stop.source, drag.frame)
                }
                _ => (stop.source, stop.video_frame),
            })
    }

    /// Starts a drag of the marker of stop `stop` pressed at window x `x`. The document
    /// changes only once the pointer has moved [`DRAG_THRESHOLD_PX`]; a press that never
    /// moves seeks to the stop. Stops that cannot be edited now are only selected.
    fn begin_stop_drag(&mut self, stop: usize, x: f32, cx: &mut Context<Self>) {
        let Some((_, frame, _)) = self
            .markers()
            .into_iter()
            .find(|(source, ..)| *source == StopSource::Explicit(stop))
        else {
            return;
        };
        let drag = self.slides_state().ok().and_then(|doc| {
            let model = self.model.as_ref()?;
            let key = &doc.stops.get(stop)?.scene;
            let clips = stop_clips(model.report());
            let clip = clip_of(&clips, key, frame)?.clone();
            let fps = model.report().fps as f64;
            Some(StopDrag {
                stop,
                ends_before: ends_before(doc, key, stop),
                clip,
                down_x: x,
                moved: false,
                origin: frame,
                frame,
                targets: SnapTargets::for_stop(&clips, self.position as f64 / fps, fps),
                snapped: None,
                sent: None,
            })
        });
        match drag {
            Some(drag) => self.stop_drag = Some(drag),
            None => self.seek(frame, cx),
        }
    }

    /// A pointer move while a marker is dragged: the stop follows inside its clip and
    /// snaps like a trimmed edge (not while `snap_disabled`). The first move opens the
    /// gesture; each new place is sent.
    fn stop_drag_move(&mut self, x: f32, snap_disabled: bool, cx: &mut Context<Self>) {
        let pixels_per_second = self.viewport.pixels_per_second();
        let (Some(model), Some(drag)) = (&self.model, &mut self.stop_drag) else {
            return;
        };
        if pixels_per_second <= 0. {
            return;
        }
        if !drag.moved {
            if (x - drag.down_x).abs() < DRAG_THRESHOLD_PX {
                return;
            }
            drag.moved = true;
            cx.emit(TimelineEvent::Stop(SlidesCommand::BeginGesture));
        }
        let fps = model.report().fps as f64;
        let raw = drag.origin as f64 / fps + f64::from(x - drag.down_x) / pixels_per_second;
        let (time, snapped) = drag
            .targets
            .snap(raw, 1. / pixels_per_second, snap_disabled);
        let wanted = (time * fps).round().max(0.) as usize;
        drag.frame = drag.clip.clamp(wanted).unwrap_or(drag.origin);
        drag.snapped = snapped.filter(|_| drag.frame == wanted);
        let at = stop_at(&drag.clip, drag.frame, drag.ends_before);
        if drag.sent != Some(at) {
            drag.sent = Some(at);
            cx.emit(TimelineEvent::Stop(SlidesCommand::MoveStop {
                stop: drag.stop,
                scene: drag.clip.key.clone(),
                at,
            }));
        }
        cx.notify();
    }

    /// Ends a marker drag: a moved one closes its gesture, a press that never moved
    /// seeks to the stop.
    fn finish_stop_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.stop_drag.take() else {
            return;
        };
        if drag.moved {
            cx.emit(TimelineEvent::Stop(SlidesCommand::EndGesture));
        } else {
            self.seek(drag.origin, cx);
        }
        cx.notify();
    }

    /// Adds a stop at video frame `frame`, in the clip that plays it.
    fn add_stop(&mut self, frame: usize, cx: &mut Context<Self>) {
        let Ok(doc) = self.slides_state() else {
            return;
        };
        let Some(model) = &self.model else {
            return;
        };
        if let Some(stop) = new_stop(doc, &stop_clips(model.report()), frame) {
            cx.emit(TimelineEvent::Stop(SlidesCommand::AddStop(stop)));
        }
    }

    /// Adds a slide stop at the playhead.
    pub fn add_stop_at_playhead(&mut self, cx: &mut Context<Self>) {
        if !self.edit_dragging() {
            self.add_stop(self.position, cx);
        }
    }

    /// Removes the stop whose marker was pressed last.
    pub fn remove_selected_stop(&mut self, cx: &mut Context<Self>) {
        if self.edit_dragging() || self.slides_state().is_err() {
            return;
        }
        if let Some(stop) = self.selected_stop.take() {
            cx.emit(TimelineEvent::Stop(SlidesCommand::RemoveStop { stop }));
            cx.notify();
        }
    }

    /// Shows the marker of stop `stop` dragged to video frame `frame`, as a drag in
    /// progress draws it, without asking the engine for anything (the UI gallery).
    pub fn show_stop_drag(&mut self, stop: usize, frame: usize, cx: &mut Context<Self>) {
        let x = self
            .markers()
            .into_iter()
            .find(|(source, ..)| *source == StopSource::Explicit(stop))
            .map(|(.., x)| x);
        if let Some(x) = x {
            self.selected_stop = Some(stop);
            self.begin_stop_drag(stop, x, cx);
        }
        if let Some(drag) = &mut self.stop_drag {
            drag.moved = true;
            drag.frame = drag.clip.clamp(frame).unwrap_or(drag.origin);
            cx.notify();
        }
    }

    /// Splits the clip under the playhead (the selected scene's, when it is there).
    pub fn split_at_playhead(&mut self, cx: &mut Context<Self>) {
        if self.edit_state().is_err() || self.edit.is_some() {
            return;
        }
        let selected = self.model.as_ref().and_then(|model| {
            let id = self.selection.scene_id.as_ref()?;
            model
                .report()
                .scenes
                .iter()
                .position(|scene| &scene.instance_id == id)
        });
        if let Some((clip, at)) = split_at(&self.clips, self.position, selected) {
            cx.emit(TimelineEvent::Edit(TimelineCommand::Split { clip, at }));
        }
    }

    /// The edit state for the telemetry, with the window bounds of every clip's box.
    fn edit_metrics(&self) -> serde_json::Value {
        let state = self.edit_state();
        let clips: Vec<_> = match (&self.model, self.bounds) {
            (Some(model), Some(bounds)) => {
                let axis = f32::from(bounds.left()) + axis_inset();
                self.clips
                    .iter()
                    .filter_map(|segment| {
                        let left = axis + self.viewport.x_at_frame(segment.start, model) as f32;
                        let right = axis + self.viewport.x_at_frame(segment.end, model) as f32;
                        let top = f32::from(bounds.top()) + METRICS.ruler_h + row_top(segment.lane);
                        Some(serde_json::json!({
                            "clip": segment.clip?, "scene": segment.scene,
                            "frames": [segment.start, segment.end],
                            "bounds": [left, top, right - left, METRICS.track_h],
                        }))
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        serde_json::json!({
            "enabled": state.is_ok(),
            "reason": state.err(),
            "can_undo": self.snapshot.as_ref().is_some_and(|s| s.can_undo),
            "can_redo": self.snapshot.as_ref().is_some_and(|s| s.can_redo),
            "notice": self.snapshot.as_ref().and_then(|s| s.notice.clone()),
            "drag": self.edit.as_ref().map(EditDrag::name),
            "drop": self.edit.as_ref().filter(|d| d.kind == EditKind::Reorder).and_then(|d| d.value),
            "snapped": self.edit.as_ref().and_then(|d| d.snapped),
            "clips": clips,
        })
    }

    /// A trim handle on the `edge` of `clip`'s box, Cap's clip handles.
    fn trim_handle(
        &self,
        edge: TrimEdge,
        clip: usize,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let start = edge == TrimEdge::Start;
        div()
            .id((if start { "trim-start" } else { "trim-end" }, clip))
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(METRICS.layout.trim_handle_w))
            .when(start, |handle| handle.left_0())
            .when(!start, |handle| handle.right_0())
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(TRIM_GRIP_W))
                    .h(px(TRIM_GRIP_H))
                    .rounded_full()
                    .bg(accent),
            )
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .hover(move |handle| handle.bg(accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &gpui::MouseDownEvent, w, cx| {
                    if v.begin_edit(EditKind::Trim(edge), clip, f32::from(e.position.x), cx) {
                        w.focus(&v.focus, cx);
                        cx.stop_propagation();
                    }
                }),
            )
    }
}

fn thumbnail_scale(model: &TimelineModel) -> f64 {
    (160. / model.report().width as f64)
        .min(90. / model.report().height as f64)
        .min(1.)
}

/// The closest zoom (pixels per second); the transform clamps to it.
const ZOOM_MAX: f64 = studio_engine::MAX_PIXELS_PER_SECOND;
/// The grip drawn in a clip's trim handle.
const TRIM_GRIP_W: f32 = 2.;
const TRIM_GRIP_H: f32 = 16.;
/// The line of a selected or dragged stop, and of where a dragged clip lands.
const STRONG_LINE_W: f32 = 2.;
/// The thumbnail strip under the tracks and one thumbnail in it.
const THUMB_STRIP_H: f32 = 38.;
const THUMB_W: f32 = 64.;
const THUMB_H: f32 = 36.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitZone {
    Ruler,
    Lanes,
}

/// Which part of the timeline a pointer at window `y` hits, given the top of the
/// timeline area. The ruler scrubs, the lanes select.
fn hit_zone(y: f32, top: f32) -> HitZone {
    if y - top < METRICS.ruler_h {
        HitZone::Ruler
    } else {
        HitZone::Lanes
    }
}

/// The frame of the default stop whose flag a press at window `x` lands on. Default flags
/// are drawn small, but take presses as wide as an explicit marker does
/// ([`super::stops::STOP_HIT_PX`] either side, 12 px).
fn default_stop_hit(markers: &[(StopSource, usize, f32)], x: f32) -> Option<usize> {
    hit_marker(
        markers
            .iter()
            .filter(|(source, ..)| *source == StopSource::Default)
            .map(|&(_, frame, at)| (frame, at)),
        x,
    )
}

/// Width left of the time axis: the track gutter plus its gap.
fn axis_inset() -> f32 {
    let metrics = METRICS;
    metrics.track_gutter_w + metrics.space_2
}

/// Zoom (pixels per second) as a 0..=1 slider fraction on a log scale from Fit (`fit`)
/// to the closest zoom.
fn zoom_to_fraction(pixels_per_second: f64, fit: f64) -> f32 {
    if fit >= ZOOM_MAX || fit <= 0. {
        return 0.;
    }
    ((pixels_per_second.clamp(fit, ZOOM_MAX) / fit).ln() / (ZOOM_MAX / fit).ln()) as f32
}

/// Inverse of [`zoom_to_fraction`].
fn fraction_to_zoom(fraction: f32, fit: f64) -> f64 {
    let fit = fit.clamp(f64::MIN_POSITIVE, ZOOM_MAX);
    fit * (ZOOM_MAX / fit).powf(f64::from(fraction.clamp(0., 1.)))
}

impl Render for TimelineView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        for image in self.retired.drain(..) {
            self.release_failures += usize::from(window.drop_image(image).is_err());
        }
        let wanted = self.thumbnail_requests();
        if wanted != self.wanted {
            self.wanted = wanted;
            cx.emit(TimelineEvent::ThumbnailsWanted);
        }
        let theme = Theme::for_window(window, cx);
        let colors = theme.colors;
        let metrics = METRICS;
        let (can_undo, can_redo) = self
            .snapshot
            .as_ref()
            .map_or((false, false), |s| (s.can_undo, s.can_redo));
        let controls = TimelineControls {
            enabled: self
                .model
                .as_ref()
                .is_some_and(|m| m.report().total_frames > 0),
            zoom_fraction: zoom_to_fraction(
                self.viewport.pixels_per_second(),
                self.fit_pixels_per_second(),
            ),
            editable: self.edit_state().is_ok(),
            can_undo,
            can_redo,
        };
        if controls != self.controls {
            self.controls = controls;
            cx.emit(TimelineEvent::Controls(controls));
        }
        let editable = controls.editable;
        // The minimap sits at the end of the status line, under the tracks.
        let minimap = self.model.as_ref().map(|model| {
            let weak = cx.entity().downgrade();
            let fps = model.report().fps as f64;
            let boundaries = self.clips.iter().map(|clip| clip.start as f64 / fps);
            let interactive = zoomed_in(&self.viewport, model);
            render_minimap(
                &theme,
                &self.viewport,
                model,
                boundaries,
                metrics.layout.minimap_max_w,
            )
            .id("timeline-minimap")
            .relative()
            .flex_none()
            // It takes presses only while part of the video is hidden.
            .when(interactive, |minimap| {
                minimap.cursor(gpui::CursorStyle::OpenHand).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|v, e, _, cx| v.minimap_down(e, cx)),
                )
            })
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let _ = weak.update(cx, |view, _| view.minimap_bounds = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
        });
        let root = div()
            .flex()
            .flex_col()
            .gap(px(metrics.track_gap))
            .min_w_0()
            .flex_shrink_0()
            .size_full()
            .pl(px(metrics.timeline_pad_x))
            .pr(px(metrics.timeline_pad_x))
            .pt(px(metrics.timeline_pad_top))
            .pb(px(metrics.timeline_pad_bottom))
            .on_mouse_move(cx.listener(|v, e, _, cx| v.minimap_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _, _, _| v.minimap_drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, _, _, _| v.minimap_drag = None),
            );
        let Some(model) = &self.model else {
            self.painted_segments = 0;
            return root.child(
                EmptyState::new(&theme, "No compiled timeline")
                    .body("Build the preview to see its scenes, audio and slide stops.")
                    .centered(),
            );
        };
        let inset = axis_inset();
        let view_width = self.viewport.width as f32;
        let geometry = self.viewport.geometry(model);
        let stop_xs: Vec<f32> = self
            .marker_frames()
            .map(|(_, frame)| self.viewport.x_at_frame(frame, model) as f32)
            .collect();
        let weak = cx.entity().downgrade();
        let radius = px(metrics.radius.segment);
        let mut tracks = div()
            .id("compiled-timeline")
            .track_focus(&self.focus)
            .role(gpui::Role::Group)
            .aria_label("Compiled timeline: ruler scrubs, tracks select ranges")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .rounded(radius)
            .focus_visible(|s| s.shadow(focus_ring(&theme)))
            .key_context(KEY_CONTEXT)
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
                    let anchor = v.bounds.map_or(0., |b| {
                        f32::from(e.position.x - b.left() - px(axis_inset())) as f64
                    });
                    v.viewport.zoom((delta * 0.01).exp(), anchor.max(0.), model);
                } else {
                    v.viewport.scroll_by(-delta, model);
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.bounds = Some(bounds);
                            if let Some(model) = &view.model {
                                let width =
                                    (f32::from(bounds.size.width) - axis_inset()).max(0.) as f64;
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
            )
            .child(render_ruler(
                &theme,
                geometry.ticks,
                inset,
                view_width,
                &stop_xs,
            ));

        // Rows below the ruler: the clip track, overlay lanes, audio files and the mix.
        // Only segments inside the padded viewport are built (Cap's `isSegmentVisible`).
        let report = model.report();
        let fps = report.fps as f64;
        let pixels_per_second = self.viewport.pixels_per_second();
        let scene_lanes = geometry.scene_lane_count.max(1);
        let audio_count = report.audio_tracks.len();
        let mut painted = 0;
        let mut scene_rows: Vec<Vec<gpui::AnyElement>> =
            (0..scene_lanes).map(|_| Vec::new()).collect();
        let mut audio_rows: Vec<Vec<gpui::AnyElement>> =
            (0..audio_count).map(|_| Vec::new()).collect();
        let drawn = self
            .edit
            .as_ref()
            .map_or_else(|| self.clips.clone(), EditDrag::draft);
        let dragged = self.edit.as_ref().filter(|d| d.moved).map(|d| d.clip);
        for (number, clip) in drawn.iter().enumerate() {
            let (start, end) = (clip.start as f64 / fps, clip.end as f64 / fps);
            if !self.viewport.segment_visible(start, end) {
                continue;
            }
            let scene = clip.scene.map(|index| (index, &report.scenes[index]));
            let selected = scene
                .is_some_and(|(_, s)| self.selection.scene_id.as_ref() == Some(&s.instance_id));
            let element = segment(
                &theme,
                if clip.lane == 0 {
                    TrackKind::Clip
                } else {
                    TrackKind::Overlay
                },
                selected,
                self.viewport.x_at_seconds(start) as f32,
                ((end - start) * pixels_per_second).max(1.) as f32,
                view_width,
                clip.label.clone(),
                format!("{:.1}s", end - start),
            )
            .when(dragged.is_some() && dragged == clip.clip, |element| {
                element.opacity(metrics.opacity.dragging)
            })
            .when_some(
                clip.clip
                    .filter(|_| editable && self.edit.is_none() && scene.is_some()),
                |element, position| {
                    element
                        .child(self.trim_handle(
                            TrimEdge::Start,
                            position,
                            colors.accent.into(),
                            cx,
                        ))
                        .child(self.trim_handle(TrimEdge::End, position, colors.accent.into(), cx))
                },
            );
            painted += 1;
            scene_rows[clip.lane.min(scene_lanes - 1)].push(match scene {
                Some((index, scene)) => self
                    .selectable_scene(element, index, clip.clip, scene, colors.accent.into(), cx)
                    .into_any_element(),
                None => element
                    .id(("clip", number))
                    .aria_label(format!("Clip {}", clip.label))
                    .into_any_element(),
            });
        }
        for rect in geometry.audio {
            let audio = &report.audio_tracks[rect.index];
            painted += 1;
            audio_rows[rect.lane.min(audio_count - 1)].push(
                segment(
                    &theme,
                    TrackKind::Audio,
                    false,
                    rect.x as f32,
                    rect.width.max(1.) as f32,
                    view_width,
                    audio.file.clone(),
                    format!("at {:.3}s", audio.start_seconds),
                )
                .into_any_element(),
            );
        }
        self.painted_segments = painted;
        let mix = !self.waveform.is_empty() && audio_count > 0;
        let total_rows = scene_lanes + audio_count + usize::from(mix);
        let mut content = div().relative().h(px(
            (row_top(total_rows) - metrics.track_gap).max(metrics.track_h)
        ));
        for (lane, segments) in scene_rows.into_iter().enumerate() {
            content = content.child(track_row(
                &theme,
                row_top(lane),
                lane_name("Scenes", lane, scene_lanes),
                Icon::Layers,
                segments,
            ));
        }
        for (track, segments) in audio_rows.into_iter().enumerate() {
            content = content.child(track_row(
                &theme,
                row_top(scene_lanes + track),
                lane_name("Audio", track, audio_count),
                Icon::Volume2,
                segments,
            ));
        }
        if mix {
            content = content.child(track_row(
                &theme,
                row_top(scene_lanes + audio_count),
                "Mix".into(),
                Icon::Volume2,
                vec![self.mix_waveform(model).into_any_element()],
            ));
        }
        let lanes = div()
            .id("timeline-lanes")
            .absolute()
            .top(px(metrics.ruler_h))
            .bottom_0()
            .w_full()
            .overflow_y_scroll()
            .child(content);
        tracks = tracks.child(lanes);

        // The selected range over the time axis; the shell paints the playhead.
        if let Some(range) = &self.selection.range {
            let start = self.viewport.x_at_frame(range.start, model);
            let end = self.viewport.x_at_frame(range.end, model);
            tracks = tracks.child(
                div()
                    .absolute()
                    .left(px(inset))
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left(px(start as f32))
                            .top(px(metrics.ruler_h))
                            .bottom_0()
                            .w(px((end - start) as f32))
                            .bg(theme.colors.selection)
                            .border_l_1()
                            .border_r_1()
                            .border_color(colors.accent),
                    ),
            );
        }

        // Where a dragged clip lands, or the target a dragged edge snapped to.
        let marker = self
            .edit
            .as_ref()
            .filter(|d| d.moved)
            .and_then(|drag| match drag.kind {
                EditKind::Reorder => drag
                    .value
                    .and_then(|to| drop_marker(&drag.layout, drag.clip, to))
                    .map(|frame| self.viewport.x_at_frame(frame, model)),
                EditKind::Trim(_) => drag
                    .snapped
                    .map(|seconds| self.viewport.x_at_seconds(seconds)),
            });
        if let Some(x) = marker {
            tracks = tracks.child(
                div()
                    .absolute()
                    .left(px(inset))
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left(px(x as f32 - STRONG_LINE_W / 2.))
                            .top(px(metrics.ruler_h))
                            .bottom_0()
                            .w(px(STRONG_LINE_W))
                            .bg(colors.accent),
                    ),
            );
        }

        // Slide stops: a flag on the ruler and a line through the lanes. Default stops
        // are hollow and dashed; the selected or dragged marker is drawn stronger, and a
        // dragged one shows its time beside it.
        if self.slides.is_some() {
            let layout = metrics.layout;
            let mut overlay = div()
                .absolute()
                .left(px(inset))
                .right_0()
                .top_0()
                .bottom_0()
                .overflow_hidden();
            let accent: Hsla = colors.accent.into();
            let muted: Hsla = colors.text_3.into();
            let flag_radius = px(layout.stop_flag_radius);
            let dragged = self.stop_drag.as_ref().filter(|d| d.moved).map(|d| d.stop);
            let mut dashed = Vec::new();
            let mut readout = None;
            for (source, frame) in self.marker_frames() {
                let x = self.viewport.x_at_frame(frame, model) as f32;
                if x < -layout.stop_flag_w || x > view_width + layout.stop_flag_w {
                    continue;
                }
                let (explicit, strong) = match source {
                    StopSource::Explicit(stop) => (
                        true,
                        Some(stop) == self.selected_stop || Some(stop) == dragged,
                    ),
                    StopSource::Default => (false, false),
                };
                if let StopSource::Explicit(stop) = source
                    && Some(stop) == dragged
                {
                    readout = Some((x, frame));
                }
                let color = if explicit { accent } else { muted };
                let pole = if strong {
                    STRONG_LINE_W
                } else {
                    metrics.size.divider
                };
                // The pole stays inside the track at its end.
                let pole_left = (x - pole / 2.).min(view_width - pole);
                // At the end of the track the flag points back so it is not cut off.
                let left = flag_left(x, view_width);
                let flag = div()
                    .absolute()
                    .left(px(left))
                    .top(px(layout.stop_flag_top))
                    .w(px(layout.stop_flag_w))
                    .h(px(layout.stop_flag_h));
                let flag = if left < x {
                    flag.rounded_tl(flag_radius).rounded_bl(flag_radius)
                } else {
                    flag.rounded_tr(flag_radius).rounded_br(flag_radius)
                };
                overlay = overlay
                    .child(
                        div()
                            .absolute()
                            .left(px(pole_left))
                            .top(px(layout.stop_flag_top))
                            .h(px(metrics.ruler_h - layout.stop_flag_top))
                            .w(px(pole))
                            .bg(color),
                    )
                    .child(if explicit {
                        flag.bg(color)
                    } else {
                        flag.border_1().border_color(color)
                    });
                if explicit {
                    overlay = overlay.child(
                        div()
                            .absolute()
                            .left(px(pole_left))
                            .top(px(metrics.ruler_h))
                            .bottom_0()
                            .w(px(pole))
                            .bg(color.opacity(if strong {
                                1.
                            } else {
                                metrics.opacity.stop_default
                            })),
                    );
                } else {
                    dashed.push(x);
                }
            }
            if !dashed.is_empty() {
                let top = metrics.ruler_h;
                let (dash, line_w) = (layout.stop_dash, metrics.size.divider);
                overlay = overlay.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            for x in &dashed {
                                let mut y = bounds.top() + px(top);
                                while y < bounds.bottom() {
                                    window.paint_quad(gpui::fill(
                                        Bounds::new(
                                            gpui::point(bounds.left() + px(x - line_w / 2.), y),
                                            gpui::size(px(line_w), px(dash)),
                                        ),
                                        muted,
                                    ));
                                    y += px(dash * 2.);
                                }
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                );
            }
            // The dragged stop's time, under the ruler on the side of the flag with room.
            if let Some((x, frame)) = readout {
                let gap = metrics.space_1;
                let pill = div()
                    .debug_selector(|| "stop-readout".into())
                    .absolute()
                    .top(px(metrics.ruler_h + gap));
                let pill = if x > view_width / 2. {
                    pill.right(px(view_width - x + gap))
                } else {
                    pill.left(px(x + gap))
                };
                overlay = overlay.child(pill.child(Toast::new(
                    &theme,
                    "stop-readout",
                    format!("{:.2}s · frame {frame}", frame as f64 / fps),
                )));
            }
            tracks = tracks.child(overlay);
        }

        let mut thumbs = div()
            .relative()
            .flex_none()
            .h(px(THUMB_STRIP_H))
            .overflow_hidden();
        for frame in self.viewport.thumbnail_frames(model) {
            let key = ThumbnailKey::new(
                report.envelope.identity.clone(),
                report.timing_generation,
                thumbnail_scale(model),
                frame,
            );
            if let Some(image) = self.cache.get(&key) {
                thumbs = thumbs.child(
                    div()
                        .absolute()
                        .left(px(self.viewport.x_at_frame(frame, model) as f32 + inset))
                        .w(px(THUMB_W))
                        .h(px(THUMB_H))
                        .overflow_hidden()
                        .child(gpui::img(image).w(px(THUMB_W)).h(px(THUMB_H))),
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
        let status = text_style(div(), TextStyle::Small)
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(colors.text_3)
            .child(selectable(format!(
                "{selected} · {}",
                match (
                    self.edit_state(),
                    self.snapshot.as_ref().and_then(|s| s.notice.as_ref())
                ) {
                    (Err(reason), _) => reason,
                    (Ok(_), Some(notice)) => notice.clone(),
                    (Ok(_), None) =>
                        "drag edges to trim, clips to reorder · C splits · Shift-drag for range · double-click the ruler adds a slide stop"
                            .into(),
                }
            )));
        root.child(tracks).child(thumbs).child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(metrics.space_2))
                .child(status)
                .children(minimap),
        )
    }
}

impl TimelineView {
    /// A scene segment that selects its scene on click or Enter. A press on a clip may
    /// also start dragging it to another place.
    fn selectable_scene(
        &self,
        element: gpui::Div,
        index: usize,
        clip: Option<usize>,
        scene: &fframes_studio_protocol::PreviewSceneInfo,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        element
            .id(format!("scene-{}", scene.instance_id))
            .role(gpui::Role::Button)
            .aria_label(format!("Scene {} instance {}", scene.name, scene.index))
            .tab_index(0)
            .focus_visible(move |s| s.border_color(accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &gpui::MouseDownEvent, w, cx| {
                    if e.modifiers.shift {
                        return;
                    }
                    v.select_scene(index, cx);
                    if let Some(clip) = clip {
                        v.begin_edit(EditKind::Reorder, clip, f32::from(e.position.x), cx);
                    }
                    w.focus(&v.focus, cx);
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(move |v, e: &gpui::KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    v.select_scene(index, cx);
                    cx.stop_propagation();
                }
            }))
    }

    /// The mix row's waveform over the visible seconds.
    fn mix_waveform(&self, model: &TimelineModel) -> impl IntoElement {
        let peaks = self.waveform.clone();
        let start = self.viewport.position.max(0.);
        let end = (self.viewport.position + self.viewport.zoom).min(model.duration());
        let x = self.viewport.x_at_seconds(start) as f32;
        let width = ((end - start) * self.viewport.pixels_per_second()) as f32;
        let color = Hsla::from(track_color(TrackKind::Audio)).opacity(METRICS.opacity.muted_media);
        let pad = METRICS.layout.waveform_pad;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let area = Bounds::new(
                    bounds.origin + gpui::point(px(x), px(pad)),
                    gpui::size(px(width), bounds.size.height - px(pad)),
                );
                if end > start
                    && let Some(path) = waveform_path(&peaks, (start, end), area)
                {
                    window.paint_path(path, color);
                }
            },
        )
        .absolute()
        .size_full()
    }
}

#[cfg(test)]
mod tests;
