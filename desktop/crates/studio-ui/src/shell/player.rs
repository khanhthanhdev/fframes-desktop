//! The player: preview frames, playback and audio, canvas selection, and the player pane's toolbar
//! and stage.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, canvas, div, px,
};
use studio_engine::TaskScope;
use studio_engine::engine::handle::{PreviewGated, PreviewNews};
use studio_engine::services::audio_service::{AudioEvent, OutputDevice};
use studio_engine::services::preview_coordinator::SeekIntent;
use studio_theme::{METRICS, Shadow, TextStyle, shadow_for, text_style};

use super::keymap::PLAYER;
use super::transport::{show_total_time, transport_mode};
use super::{PreparedInstall, StudioShell};
use crate::actions;
use crate::canvas_view::StageLayout;
use crate::kit::{
    Badge, EmptyState, Icon, Notice, Toast, ToastHost, Tone, focus_ring, icon, selectable,
    with_tooltip,
};

/// The canvas message's toast: the host, and the message it last showed, so a message that
/// is set again on every frame shows once.
#[derive(Debug, Default)]
pub(super) struct CanvasToast {
    host: ToastHost,
    seen: Option<String>,
}

impl StudioShell {
    pub(super) fn detach_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(workflow) = &self.agent.workflow {
            let _ = workflow.set_displayed_preview_identity(None);
        }
        if let Some(preview) = self.preview.take() {
            // Teardown stays off the UI thread; the root token rejects any late spawn.
            std::thread::spawn(move || preview.close());
        }
        self.preview_session = None;
        self.timing_sent = None;
        self.props_sent = None;
        self.awaiting_preview = None;
        self.preview_state.close();
        self.displayed = None;
        self.pending_ready = None;
        self.pending_frame = None;
        self.canvas.clear();
        self.sync_props_selection(cx);
        self.canvas_pan_position = None;
        self.clear_images = true;
        let _ = self
            .transport
            .reinstall(30, 0, 0, self.clock_start.elapsed().as_secs_f64());
        self.audio.stop(self.transport.epoch());
        self.output = None;
        self.audio_ready_epoch = None;
        self.resume_install = false;
        self.audio_note = "Audio output not prepared".into();
        self.resume_scrub = false;
        self.timeline.update(cx, |v, cx| v.clear(cx));
    }

    /// Takes the preview coordinator's news. Held back while a command runs or earlier
    /// news is with the engine; both drain again when they finish.
    pub(super) fn drain_preview(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.preview_gate_pending {
            return;
        }
        let Some(preview) = &self.preview else {
            return;
        };
        let events = preview.events();
        if events.stop_scan_changed {
            cx.notify();
        }
        // The engine hears the layout the window installed, so the slide stops follow it.
        let retimed = events.retimed.and_then(|retimed| self.retimed(retimed, cx));
        if let Some((key, frame)) = events.thumbnail {
            self.timeline
                .update(cx, |v, cx| v.thumbnail(key, frame, cx));
            self.thumbnails_sent = None;
            self.request_thumbnails(cx);
        }
        if let Some(frame) = events.frame
            && self.presents_frame(&frame)
        {
            self.pending_frame = Some(frame);
            self.notify_player(cx);
        }
        if let Some(Err(error)) = events.props_applied {
            self.error = Some(format!("The props edit could not be previewed: {error}"));
            cx.notify();
        }
        if let Some((identity, info)) = events.props_info {
            self.props_panel
                .update(cx, |panel, cx| panel.set_info(identity, info, cx));
        }
        if events.compiled.is_none()
            && events.ready.is_none()
            && events.error.is_none()
            && retimed.is_none()
        {
            return;
        }
        self.preview_gate_pending = true;
        self.engine.preview_news(PreviewNews {
            compiled: events.compiled,
            error: events.error,
            ready: events.ready,
            retimed,
        });
    }

    /// The displayed preview laid out with a new timing document: its timeline, length and
    /// audio change, the worker stays. Playback stops; the playhead keeps its frame number
    /// and its frame is asked for again. Returns the preview when it was installed.
    fn retimed(
        &mut self,
        retimed: Result<
            std::sync::Arc<studio_engine::ReadyPreview>,
            (studio_engine::OperationTag, String),
        >,
        cx: &mut Context<Self>,
    ) -> Option<std::sync::Arc<studio_engine::ReadyPreview>> {
        let ready = match retimed {
            Ok(ready) => ready,
            Err((_, error)) => {
                self.error = Some(format!("The timeline edit could not be previewed: {error}"));
                cx.notify();
                return None;
            }
        };
        let model = match studio_engine::TimelineModel::new(ready.timeline.clone()) {
            Ok(model) => model,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return None;
            }
        };
        if self.preview_state.retime(&ready).is_err() {
            return None;
        }
        let resume = self.transport.playing();
        self.pending_ready = None;
        self.resume_install = false;
        if let Err(error) = self.transport.reinstall(
            ready.timeline.fps,
            ready.timeline.total_frames,
            self.preview_state.position(),
            self.clock_start.elapsed().as_secs_f64(),
        ) {
            self.error = Some(error.to_string());
        }
        self.audio.stop(self.transport.epoch());
        self.output = None;
        self.audio_ready_epoch = None;
        let position = self.preview_state.position();
        let waveform = ready
            .audio_source
            .as_ref()
            .map(|source| source.waveform().clone())
            .unwrap_or_else(|| std::sync::Arc::from([]));
        self.timeline
            .update(cx, |v, cx| v.install(model, waveform, position, cx));
        self.displayed = Some(ready.clone());
        self.thumbnails_sent = None;
        self.request_frame(position, cx);
        self.request_thumbnails(cx);
        self.after_retime(resume, cx);
        cx.notify();
        Some(ready)
    }

    /// The clock was reinstalled for a new layout: playback that ran goes on, except
    /// while presenting, which holds at the nearest stop (again once the new stops
    /// arrive, see `sync_deck`).
    pub(super) fn after_retime(&mut self, was_playing: bool, cx: &mut Context<Self>) {
        if self.presenting.is_some() {
            self.hold_at_nearest_stop(cx);
        } else if was_playing {
            self.toggle_playback(cx);
        }
    }

    /// Asks the coordinator for the timeline's visible thumbnails when they changed;
    /// nothing while playing.
    pub(super) fn request_thumbnails(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = &self.preview else {
            return;
        };
        let requests = if self.transport.playing() {
            vec![]
        } else {
            self.timeline.read(cx).thumbnail_requests()
        };
        if self.thumbnails_sent.as_ref() != Some(&requests) {
            preview.thumbnails(requests.clone());
            self.thumbnails_sent = Some(requests);
        }
    }

    pub(super) fn preview_gated(&mut self, gated: PreviewGated, cx: &mut Context<Self>) {
        let PreviewGated {
            ready,
            state,
            error,
            presentation,
            ..
        } = gated;
        self.preview_gate_pending = false;
        if let Some(p) = presentation {
            self.presentation = p;
            self.push_presentation_views(cx);
        }
        if let Some((tag, e)) = error {
            self.preview_state.fail(&tag, e.clone());
            self.discard_install(cx);
            self.error = Some(e);
        } else if let (Some(ready), Some(state)) = (ready, state) {
            // A presentation never starts playback or swaps the video under the
            // presenter: the build waits until it ends.
            if let Some(present) = &mut self.presenting {
                present.hold_build(ready);
                self.notify_player(cx);
            } else {
                self.offer_ready(ready, &state, cx);
            }
        }
        cx.notify();
    }

    /// A ready build the engine allows: installed when it is still current, after a
    /// pause when playback runs.
    pub(super) fn offer_ready(
        &mut self,
        ready: std::sync::Arc<studio_engine::ReadyPreview>,
        state: &studio_engine::ProjectState,
        cx: &mut Context<Self>,
    ) {
        // A staged preview of just-published bytes carries the promotion
        // tag (base source = the published revision, the task's own
        // generation); it is installable only under the engine's current
        // authorization, never as a build of the task base.
        let promoted = state.promotion().is_some_and(|p| p.tag() == ready.tag());
        if !promoted && !state.is_current_build(ready.tag()) {
            if let Some(preview) = &self.preview {
                preview.cancel_build();
            }
            self.preview_state.cancel_build();
        } else if self.transport.playing()
            && (promoted
                || matches!(state.job(), studio_engine::JobState::Succeeded(tag) if tag == ready.tag()))
        {
            // Re-evaluate the audible position, then re-prime the candidate
            // under the newly frozen seek serial. Old playback served the
            // entire compilation/preparation, not the compiler's cursor.
            self.resume_install = true;
            self.pause_playback(cx);
        } else if self.preview_state.can_install(&ready, state).is_ok() {
            let mut clock = self.transport.clone();
            if let Err(e) = clock.reinstall(
                ready.timeline.fps,
                ready.timeline.total_frames,
                ready.position,
                self.clock_start.elapsed().as_secs_f64(),
            ) {
                self.error = Some(e.to_string());
            } else {
                // Reserve the staged epoch in the active clock as well.
                // A seek during preparation must advance past this epoch,
                // never collide with a callback for another revision.
                let _ = self.transport.wait_for_output();
                self.audio.stage(
                    ready.clone(),
                    clock.epoch(),
                    ready.position,
                    clock.end_frame(),
                    self.output_device.clone(),
                );
                self.output = None;
                self.audio_ready_epoch = None;
                self.pending_ready = Some(PreparedInstall {
                    ready,
                    clock,
                    audio: None,
                    audio_ready: false,
                });
                self.audio_note = "Priming matching audio and first frame".into();
            }
        }
        cx.notify();
    }

    pub(super) fn seek_preview(&mut self, position: usize, cx: &mut Context<Self>) {
        let resume = std::mem::take(&mut self.resume_install);
        self.pending_ready = None;
        if let Err(e) = self
            .transport
            .seek(position, self.clock_start.elapsed().as_secs_f64())
        {
            self.error = Some(e.to_string());
            return;
        }
        self.audio.stop(self.transport.epoch());
        self.output = None;
        self.audio_ready_epoch = None;
        self.request_frame(self.transport.position(), cx);
        if resume && !self.transport.playing() {
            self.toggle_playback(cx);
        }
        if self.transport.playing() {
            self.prime_playback(cx);
        }
        self.schedule_playback_timer(cx);
    }

    pub(super) fn request_frame(&mut self, position: usize, cx: &mut Context<Self>) {
        if self.send_frame_request(position, cx) {
            cx.notify();
        }
    }

    /// A new frame or playhead during playback: only the player section renders again, the
    /// cached sections stay as they are.
    pub(super) fn notify_player(&mut self, cx: &mut Context<Self>) {
        match &self.sections {
            Some(sections) => sections.player.update(cx, |_, cx| cx.notify()),
            None => cx.notify(),
        }
    }

    /// Asks the coordinator for the frame at `position`; false when nothing is shown or
    /// the seek was refused.
    fn send_frame_request(&mut self, position: usize, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.preview_state.displayed().cloned() else {
            return false;
        };
        let scale = self.displayed.as_ref().map_or(1., |d| {
            crate::frame_image::preview_scale(
                d.timeline.width,
                d.timeline.height,
                self.preview_extent,
            )
        });
        let Ok(serial) = self.preview_state.seek(position, scale) else {
            return false;
        };
        if self.requested_clock_epoch != self.transport.epoch() {
            self.requested_clock_epoch = self.transport.epoch();
            self.epoch_serial = serial;
        }
        if let Some(preview) = &self.preview {
            preview.seek(SeekIntent {
                identity: id,
                serial,
                position: self.preview_state.position(),
                scale: self.preview_state.scale(),
            });
        }
        let position = self.preview_state.position();
        self.timeline.update(cx, |v, _| v.set_position(position));
        self.forward_playhead();
        true
    }

    /// Whether a finished frame goes on screen: the frame asked for last or, while playing,
    /// a frame of this run newer than the one shown. Without the second, a render slower
    /// than one frame interval would never be shown, since the playhead moved on meanwhile.
    pub(super) fn presents_frame(&self, f: &studio_engine::PreviewFrame) -> bool {
        self.requested_clock_epoch == self.transport.epoch()
            && (self.preview_state.accepts_frame(f)
                || self.transport.playing()
                    && f.response.seek_serial > self.presented_serial
                    && self.preview_state.accepts_late_frame(f, self.epoch_serial))
    }

    pub(super) fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.transport.playing() || self.resume_install {
            self.resume_install = false;
            self.pause_playback(cx);
        } else {
            self.pending_ready = None;
            if let Err(e) = self
                .transport
                .toggle(self.clock_start.elapsed().as_secs_f64())
            {
                self.error = Some(e.to_string());
            } else if self.transport.playing() {
                // Every start passes through a matching first-frame/PCM epoch.
                let _ = self.transport.wait_for_output();
                self.request_frame(self.transport.position(), cx);
                self.prime_playback(cx);
            }
        }
        self.schedule_playback_timer(cx);
        cx.notify();
    }

    pub(super) fn pause_playback(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self
            .transport
            .pause(self.clock_start.elapsed().as_secs_f64())
        {
            self.error = Some(e.to_string());
        }
        self.pending_ready = None;
        self.audio.stop(self.transport.epoch());
        self.output = None;
        self.audio_ready_epoch = None;
        self.request_frame(self.transport.position(), cx);
        self.schedule_playback_timer(cx);
        self.request_thumbnails(cx);
    }

    pub(super) fn pause_at_painted_frame(&mut self, cx: &mut Context<Self>) {
        if !self.transport.playing() {
            return;
        }
        let frame = self
            .painted_frame
            .unwrap_or_else(|| self.transport.position());
        self.pause_playback(cx);
        self.seek_preview(frame, cx);
    }

    pub(super) fn install_task_scope(
        &mut self,
        scope: Result<Option<TaskScope>, String>,
        cx: &mut Context<Self>,
    ) {
        self.sync_props_selection(cx);
        let scope = scope.and_then(|scope| match scope {
            Some(mut scope) => {
                scope.canvas_selection = self.canvas.task_scope_selection();
                scope.validate().map_err(|error| error.to_string())?;
                Ok(Some(scope))
            }
            None if self.canvas.task_scope_selection().is_some() => {
                Err("canvas selection has no matching compiled timeline".into())
            }
            None => Ok(None),
        });
        self.panel
            .update(cx, |panel, cx| panel.set_task_scope(scope, cx));
    }

    fn canvas_point(&self, position: gpui::Point<gpui::Pixels>) -> (f64, f64) {
        (
            f64::from(position.x) - self.canvas_origin.0,
            f64::from(position.y) - self.canvas_origin.1,
        )
    }

    fn canvas_pointer_down(&mut self, event: &gpui::MouseDownEvent, cx: &mut Context<Self>) {
        let point = self.canvas_point(event.position);
        if event.button != MouseButton::Middle {
            self.canvas_pan_position = None;
        }
        if event.button == MouseButton::Left {
            self.pause_at_painted_frame(cx);
        }
        match event.button {
            MouseButton::Middle => self.canvas_pan_position = Some(point),
            MouseButton::Left if event.modifiers.shift => {
                if !self.canvas.begin_rectangle(point.0, point.1) {
                    self.canvas.error =
                        Some("Start a rectangle drag inside the painted video image.".into());
                }
            }
            MouseButton::Left => {
                return self.select_canvas(point, event.modifiers.alt, cx);
            }
            _ => return,
        }
        self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
        cx.notify();
    }

    /// Selects the object at canvas `point` (`cycle`: the next one under it) and makes the
    /// selection the agent's task scope.
    pub(super) fn select_canvas(&mut self, point: (f64, f64), cycle: bool, cx: &mut Context<Self>) {
        // An empty stage has nothing to select; its own message says how to get a preview.
        if self.displayed.is_none() {
            return;
        }
        self.canvas.error = None;
        if let Err(error) = self.canvas.select_at(point.0, point.1, cycle) {
            self.canvas.error = Some(error.to_string());
        }
        self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
        cx.notify();
    }

    /// Where video pixel (`x`, `y`) is painted on the canvas, if the video is laid out.
    pub(super) fn video_to_canvas(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let viewport = self.canvas.viewport.as_ref()?;
        let image = viewport.image_bounds();
        let (width, height) = viewport.video_dimensions();
        Some((
            image.x + x * image.width / f64::from(width),
            image.y + y * image.height / f64::from(height),
        ))
    }

    fn canvas_pointer_move(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        let point = self.canvas_point(event.position);
        if self.canvas.is_drawing_rectangle() {
            self.canvas.update_rectangle(point.0, point.1);
            cx.notify();
        } else if event.pressed_button == Some(MouseButton::Middle)
            && let Some(previous) = self.canvas_pan_position.replace(point)
            && let Some(viewport) = &mut self.canvas.viewport
        {
            let _ = viewport.pan_by(point.0 - previous.0, point.1 - previous.1);
            cx.notify();
        }
    }

    fn canvas_pointer_up(&mut self, button: MouseButton, cx: &mut Context<Self>) {
        if button == MouseButton::Middle {
            self.canvas_pan_position = None;
        } else if button == MouseButton::Left && self.canvas.is_drawing_rectangle() {
            if let Some(rect) = self.canvas.end_rectangle() {
                self.canvas.message = Some(format!(
                    "Rectangle scope · {:.0}×{:.0} video pixels · source identity is not inferred.",
                    rect.width, rect.height
                ));
            }
            self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
            cx.notify();
        }
    }

    fn canvas_scroll(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        let point = self.canvas_point(event.position);
        let delta = match event.delta {
            gpui::ScrollDelta::Pixels(pixels) => f64::from(pixels.y),
            gpui::ScrollDelta::Lines(lines) => f64::from(lines.y * 32.0),
        };
        if event.modifiers.control || event.modifiers.platform {
            let factor = if delta > 0.0 {
                1.0 + delta.abs() * 0.01
            } else {
                1.0 / (1.0 + delta.abs() * 0.01)
            };
            if let Some(viewport) = &mut self.canvas.viewport {
                let _ = viewport.zoom_at(point.0, point.1, factor);
            }
        } else if let Some(viewport) = &mut self.canvas.viewport {
            let _ = viewport.pan_by(-delta, 0.0);
        }
        cx.notify();
    }

    pub(super) fn cycle_canvas_selection(&mut self, cx: &mut Context<Self>) {
        if self.displayed.is_none() {
            return;
        }
        self.canvas.error = None;
        if let Err(error) = self.canvas.cycle_last_hit() {
            self.canvas.error = Some(error.to_string());
        }
        self.sync_props_selection(cx);
        cx.notify();
    }

    pub(super) fn clear_canvas_selection(&mut self, cx: &mut Context<Self>) {
        self.canvas.clear_selection();
        self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
        cx.notify();
    }

    pub(super) fn reset_canvas_view(&mut self, cx: &mut Context<Self>) {
        if let Some(viewport) = &mut self.canvas.viewport {
            viewport.reset_to_fit();
        }
        cx.notify();
    }

    fn prime_playback(&mut self, cx: &mut Context<Self>) {
        if let Some(ready) = &self.displayed {
            self.output = None;
            self.audio_ready_epoch = None;
            // Silent from the frame playback stops at: a slide stop while presenting.
            self.audio.stage(
                ready.clone(),
                self.transport.epoch(),
                self.transport.position(),
                self.transport.end_frame(),
                self.output_device.clone(),
            );
            self.audio_note = "Buffering · waiting for matching first frame and PCM".into();
            cx.notify();
        }
    }

    pub(super) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.audio.mute(self.muted);
        cx.notify();
    }

    pub(super) fn change_output(&mut self, next: OutputDevice, cx: &mut Context<Self>) {
        self.output_device = next;
        self.seek_preview(self.transport.position(), cx);
        if !self.transport.playing() {
            self.prime_playback(cx);
        }
    }

    pub(super) fn cycle_output(&mut self, cx: &mut Context<Self>) {
        let next = self.output_device.next(&self.audio.devices());
        self.change_output(next, cx);
    }

    /// The output device and its clock, or why there is no audio: the player toolbar and
    /// Settings → General show it.
    pub(super) fn audio_status(&self) -> String {
        self.output.as_ref().map_or_else(
            || self.audio_note.clone(),
            |output| {
                let metrics = output.metrics();
                format!(
                    "{} · {} Hz · predicted output clock{}{} · latency {:.1}ms · underruns {}",
                    output.device,
                    output.sample_rate,
                    if self.muted { " · muted" } else { "" },
                    if metrics.invalid_timestamps > 0 {
                        " (latency estimate)"
                    } else {
                        ""
                    },
                    metrics.predicted_latency_ms,
                    metrics.underrun_frames
                )
            },
        )
    }

    fn poll_audio(&mut self, cx: &mut Context<Self>) {
        match self.audio.event() {
            Some(AudioEvent::Staged {
                epoch,
                identity,
                output,
                unavailable,
            }) => {
                if let Some(pending) = &mut self.pending_ready
                    && pending.clock.epoch() == epoch
                    && pending.ready.identity() == &identity
                {
                    pending.audio = output;
                    pending.audio_ready = true;
                    self.audio_note =
                        unavailable.unwrap_or_else(|| "Matching audio prepared".into());
                } else if epoch == self.transport.epoch()
                    && self.preview_state.displayed() == Some(&identity)
                {
                    self.output = output;
                    self.audio_ready_epoch = Some(epoch);
                    self.audio_note = unavailable.unwrap_or_else(|| "Audio output prepared".into());
                }
                cx.notify();
            }
            Some(AudioEvent::Failed { epoch, error }) => {
                if self
                    .pending_ready
                    .as_ref()
                    .is_some_and(|p| p.clock.epoch() == epoch)
                {
                    let pending = self.pending_ready.take().unwrap();
                    self.preview_state.fail(pending.ready.tag(), error.clone());
                    if let Some(preview) = &self.preview {
                        preview.cancel_build();
                    }
                    self.error = Some(if self.displayed.is_some() {
                        format!("Audio preparation failed; prior preview retained: {error}")
                    } else {
                        format!("Audio preparation failed: {error}")
                    });
                    if self.resume_install {
                        self.resume_install = false;
                        self.toggle_playback(cx);
                    }
                } else if epoch == self.transport.epoch() {
                    self.output = None;
                    self.audio_ready_epoch = Some(epoch);
                    self.audio_note = format!("Audio unavailable · {error}");
                }
                cx.notify();
            }
            Some(AudioEvent::Lost { epoch }) if epoch == self.transport.epoch() => {
                self.output = None;
                self.audio_ready_epoch = None;
                let _ = self
                    .transport
                    .fallback(self.clock_start.elapsed().as_secs_f64());
                self.audio.stop(self.transport.epoch());
                self.request_frame(self.transport.position(), cx);
                self.audio_note =
                    "Audio device lost · monotonic fallback · Retry audio to reconnect".into();
            }
            _ => {}
        }
    }

    pub(super) fn activate_audio(&mut self, cx: &mut Context<Self>) {
        let epoch = self.transport.epoch();
        if self.audio_ready_epoch == Some(epoch)
            && self.presented_serial == self.preview_state.serial()
        {
            self.audio_ready_epoch = None;
            if self.output.is_some() {
                self.audio.commit(epoch, self.transport.playing());
            } else {
                let _ = self
                    .transport
                    .fallback(self.clock_start.elapsed().as_secs_f64());
                self.audio.stop(self.transport.epoch());
                self.requested_clock_epoch = self.transport.epoch();
                self.audio_note = "Audio unavailable · monotonic fallback".into();
            }
            cx.notify();
        }
    }

    pub(super) fn step_preview(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.resume_install = false;
        self.pause_playback(cx);
        self.seek_preview(self.transport.position().saturating_add_signed(delta), cx);
    }

    /// Runs when the audio output published a clock snapshot, an audio event arrived or the next
    /// frame boundary passed: takes the output clock, moves the cursor and asks for its frame.
    pub(super) fn pump_playback(&mut self, cx: &mut Context<Self>) {
        if self.closed.load(Ordering::Acquire) {
            self.playback_timer = None;
            return;
        }
        self.poll_audio(cx);
        if let Some(output) = &self.output
            && let Some(snapshot) = output.snapshot()
        {
            self.transport.accept_output(snapshot);
        }
        let was_playing = self.transport.playing();
        let position = self
            .transport
            .tick(self.clock_start.elapsed().as_secs_f64());
        if position != self.preview_state.position() && self.send_frame_request(position, cx) {
            self.notify_player(cx);
        }
        if was_playing != self.transport.playing() {
            if !self.transport.playing() {
                self.audio.stop(self.transport.epoch());
                self.output = None;
                self.audio_ready_epoch = None;
            }
            self.request_thumbnails(cx);
            cx.notify();
        }
        self.forward_playhead();
        self.schedule_playback_timer(cx);
    }

    /// Waits for the next frame boundary while playing; the wait replaces the previous one, and
    /// nothing waits while paused. It covers the host clock, which has no callbacks, and backs
    /// up the output clock should its device stall without reporting a loss.
    pub(super) fn schedule_playback_timer(&mut self, cx: &mut Context<Self>) {
        self.playback_timer = None;
        let Some(wait) = self
            .transport
            .next_frame_in(self.clock_start.elapsed().as_secs_f64())
        else {
            return;
        };
        self.playback_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f64(wait))
                .await;
            let _ = this.update(cx, |shell, cx| shell.pump_playback(cx));
        }));
    }

    /// The preview, transport and history controls: toolbar, stage and transport in one
    /// flat, square pane on the window's fill; its neighbours draw the hairlines between
    /// them. The timeline is placed by `render`.
    pub(super) fn render_player_area(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Presenting, the section shows the presentation stage alone: frames still
        // notify only this section.
        if self.presenting.is_some() {
            return self.render_present_stage(cx);
        }
        self.sync_canvas_toast(cx);
        let entity = cx.entity();
        let mut player = div()
            .bg(self.theme.colors.window)
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.render_player_toolbar(cx))
            .child(self.render_stage(cx))
            .child(self.render_transport(cx))
            .child(super::sections::section_probe(
                super::sections::Section::Player,
                cx,
            ))
            .child(
                // Measures the card so the transport can pick its compact layout.
                canvas(
                    move |bounds, window, cx| {
                        let width = f32::from(bounds.size.width);
                        let relayout = entity.update(cx, |shell, cx| {
                            let relayout = transport_mode(shell.player_width)
                                != transport_mode(width)
                                || show_total_time(shell.player_width) != show_total_time(width);
                            if relayout {
                                cx.notify();
                            }
                            shell.player_width = width;
                            relayout
                        });
                        // A notification during paint does not draw another frame by itself:
                        // without this an idle window keeps the old layout after a drawer
                        // opens until the next input.
                        if relayout {
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        if let Some(menu) = self.render_overflow_menu(cx) {
            player = player.child(menu);
        }
        player.into_any_element()
    }

    fn render_player_toolbar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let palette = theme.colors;
        let (label, tone) = preview_badge(&self.preview_state.status, self.displayed.is_some());
        let audio_label = self.audio_status();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_2))
            .h(px(METRICS.toolbar_h))
            .px(px(METRICS.space_3))
            .border_b(px(METRICS.size.divider))
            .border_color(palette.line)
            .overflow_hidden()
            // The badge keeps one line; the audio note beside it truncates instead.
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(Badge::new(label).tone(tone)),
            )
            .child(with_tooltip(
                div().id("canvas-hint").flex_none().child(icon(
                    Icon::Info,
                    METRICS.icon_md,
                    palette.text_3,
                )),
                &theme,
                Some(CANVAS_HINT.into()),
            ))
            .child(
                text_style(div(), TextStyle::Small)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .text_color(palette.text_2)
                    .child(selectable(audio_label)),
            )
            .child(div().flex_none().child(self.action_button(
                "present",
                "Present",
                self.displayed.is_some(),
                false,
                Some(Icon::Presentation),
                cx,
                |s, cx| s.present(None, cx),
            )))
            .into_any_element()
    }

    /// The stage: the letterboxed frame with its ring, shadow and overlays inside
    /// the `preview-surface` that owns pointer, keyboard and telemetry.
    fn render_stage(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let palette = theme.colors;
        let radius = px(METRICS.radius_frame);
        let mut preview_surface = div()
            .id("preview-surface")
            .relative()
            .track_focus(&self.preview_focus)
            .tab_index(0)
            .role(gpui::Role::Group)
            .aria_label("Compiled preview")
            .focus_visible(|s| s.shadow(focus_ring(&theme)))
            .key_context(PLAYER)
            // Home and End on the preview seek the shown preview itself; from anywhere
            // else they go through the timeline (see the window's handlers).
            .on_action(
                cx.listener(|shell, _: &actions::SeekToStart, _, cx| shell.seek_preview(0, cx)),
            )
            .on_action(cx.listener(|shell, _: &actions::SeekToEnd, _, cx| {
                if let Some(d) = &shell.displayed {
                    shell.seek_preview(d.timeline.total_frames, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, event: &gpui::MouseDownEvent, _, cx| {
                    shell.canvas_pointer_down(event, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|shell, event: &gpui::MouseDownEvent, _, cx| {
                    shell.canvas_pointer_down(event, cx)
                }),
            )
            .on_mouse_move(cx.listener(|shell, event: &gpui::MouseMoveEvent, _, cx| {
                shell.canvas_pointer_move(event, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Left, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Left, cx)),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Middle, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Middle, cx)),
            )
            .on_scroll_wheel(cx.listener(|shell, event: &gpui::ScrollWheelEvent, _, cx| {
                shell.canvas_scroll(event, cx)
            }))
            .flex_1()
            .min_h(px(METRICS.layout.stage_min_h))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2();
        let entity = cx.entity();
        // The surface is the padded stage's content box; its bounds are what the
        // preview telemetry and click mapping report.
        preview_surface = preview_surface.child(
            canvas(
                move |bounds, window, cx| {
                    let dpi = window.scale_factor();
                    let extent = (
                        (f32::from(bounds.size.width) * dpi).round().max(1.) as u32,
                        (f32::from(bounds.size.height) * dpi).round().max(1.) as u32,
                    );
                    entity.update(cx, |shell, cx| {
                        shell.canvas_origin =
                            (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
                        shell.canvas_extent =
                            (f64::from(bounds.size.width), f64::from(bounds.size.height));
                        shell.preview_extent = extent;
                        if let Some(displayed) = &shell.displayed {
                            let before = shell.canvas.viewport.clone();
                            let _ = shell.canvas.resize(
                                f64::from(bounds.size.width),
                                f64::from(bounds.size.height),
                                displayed.timeline.width as u32,
                                displayed.timeline.height as u32,
                            );
                            if before != shell.canvas.viewport {
                                cx.notify();
                            }
                        }
                        if shell.displayed.as_ref().is_some_and(|d| {
                            crate::frame_image::preview_scale(
                                d.timeline.width,
                                d.timeline.height,
                                extent,
                            ) != shell.preview_state.scale()
                        }) {
                            shell.request_frame(shell.transport.position(), cx);
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        if let Some(image) = self.images.current_image() {
            let image = gpui::img(image).rounded(radius);
            preview_surface = if let Some(layout) = self
                .canvas
                .viewport
                .as_ref()
                .map(StageLayout::from_viewport)
            {
                let frame = layout.frame_rect;
                let placed = |element: gpui::Div, rect: studio_engine::CanvasRect| {
                    element
                        .absolute()
                        .left(px(rect.x as f32))
                        .top(px(rect.y as f32))
                        .w(px(rect.width as f32))
                        .h(px(rect.height as f32))
                };
                // Overlays live inside the frame, so they clip to it and use frame
                // coordinates.
                let mut content = placed(div(), frame)
                    .overflow_hidden()
                    .child(image.size_full());
                if let Some(bounds) = self.canvas.selected_bounds() {
                    content = content.child(
                        placed(div(), layout.to_frame_space(bounds))
                            .border_2()
                            .border_color(palette.accent)
                            .bg(theme.colors.selection),
                    );
                }
                if let (Some(viewport), Some(rect)) =
                    (&self.canvas.viewport, &self.canvas.rectangle_scope)
                {
                    content = content.child(
                        placed(
                            div(),
                            layout.to_frame_space(viewport.video_scope_bounds(rect)),
                        )
                        .border_2()
                        .border_color(palette.scope),
                    );
                }
                preview_surface
                    .child(
                        placed(div(), frame)
                            .rounded(radius)
                            .shadow(shadow_for(theme.appearance, Shadow::Preview).into_vec()),
                    )
                    .child(content)
                    .child(
                        placed(div(), frame)
                            .rounded(radius)
                            .border_1()
                            .border_color(palette.line),
                    )
            } else {
                preview_surface.child(image.size_full())
            };
        } else {
            preview_surface = preview_surface.child(
                EmptyState::new(
                    &theme,
                    if self.displayed.is_some() {
                        "Empty video"
                    } else {
                        "No preview available"
                    },
                )
                .body("Build preview to compile the project's current source.")
                .centered(),
            );
        }
        let mut stage = div()
            .id("stage")
            .relative()
            .flex_1()
            .min_h(px(METRICS.layout.stage_min_h))
            .flex()
            .flex_col()
            .overflow_hidden()
            .p(px(METRICS.space_4))
            .bg(palette.stage)
            .child(preview_surface);
        if let Some(error) = self.canvas.error.clone() {
            let entity = cx.entity();
            stage = stage.child(
                div()
                    .absolute()
                    .left(px(METRICS.space_4))
                    .right(px(METRICS.space_4))
                    .bottom(px(METRICS.space_4))
                    .child(Notice::new("canvas-error", Tone::Danger, error).on_dismiss(
                        move |_, cx| {
                            entity.update(cx, |shell, cx| {
                                shell.canvas.error = None;
                                shell.notify_player(cx);
                            });
                        },
                    )),
            );
        } else if let Some(toast) = self.canvas_toast.borrow().host.current() {
            let entity = cx.entity();
            stage = stage.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(METRICS.space_4))
                    .flex()
                    .justify_center()
                    .child(
                        Toast::new(&theme, "canvas-message", toast.message.clone()).on_hover(
                            move |hovered, _, cx| {
                                entity.update(cx, |shell, cx| {
                                    shell
                                        .canvas_toast
                                        .borrow_mut()
                                        .host
                                        .set_hovered(*hovered, Instant::now());
                                    shell.schedule_canvas_toast(cx);
                                });
                            },
                        ),
                    ),
            );
        }
        stage.into_any_element()
    }

    /// Shows the canvas message as a toast when it changes.
    fn sync_canvas_toast(&self, cx: &mut Context<Self>) {
        let message = self.canvas.message.clone();
        {
            let mut toast = self.canvas_toast.borrow_mut();
            if toast.seen == message {
                return;
            }
            toast.seen = message.clone();
            match message {
                Some(message) => {
                    toast.host.show(message, Instant::now());
                }
                None => return,
            }
        }
        self.schedule_canvas_toast(cx);
    }

    /// Removes the canvas toast when it expires; nothing while it is hovered.
    fn schedule_canvas_toast(&self, cx: &mut Context<Self>) {
        let pending = {
            let toast = self.canvas_toast.borrow();
            toast
                .host
                .current()
                .map(|current| current.id)
                .zip(toast.host.remaining(Instant::now()))
        };
        let Some((id, wait)) = pending else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |shell, cx| {
                let expired = {
                    let mut toast = shell.canvas_toast.borrow_mut();
                    // A newer toast runs its own timer.
                    if toast.host.current().map(|current| current.id) != Some(id) {
                        return;
                    }
                    toast.host.tick(Instant::now())
                };
                if expired {
                    shell.notify_player(cx);
                } else {
                    shell.schedule_canvas_toast(cx);
                }
            });
        })
        .detach();
    }
}

const CANVAS_HINT: &str = "Select: click · Rectangle: Shift-drag · Cycle: Alt-click/C · Zoom: Ctrl-wheel · Pan: middle-drag · Fit: 0";

/// The player toolbar's preview badge. "Prior preview retained" only when a preview is on
/// screen to retain.
fn preview_badge(status: &studio_engine::PreviewStatus, retained: bool) -> (&'static str, Tone) {
    match status {
        studio_engine::PreviewStatus::Building if retained => {
            ("Building/preparing · prior preview retained", Tone::Info)
        }
        studio_engine::PreviewStatus::Building => ("Building preview", Tone::Info),
        studio_engine::PreviewStatus::Preparing => {
            ("Preparing matching frame and audio", Tone::Info)
        }
        studio_engine::PreviewStatus::Error(_) if retained => {
            ("Preview failed · prior preview retained", Tone::Warning)
        }
        studio_engine::PreviewStatus::Error(_) => ("Preview failed", Tone::Warning),
        _ => ("CPU preview · shaders unsupported", Tone::Neutral),
    }
}

#[cfg(test)]
mod badge_tests {
    use studio_engine::PreviewStatus;

    use super::preview_badge;

    #[test]
    fn the_badge_mentions_a_retained_preview_only_when_one_is_displayed() {
        let building = PreviewStatus::Building;
        let failed = PreviewStatus::Error("boom".into());
        assert_eq!(preview_badge(&building, false).0, "Building preview");
        assert_eq!(preview_badge(&failed, false).0, "Preview failed");
        assert!(
            preview_badge(&building, true)
                .0
                .contains("prior preview retained")
        );
        assert!(
            preview_badge(&failed, true)
                .0
                .contains("prior preview retained")
        );
    }
}
