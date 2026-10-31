//! What the shell reports to hosts that observe it: telemetry for the test harnesses and
//! the bounds of the controls they click.

use std::path::PathBuf;
use std::time::Duration;

use gpui::Context;
use studio_engine::containment::OwnershipPolicy;
use studio_engine::engine::backend::Command;
use studio_engine::services::audio_service::OutputHandle;
use studio_engine::services::export_service::ExportProgress;

use super::StudioShell;
use crate::probe::BoundsProbe;

impl StudioShell {
    /// Reports the bounds of the measured buttons in the telemetry without writing a
    /// telemetry file, for test harnesses that read [`Self::telemetry`] directly.
    pub fn report_button_bounds(&mut self, cx: &mut Context<Self>) {
        self.qualifying = true;
        // The cached sections render again with their bounds probes.
        cx.notify();
    }

    /// Whether the agent prompt, or a question's text field, holds the keyboard focus.
    pub fn prompt_focused(&self, window: &gpui::Window, cx: &gpui::App) -> bool {
        self.panel.read(cx).composer_has_focus(window, cx)
    }

    /// The studio's observation entry: opens `project` and writes the agent panel's redacted state
    /// (phase, counts, bounds; never prompts or message text) to `output` every 250 ms.
    /// Nothing is launched and no command is sent; an operator drives the real panel.
    ///
    /// `test_ownership` is the explicit test-only injection for fixtures: with it the
    /// adapter's writers count as that ownership WITHOUT the containment check of the
    /// launch (the panel says so); without it only the containment verified at launch
    /// counts, exactly as in the product.
    pub fn start_agent_telemetry(
        &mut self,
        project: PathBuf,
        output: PathBuf,
        test_ownership: Option<studio_bootstrap::WriterOwnership>,
        cx: &mut Context<Self>,
    ) {
        if let Some(ownership) = test_ownership {
            self.agent.ownership = OwnershipPolicy::TestInjected(ownership);
        }
        // Observation only: measured bounds of the named transport buttons go to the
        // telemetry so a driver clicks what the shell actually laid out.
        self.qualifying = true;
        // The cached sections render again with their bounds probes.
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut opened = false;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let Ok(value) = this.update(cx, |shell, cx| {
                    if !opened && !shell.busy {
                        shell.dispatch(Command::Open(project.clone()), cx);
                        opened = true;
                    }
                    serde_json::json!({
                        "project_open": shell.presentation.project.is_some(),
                        "sdk_ready_for_agent": shell.presentation.agent_sdk.is_some(),
                        "agent_workflow_open": shell.agent.workflow.is_some(),
                        "awaiting_preview": shell.awaiting_preview.is_some(),
                        "displayed_revision": shell
                            .displayed
                            .as_ref()
                            .map(|d| d.identity().source_revision.chars().take(12).collect::<String>()),
                        "displayed_identity": shell.displayed.as_ref().map(|d| serde_json::json!({
                            "open_session_len": d.identity().open_session.len(),
                            "worker_generation": d.identity().worker_generation,
                            "timeline_open_session_len": d.timeline.envelope.identity.open_session.len(),
                            "timeline_worker_generation": d.timeline.envelope.identity.worker_generation,
                        })),
                        "panel": shell.panel.read(cx).telemetry(cx),
                        "preview": shell.preview_telemetry(),
                        "canvas": shell.canvas_telemetry(),
                        "buttons": serde_json::to_value(&shell.button_bounds)
                            .unwrap_or(serde_json::Value::Null),
                        "sections": serde_json::json!(shell.section_bounds),
                        "ruler_bounds": shell.timeline.read(cx).qualification_metrics()["ruler_bounds"].clone(),
                        "timeline_selection": shell.timeline.read(cx).qualification_metrics()["selection"].clone(),
                    })
                }) else {
                    break;
                };
                let path = output.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let temporary = path.with_extension("tmp");
                        std::fs::write(
                            &temporary,
                            serde_json::to_vec(&value).map_err(std::io::Error::other)?,
                        )?;
                        std::fs::rename(temporary, path)
                    })
                    .await;
                if let Err(error) = result {
                    let _ = this.update(cx, |shell, cx| {
                        shell.error = Some(format!("Agent telemetry: {error}"));
                        cx.notify();
                    });
                    break;
                }
            }
        })
        .detach();
    }

    /// Redacted playback identity for the observation entry: revisions are 12-hex prefixes,
    /// the audio digest is the prepared mix's hash prefix; nothing here is project text.
    fn preview_telemetry(&self) -> serde_json::Value {
        let prefix = |text: &str| text.chars().take(12).collect::<String>();
        serde_json::json!({
            "status": match &self.preview_state.status {
                studio_engine::PreviewStatus::Absent => "absent",
                studio_engine::PreviewStatus::Building => "building",
                studio_engine::PreviewStatus::Preparing => "preparing",
                studio_engine::PreviewStatus::Ready => "ready",
                studio_engine::PreviewStatus::Displayed => "displayed",
                studio_engine::PreviewStatus::Error(_) => "error",
                studio_engine::PreviewStatus::Closed => "closed",
            },
            "error": match &self.preview_state.status {
                studio_engine::PreviewStatus::Error(e) => Some(format!("{e:?}").chars().take(4000).collect::<String>()),
                _ => None,
            },
            "transport_epoch": self.transport.epoch(),
            "playing": self.transport.playing(),
            "seek_serial": self.preview_state.serial(),
            "position": self.preview_state.position(),
            "audio_output": self.output.is_some(),
            "painted_frame": self.painted_frame,
            "presented_serial": self.presented_serial,
            "displayed": self.displayed.as_ref().map(|d| serde_json::json!({
                "video_revision": prefix(&d.identity().source_revision),
                "audio_revision": prefix(&d.audio.envelope.identity.source_revision),
                "audio_sha256": prefix(&d.audio.sha256),
                "audio_samples": d.audio.sample_count,
                "sample_rate": d.audio.sample_rate,
                "fps": d.timeline.fps,
                "pcm_start_sample": d.pcm_start_sample,
                "pcm_samples": d.pcm.len() / 8,
                "frame_index": d.frame.as_ref().map(|f| f.response.frame_index),
                "frame_serial": d.frame.as_ref().map(|f| f.response.seek_serial),
                "total_frames": d.timeline.total_frames,
                "position": d.position,
                "seek_serial": d.seek_serial,
            })),
        })
    }

    /// Bounded native-selection observations for the opt-in telemetry.
    /// Only semantic keys and geometry are recorded; authored source and image bytes are not.
    fn canvas_telemetry(&self) -> serde_json::Value {
        let metadata = self
            .canvas
            .displayed
            .as_ref()
            .and_then(|displayed| displayed.metadata.as_ref());
        let objects: Vec<_> = metadata
            .into_iter()
            .flat_map(|metadata| metadata.objects.iter())
            .take(64)
            .map(|object| {
                serde_json::json!({
                    "identity": object.identity,
                    "bounds": object.bounds,
                    "paint_order": object.paint_order,
                    "support": format!("{:?}", object.support),
                })
            })
            .collect();
        let selection = self.canvas.selection.as_ref().map(|selection| {
            serde_json::json!({
                "identity": selection.identity,
                "bounds": selection.bounds,
                "support": format!("{:?}", selection.support),
            })
        });
        let viewport = self.canvas.viewport.as_ref();
        let image_bounds = viewport.map(|viewport| {
            let bounds = viewport.image_bounds();
            serde_json::json!({
                "x": bounds.x,
                "y": bounds.y,
                "width": bounds.width,
                "height": bounds.height,
            })
        });
        let rectangle = self.canvas.rectangle_scope.map(|rectangle| {
            serde_json::json!({
                "x": rectangle.x,
                "y": rectangle.y,
                "width": rectangle.width,
                "height": rectangle.height,
            })
        });
        let rendered_image = self.images.current_image();
        let rendered_image_size = rendered_image.as_ref().map(|image| image.size(0));
        let rendered_bright_pixels = rendered_image
            .as_ref()
            .and_then(|image| image.as_bytes(0))
            .map(|pixels| {
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|pixel| pixel[..3].iter().all(|channel| *channel > 180) && pixel[3] > 0)
                    .count()
            });
        serde_json::json!({
            "frame_index": self.canvas.displayed.as_ref().map(|frame| frame.frame_index),
            "seek_serial": self.canvas.displayed.as_ref().map(|frame| frame.seek_serial),
            "metadata_status": metadata.map(|metadata| format!("{:?}", metadata.status)),
            "object_count": metadata.map_or(0, |metadata| metadata.objects.len()),
            "objects_truncated": metadata.is_some_and(|metadata| metadata.objects.len() > 64),
            "objects": objects,
            "selection": selection,
            "rectangle": rectangle,
            "drawing_rectangle": self.canvas.is_drawing_rectangle(),
            "video_dimensions": viewport.map(|viewport| viewport.video_dimensions()),
            "image_bounds": image_bounds,
            "canvas_origin": self.canvas_origin,
            "canvas_extent": self.canvas_extent,
            "rendered_image_size": rendered_image_size.map(|size| (size.width.0, size.height.0)),
            "rendered_bright_pixels": rendered_bright_pixels,
            "message": self.canvas.message,
        })
    }

    /// The shell's redacted state for hosts that observe the app (test harnesses): the
    /// snapshot plus the preview, canvas, timeline, agent
    /// panel and job state. Revisions are prefixes; no project text or image bytes.
    pub fn telemetry(&self, cx: &gpui::App) -> serde_json::Value {
        let mut value = self.snapshot(cx);
        value["project_open"] = self.presentation.project.is_some().into();
        if let Some(project) = &self.presentation.project {
            value["accepted_revision"] = project.accepted.as_str().into();
            value["checkpoint_path"] = project.checkpoint.to_string_lossy().into_owned().into();
            value["job"] = project.job.clone().into();
        }
        value["app_version"] = self.app_version.into();
        value["sdk"] = self.presentation.sdk.clone().into();
        value["sdk_ready_for_agent"] = self.presentation.agent_sdk.is_some().into();
        value["awaiting_preview"] = self.awaiting_preview.is_some().into();
        value["preview"] = self.preview_telemetry();
        value["canvas"] = self.canvas_telemetry();
        value["timeline"] = self.timeline.read(cx).qualification_metrics();
        let split = self.split();
        value["split"] = serde_json::json!({
            "available": self.editor_height - studio_theme::METRICS.gutter,
            "timeline_height": split.timeline_height,
            "player_min_height": split.player_min_height,
            "grip_bounds": self.grip_bounds,
            "resizing": self.resize.is_some(),
        });
        value["agent_column"] = serde_json::json!({
            "width": self.sidebar_column_width(),
            "grip_bounds": self.agent_grip_bounds,
        });
        // The selected sidebar tab.
        value["drawer"] = self.sidebar_tab.name().into();
        value["viewport"] = serde_json::json!(self.viewport);
        value["settings_open"] = self.settings_open.into();
        value["settings_page"] = format!("{:?}", self.settings_page).into();
        if let Some(review) = self.review_telemetry(cx) {
            value["review"] = review;
        }
        value["header_appearance"] = match self.header_appearance.get() {
            Some(studio_theme::Appearance::Light) => "light".into(),
            Some(studio_theme::Appearance::Dark) => "dark".into(),
            None => serde_json::Value::Null,
        };
        value["sections"] = serde_json::json!(self.section_bounds);
        // How often each section view rendered; a new frame should render only the player.
        value["renders"] = self
            .sections
            .iter()
            .flat_map(|sections| sections.all())
            .map(|view| {
                let view = view.read(cx);
                (
                    view.section.name().to_owned(),
                    serde_json::Value::from(view.renders),
                )
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        value["renders"]["timeline"] = self.timeline.read(cx).renders().into();
        value["panel"] = self.panel.read(cx).telemetry(cx);
        value["export"] = export_telemetry(self.export_status.as_ref());
        value["export"]["panel_open"] = self.export_panel.open.into();
        value["create_template"] = self
            .create_choice
            .map(|template| format!("{template:?}"))
            .into();
        value["export"]["support"] = self.export_panel.support.name().into();
        value["export"]["settings"] =
            serde_json::to_value(self.export_settings()).unwrap_or_default();
        value["presets"] = self.preset_panel.read(cx).telemetry();
        value["props"] = self.props_panel.read(cx).telemetry();
        value["output_devices"] = self.audio.devices().into();
        value
    }

    fn snapshot(&self, cx: &gpui::App) -> serde_json::Value {
        let thumbnails = self.timeline.read(cx).qualification_metrics();
        let buttons = serde_json::to_value(&self.button_bounds).expect("finite geometry");
        serde_json::json!({
            "pid": std::process::id(), "busy": self.busy,
            "onboarding": self.onboarding.as_ref().map(|o| o.step.name()),
            "permission_policy": self.permission_policy,
            "project_path": self.presentation.project.as_ref().map(|p| &p.root), "error": self.error,
            "current_source_revision": self.presentation.project.as_ref().map(|p| p.source.as_str()),
            "preview_status": format!("{:?}", self.preview_state.status),
            "displayed_generation": self.displayed.as_ref().map(|d| d.identity().worker_generation),
            "displayed_revision": self.displayed.as_ref().map(|d| &d.identity().source_revision),
            "position": self.transport.position(), "playing": self.transport.playing() || self.resume_install,
            "total_frames": self.displayed.as_ref().map(|d| d.timeline.total_frames),
            "fps": self.displayed.as_ref().map(|d| d.timeline.fps),
            "clock_epoch": self.transport.epoch(), "seek_serial": self.preview_state.serial(),
            "paint_serial": self.presented_serial, "paint_frame": self.painted_frame,
            "preview_extent": self.preview_extent, "preview_scale": self.preview_state.scale(),
            "audio_ready": self.output.is_some(),
            "audio_identity_matches": self.output.as_ref().is_none_or(|o| Some(&o.identity) == self.preview_state.displayed() && o.epoch == self.transport.epoch()),
            "audio_note": self.audio_note, "output_device": format!("{:?}", self.output_device), "muted": self.muted,
            "audio_metrics": self.output.as_ref().map(OutputHandle::metrics),
            "thumbnail_metrics": thumbnails,
            "image_metrics": { "resident": self.images.resident_count(), "queued": self.images.queue_depth(), "dropped": self.images.dropped_count(), "release_failures": self.images.release_failures(), "out_of_order": self.images.out_of_order() },
            "pump_metrics": self.preview.as_ref().map(|p| p.metrics()), "buttons": buttons,
            "ruler_bounds": thumbnails["ruler_bounds"],
        })
    }

    /// Bounds reporter for the buttons a harness clicks; `None` unless bounds are reported,
    /// and for every other button.
    pub(super) fn bounds_probe(
        &self,
        id: &gpui::ElementId,
        cx: &mut Context<Self>,
    ) -> Option<BoundsProbe> {
        let name = match id {
            gpui::ElementId::Name(name)
                if self.qualifying
                    && matches!(
                        name.as_ref(),
                        "build-preview"
                            | "cancel-preview"
                            | "play-pause"
                            | "mute-audio"
                            | "next-audio-output"
                            | "retry-audio-output"
                            | "no-audio-output"
                            | "previous-frame"
                            | "next-frame"
                            | "close"
                            | "create"
                            | "settings"
                            | "settings-back"
                            | "onboarding-skip"
                            | "onboarding-back"
                            | "onboarding-next"
                            | "run-setup-again"
                            | "open-presets"
                            | "install"
                            | "export"
                            | "timeline-undo"
                            | "timeline-redo"
                            | "timeline-split"
                            | "timeline-cycle"
                            | "timeline-zoom-in"
                            | "timeline-zoom-out"
                            | "timeline-fit"
                    ) =>
            {
                Some(name.to_string())
            }
            _ => None,
        };
        let entity = cx.entity();
        name.map(|name| {
            BoundsProbe::new(move |bounds, cx| {
                entity.update(cx, |shell, _| {
                    shell.button_bounds.insert(name.clone(), bounds);
                });
            })
        })
    }
}

/// The export's state and its detail (a failure's reason, the published file's name).
fn export_telemetry(status: Option<&ExportProgress>) -> serde_json::Value {
    let (state, detail) = match status {
        None => ("idle", None),
        Some(ExportProgress::Started { .. }) => ("started", None),
        Some(ExportProgress::Rendering { done, total }) => {
            ("rendering", Some(format!("{done}/{total}")))
        }
        Some(ExportProgress::Audio) => ("audio", None),
        Some(ExportProgress::Warning(warning)) => ("warning", Some(warning.clone())),
        Some(ExportProgress::Verifying) => ("verifying", None),
        Some(ExportProgress::Posters { done, total }) => {
            ("posters", Some(format!("{done}/{total}")))
        }
        Some(ExportProgress::WritingDeck) => ("writing_deck", None),
        Some(ExportProgress::Complete { destination, .. }) => (
            "complete",
            destination
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        ),
        Some(ExportProgress::Failed(reason)) => ("failed", Some(reason.clone())),
        Some(ExportProgress::Cancelled) => ("cancelled", None),
    };
    serde_json::json!({ "state": state, "detail": detail })
}
