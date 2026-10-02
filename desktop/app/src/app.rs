use crate::frame_image::{ImagePresentationManager, create_render_image, generate_reference_frame};
use crate::presentation_stress::PresentationStress;
use crate::preview_element::PreviewElement;
use crate::selection_spike::{DisplayedSourceFrame, SelectedElementInfo, SelectionSpike};
use crate::setup_view::SetupView;
use crate::text_input::TextInput;
use crate::worker_client::WorkerClient;
use gpui::{
    AppContext, Bounds, Context, Entity, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, RenderImage, StatefulInteractiveElement, Styled, Window, div,
    green, px, rgb, white,
};

use std::time::{Duration, Instant};
use studio_bootstrap::ProcessTreeManager;
pub struct StudioSpikeApp {
    pub agent_spike: Entity<crate::agent_spike::AgentSpike>,
    pub worker_generation: u64,
    pub worker_busy: bool,
    pub worker_epoch: u64,
    pub text_input: Entity<TextInput>,
    pub setup_view: Entity<SetupView>,
    pub image_manager: ImagePresentationManager,
    pub frame_index: usize,
    pub dimensions: (u32, u32),
    pub last_conversion_duration: Duration,
    pub replacement_count: usize,
    pub process_tree: ProcessTreeManager,
    pub status_message: String,
    pub worker_client: Option<WorkerClient>,
    pub worker_status: String,
    pub pending_stress_image: Option<std::sync::Arc<RenderImage>>,
    pub stress: PresentationStress,
    pub stress_completion: Option<(u64, usize)>,
    pub stress_painted: Option<(u64, usize)>,
    pub preview_bounds: Option<Bounds<Pixels>>,
    pub displayed_source: Option<DisplayedSourceFrame>,
    pub pending_source: Option<DisplayedSourceFrame>,
    pub selected_source: Option<SelectedElementInfo>,
    pub worker_project_root: Option<std::path::PathBuf>,
    pub stress_test_start: Option<Instant>,
    pub stress_metrics: crate::stress_metrics::StressMetrics,
    pub qualification_output: Option<std::path::PathBuf>,
    pub displayed_setup_frame: Option<std::path::PathBuf>,
}

impl StudioSpikeApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let text_input = cx.new(TextInput::new);
        let setup_view = cx.new(SetupView::new);
        let process_tree = ProcessTreeManager::new();
        cx.on_app_quit(|app, cx| {
            let manager = app.process_tree.clone();
            let setup = app.setup_view.read(cx).process_tree.clone();
            cx.background_executor().spawn(async move {
                manager.terminate_all(Duration::from_millis(300));
                setup.terminate_all(Duration::from_millis(300));
            })
        })
        .detach();

        let agent_project = setup_view
            .read(cx)
            .projects_root
            .join("phase-zero-annotated-video");
        let agent_sdk = setup_view.read(cx).sdk_home.join("active");
        let agent_manifest = setup_view.read(cx).manifest.clone();
        let agent_spike = cx.new(|agent_cx| {
            crate::agent_spike::AgentSpike::new(
                agent_project,
                agent_sdk,
                agent_manifest,
                ProcessTreeManager::new(),
                agent_cx,
            )
        });
        cx.subscribe(
            &agent_spike,
            |app, panel, _: &crate::agent_spike::CandidateReady, cx| {
                let candidate = panel.update(cx, |panel, _| panel.candidate.take());
                if let Some(candidate) = candidate {
                    app.stress.stop();
                    app.stress_completion = None;
                    app.stress_painted = None;
                    if let Some(mut old) = app.worker_client.take() {
                        let _ = old.force_crash();
                    }
                    app.worker_epoch += 1;
                    app.worker_busy = false;
                    app.worker_generation = candidate.worker.generation();
                    app.worker_project_root = Some(candidate.source.project_root.clone());
                    app.dimensions = (
                        candidate.source.header.width,
                        candidate.source.header.height,
                    );
                    app.displayed_source = None;
                    app.pending_source = Some(candidate.source);
                    app.selected_source = None;
                    app.worker_client = Some(candidate.worker);
                    app.pending_stress_image = Some(candidate.image);
                    app.status_message = "Agent candidate ready for source inspection".into();
                    cx.notify();
                }
            },
        )
        .detach();
        Self {
            agent_spike,
            worker_generation: 0,
            worker_busy: false,
            worker_epoch: 0,
            text_input,
            setup_view,
            image_manager: ImagePresentationManager::new(),
            frame_index: 0,
            dimensions: (640, 360),
            last_conversion_duration: Duration::ZERO,
            replacement_count: 0,
            process_tree,
            status_message: "Ready".to_string(),
            worker_client: None,
            worker_status: "Disconnected".to_string(),
            pending_stress_image: None,
            stress: PresentationStress::default(),
            stress_completion: None,
            stress_painted: None,
            preview_bounds: None,
            displayed_source: None,
            pending_source: None,
            selected_source: None,
            worker_project_root: None,
            stress_test_start: None,
            stress_metrics: Default::default(),
            qualification_output: None,
            displayed_setup_frame: None,
        }
    }

    pub fn launch_real_worker(&mut self, cx: &mut Context<Self>) {
        if self.worker_busy || self.stress.active || self.agent_spike.read(cx).active {
            return;
        }
        if let Some(mut old) = self.worker_client.take() {
            let _ = old.force_crash();
        }
        self.worker_epoch += 1;
        let epoch = self.worker_epoch;
        self.worker_generation = crate::worker_client::allocate_worker_generation();
        let generation = self.worker_generation;
        let sdk = self.setup_view.read(cx).sdk_home.join("active");
        let root = self.worker_project_root.clone().unwrap_or_else(|| {
            self.setup_view
                .read(cx)
                .projects_root
                .join("phase-zero-annotated-video")
        });
        self.worker_project_root = Some(root.clone());
        let manifest = self.setup_view.read(cx).manifest.clone();
        let manager = self.process_tree.clone();
        self.worker_busy = true;
        self.worker_status = "Building/launching annotated worker...".into();
        let task = cx.background_executor().spawn(async move {
            crate::worker_project::create_worker_project(&root, &sdk)?;
            crate::worker_project::launch_worker(&root, &sdk, manifest, generation, &manager)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |app, cx| {
                if app.worker_epoch != epoch {
                    return;
                }
                app.worker_busy = false;
                match result {
                    Ok(worker) => {
                        app.worker_status = format!("Connected generation {}", worker.generation());
                        app.worker_client = Some(worker);
                    }
                    Err(error) => app.worker_status = error,
                }
                app.status_message = app.worker_status.clone();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn step_real_worker_frame(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.stress.active || self.agent_spike.read(cx).active {
            return;
        }
        let epoch = self.worker_epoch;
        let Some(mut worker) = self.worker_client.take() else {
            self.status_message = "Worker unavailable or busy".into();
            cx.notify();
            return;
        };
        self.worker_busy = true;
        let project_root = self.worker_project_root.clone();
        let target = self.frame_index % 150;
        let task = cx.background_executor().spawn(async move {
            let start = Instant::now();
            let result = (|| {
                worker
                    .request_render_frame(target)
                    .map_err(|e| e.to_string())?;
                let header = worker.latest_header().cloned().ok_or("No frame header")?;
                let image =
                    create_render_image(&header, worker.latest_pixels().ok_or("No frame pixels")?)
                        .map_err(|e| e.to_string())?;
                let elements = worker
                    .request_elements(&header)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>((image, header, elements, start.elapsed()))
            })();
            (worker, result)
        });
        cx.spawn(async move |this, cx| {
            let (worker, result) = task.await;
            let _ = this.update(cx, |app, cx| {
                if app.worker_epoch != epoch {
                    return;
                }
                app.worker_busy = false;
                app.worker_client = Some(worker);
                match result {
                    Ok((image, header, elements, duration)) => {
                        app.dimensions = (header.width, header.height);
                        app.frame_index = target + 1;
                        app.pending_stress_image = Some(image);
                        app.last_conversion_duration = duration;
                        app.selected_source = None;
                        app.displayed_source = None;
                        app.pending_source =
                            project_root.map(|project_root| DisplayedSourceFrame {
                                header,
                                elements,
                                project_root,
                            });
                        app.status_message =
                            format!("Worker frame {target}; click the title to inspect its source");
                    }
                    Err(error) => app.status_message = format!("Worker frame failed: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn force_crash_worker(&mut self, cx: &mut Context<Self>) {
        if let Some(worker) = self.worker_client.as_mut() {
            let _ = worker.force_crash();
            self.worker_status = "CRASHED (killed forcefully)".into();
            self.status_message =
                "Worker killed! Host UI remains completely alive and responsive.".into();
            cx.notify();
        } else if self.worker_busy {
            self.process_tree.terminate_all(Duration::from_millis(100));
            self.status_message = "Stopping the active worker operation...".into();
            cx.notify();
        }
    }

    pub fn restart_worker(&mut self, cx: &mut Context<Self>) {
        self.launch_real_worker(cx);
    }

    pub fn start_stress_test(&mut self, cx: &mut Context<Self>) {
        if self.stress.active || self.worker_busy || self.agent_spike.read(cx).active {
            return;
        }
        if self.worker_client.is_none() {
            self.status_message =
                "Connect a real worker before starting presentation stress".into();
            cx.notify();
            return;
        }
        self.stress.start();
        self.stress_metrics = crate::stress_metrics::StressMetrics {
            release_baseline: self.image_manager.dropped_count(),
            host_rss_baseline_bytes: crate::stress_metrics::resident_bytes(std::process::id()),
            last_heartbeat: Some(Instant::now()),
            last_completion: Some(Instant::now()),
            ..Default::default()
        };
        let run = self.stress.run;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let active = this
                    .update(cx, |app, cx| {
                        if !app.stress.active || app.stress.run != run {
                            return false;
                        }
                        app.stress_metrics.heartbeat();
                        if app
                            .stress_metrics
                            .last_completion
                            .is_some_and(|last| last.elapsed() > Duration::from_secs(45))
                            || app
                                .stress_test_start
                                .is_some_and(|start| start.elapsed() > Duration::from_secs(600))
                        {
                            app.abort_stress(
                                "Presentation stress exceeded its completion deadline".into(),
                                cx,
                            );
                            return false;
                        }
                        true
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
        self.stress_test_start = Some(Instant::now());
        self.status_message = "Seeking and presenting 1,000 real worker images...".into();
        self.displayed_source = None;
        self.pending_source = None;
        self.selected_source = None;
        self.schedule_next_stress_frame(cx);
    }

    fn schedule_next_stress_frame(&mut self, cx: &mut Context<Self>) {
        let Some(token) = self.stress.admit() else {
            return;
        };
        let Some(mut worker) = self.worker_client.take() else {
            self.abort_stress("Worker unavailable".into(), cx);
            return;
        };
        self.worker_busy = true;
        let epoch = self.worker_epoch;
        let project = self.worker_project_root.clone();
        self.stress_metrics.queue_high_water = self.stress_metrics.queue_high_water.max(1);
        let task = cx.background_executor().spawn(async move {
            let result = (|| -> Result<_, String> {
                let render_start = Instant::now();
                let target = (token.1 * 37) % 150;
                let first = worker
                    .request_seek(target)
                    .ok_or("Seek already in flight")?;
                // Exercise actual latest-wins dispatch over two completed requests.
                worker.request_seek((target + 11) % 150);
                worker.request_seek((target + 23) % 150);
                let next = worker
                    .request_render_frame(first)
                    .map_err(|e| e.to_string())?
                    .ok_or("Pending seek missing")?;
                if worker
                    .request_render_frame(next)
                    .map_err(|e| e.to_string())?
                    .is_some()
                {
                    return Err("Unexpected pending seek".into());
                }
                let header = worker
                    .latest_header()
                    .cloned()
                    .ok_or("Frame header missing")?;
                let elements = worker
                    .request_elements(&header)
                    .map_err(|e| e.to_string())?;
                let render_ms = render_start.elapsed().as_secs_f64() * 1_000.;
                let conversion_start = Instant::now();
                let image = create_render_image(
                    &header,
                    worker.latest_pixels().ok_or("Frame pixels missing")?,
                )
                .map_err(|e| e.to_string())?;
                let conversion_ms = conversion_start.elapsed().as_secs_f64() * 1_000.;
                let host_rss = crate::stress_metrics::resident_bytes(std::process::id());
                let worker_rss = worker
                    .process_id()
                    .and_then(crate::stress_metrics::resident_bytes);
                Ok((
                    image,
                    header,
                    elements,
                    render_ms,
                    conversion_ms,
                    host_rss,
                    worker_rss,
                ))
            })();
            (worker, result)
        });
        cx.spawn(async move |this, cx| {
            let (worker, result) = task.await;
            let _ = this.update(cx, |app, cx| {
                if app.worker_epoch != epoch || !app.stress.active || app.stress.run != token.0 {
                    return;
                }
                app.worker_busy = false;
                app.worker_client = Some(worker);
                match result {
                    Ok((
                        image,
                        header,
                        elements,
                        render_ms,
                        conversion_ms,
                        host_rss,
                        worker_rss,
                    )) => {
                        app.dimensions = (header.width, header.height);
                        app.pending_stress_image = Some(image);
                        app.stress_completion = Some(token);
                        app.pending_source = project.map(|project_root| DisplayedSourceFrame {
                            header,
                            elements,
                            project_root,
                        });
                        app.stress_metrics.verified_render_requests += 2;
                        app.stress_metrics.coalesced_seeks += 2;
                        app.stress_metrics.render_transfer_ms += render_ms;
                        app.stress_metrics.conversion_ms += conversion_ms;
                        app.stress_metrics.sample_memory(host_rss, worker_rss);
                    }
                    Err(error) => app.abort_stress(format!("Stress worker failed: {error}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn abort_stress(&mut self, error: String, cx: &mut Context<Self>) {
        self.stress.stop();
        self.stress_completion = None;
        self.stress_painted = None;
        self.pending_stress_image = None;
        self.pending_source = None;
        self.worker_epoch += 1;
        self.worker_busy = false;
        let worker = self.worker_client.take();
        // Cleanup owns the old domain; a reconnect cannot be killed by this task.
        let manager = std::mem::replace(&mut self.process_tree, ProcessTreeManager::new());
        cx.background_executor()
            .spawn(async move {
                manager.terminate_all(Duration::from_millis(300));
                drop(worker);
            })
            .detach();
        self.stress_metrics.failure = Some(error.clone());
        self.status_message = error;
        self.persist_stress(cx);
    }

    fn persist_stress(&mut self, cx: &mut Context<Self>) {
        self.stress_metrics.native_input_observed = self.text_input.read(cx).content().to_owned();
        self.stress_metrics.selected_source = self.selected_source.clone();
        self.stress_metrics.confirmed_presentations = self.stress.confirmed;
        self.stress_metrics.completed = self.stress.confirmed == PresentationStress::TARGET
            && self.stress_metrics.failure.is_none();
        self.stress_metrics.elapsed_ms = self
            .stress_test_start
            .map(|start| start.elapsed().as_secs_f64() * 1_000.)
            .unwrap_or_default();
        self.stress_metrics.release_requests = self
            .image_manager
            .dropped_count()
            .saturating_sub(self.stress_metrics.release_baseline);
        self.stress_metrics.release_failures = self.image_manager.release_failures();
        let output = self
            .qualification_output
            .clone()
            .or_else(|| std::env::var_os("FFRAMES_STRESS_OUTPUT").map(std::path::PathBuf::from));
        if let Some(output) = output {
            let result = serde_json::to_vec_pretty(&self.stress_metrics)
                .map_err(|e| e.to_string())
                .and_then(|bytes| std::fs::write(&output, bytes).map_err(|e| e.to_string()));
            if let Err(error) = result {
                eprintln!("Evidence write to {} failed: {error}", output.display());
                self.status_message = format!("Evidence write failed: {error}");
            }
        }
        if self.qualification_output.is_some() {
            cx.quit();
        }
    }

    pub fn confirm_stress_frame(&mut self, token: (u64, usize), cx: &mut Context<Self>) {
        if !self.stress.acknowledge(token.0, token.1) {
            return;
        }
        self.stress_completion = None;
        self.stress_painted = None;
        self.stress_metrics.last_completion = Some(Instant::now());
        self.stress_metrics.confirmed_presentations = self.stress.confirmed;
        self.stress_metrics.managed_current_images_high_water = self
            .stress_metrics
            .managed_current_images_high_water
            .max(self.image_manager.resident_count());
        if self.stress.active {
            self.status_message = format!(
                "Confirmed {}/1,000 GPUI presentations",
                self.stress.confirmed
            );
            self.schedule_next_stress_frame(cx);
        } else {
            self.status_message = format!(
                "Confirmed 1,000 GPUI presentations in {:?}; resident {}, evictions {}",
                self.stress_test_start
                    .map(|s| s.elapsed())
                    .unwrap_or_default(),
                self.image_manager.resident_count(),
                self.image_manager.dropped_count()
            );
            self.persist_stress(cx);
        }
        cx.notify();
    }

    pub fn select_preview(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        self.selected_source = None;
        let result = (|| {
            let bounds = self
                .preview_bounds
                .ok_or("Preview has not painted".to_string())?;
            let frame = self
                .displayed_source
                .as_ref()
                .ok_or("Display a worker frame with source metadata first".to_string())?;
            let (x, y) = SelectionSpike::map_viewport_to_canvas(
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                frame.header.width as f32,
                frame.header.height as f32,
                f32::from(position.x - bounds.left()),
                f32::from(position.y - bounds.top()),
            )
            .ok_or("Click is outside the image")?;
            let current = studio_agent_spike::source_revision(&frame.project_root)
                .map_err(|e| e.to_string())?;
            if current != frame.header.source_revision {
                return Err("Project source changed; rebuild before selecting".into());
            }
            frame
                .select(x, y, self.worker_generation, &current)
                .map_err(|e| e.to_string())
        })();
        match result {
            Ok(info) => {
                self.status_message = format!("Selected {}", info.element_id);
                self.selected_source = Some(info);
            }
            Err(error) => self.status_message = error.to_string(),
        }
        cx.notify();
    }

    pub fn load_initial_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step_frame(window, cx);
    }

    pub fn step_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stress.active {
            return;
        }
        self.displayed_source = None;
        self.pending_source = None;
        self.selected_source = None;
        let (width, height) = self.dimensions;
        let start = Instant::now();
        let (header, payload) = generate_reference_frame(
            "rev_spike",
            1,
            self.frame_index as u64,
            self.frame_index,
            width,
            height,
            true, // test stride stripping
        );

        match create_render_image(&header, &payload) {
            Ok(new_image) => {
                self.last_conversion_duration = start.elapsed();
                self.image_manager.replace_image(new_image, window);
                self.frame_index += 1;
                self.replacement_count += 1;
                self.status_message = format!(
                    "Presented frame #{} (stride {} bytes, {}x{}) in {:?}",
                    header.frame_index,
                    header.stride_bytes,
                    width,
                    height,
                    self.last_conversion_duration
                );
            }
            Err(err) => {
                self.status_message = format!("Frame conversion error: {err}");
            }
        }
        cx.notify();
    }
}

impl Render for StudioSpikeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Present any pending stress test image on the real painted frame boundary
        if let Some(img) = self.pending_stress_image.take() {
            self.image_manager.replace_image(img, window);
            self.replacement_count += 1;
        }

        // The setup image is a bootstrap preview; source inspection requires worker metadata.
        let setup_state = self.setup_view.read(cx).state.clone();
        if !self.stress.active
            && let crate::setup_view::SetupState::Rendered { ref frame_path, .. } = setup_state
            && self.displayed_setup_frame.as_ref() != Some(frame_path)
            && let Ok(bytes) = std::fs::read(frame_path)
            && let Ok(image) = image::load_from_memory(&bytes)
        {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            if let Ok(header) = fframes_studio_protocol::FrameHeader::new_straight_rgba(
                "setup", 1, 1, 0, width, height,
            ) && let Ok(image) = create_render_image(&header, rgba.as_raw())
            {
                self.image_manager.replace_image(image, window);
                self.dimensions = (width, height);
                self.displayed_setup_frame = Some(frame_path.clone());
                self.displayed_source = None;
                self.pending_source = None;
                self.status_message = format!(
                    "Displayed generated template frame: {}",
                    frame_path.display()
                );
            }
        }

        let current_image = self.image_manager.current_image();
        let resident_count = self.image_manager.resident_count();
        let queue_depth = self.image_manager.queue_depth();
        let dropped_count = self.image_manager.dropped_count();
        let (w, h) = self.dimensions;

        div()
            .id("studio-spike")
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x121212))
            .p_4()
            .gap_4()
            // Title Header
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(0x2A2A2A))
                    .pb_2()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(white())
                            .child("fframes studio — Native GPUI Spike"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(green())
                            .child(format!("PID: {}", std::process::id())),
                    ),
            )
            // Setup Doctor Section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.setup_view.clone()),
            )
            // IME Text Input Section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .bg(rgb(0x181818))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x2A2A2A))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xAAAAAA))
                            .child("1. IME / Text Input Composer (marked text & UTF-16 selection)"),
                    )
                    .child(self.text_input.clone()),
            )
            // Frame Image Presentation Section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .bg(rgb(0x181818))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x2A2A2A))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xAAAAAA))
                            .child("2. RGBA to GPUI BGRA Frame Presentation (Measured & Bounded)"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_4()
                            .child(
                                div()
                                    .id("source-preview")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            |app, event: &gpui::MouseDownEvent, _window, cx| {
                                                app.select_preview(event.position, cx)
                                            },
                                        ),
                                    )
                                    .w(px(320.0))
                                    .h(px(180.0))
                                    .bg(rgb(0x000000))
                                    .border_1()
                                    .border_color(rgb(0x333333))
                                    .rounded_sm()
                                    .overflow_hidden()
                                    .child(PreviewElement {
                                        app: cx.entity(),
                                        image: current_image,
                                        dimensions: self.dimensions,
                                        completion: self.stress_completion,
                                    }),
                            )
                            .child(
                                // Metrics panel
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(white())
                                    .child(format!("Canvas: {w}x{h} (letterboxed 320x180)"))
                                    .child(format!(
                                        "Last conversion: {:?}",
                                        self.last_conversion_duration
                                    ))
                                    .child(format!(
                                        "Resident image count: {resident_count} (cap: 1)"
                                    ))
                                    .child(format!("Queue depth: {queue_depth}"))
                                    .child(format!(
                                        "Total replacements: {}",
                                        self.replacement_count
                                    ))
                                    .child(format!("Dropped from GPU atlas: {dropped_count}"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x4AF626))
                                            .child(self.status_message.clone()),
                                    ),
                            ),
                    ),
            )
            .children(self.selected_source.as_ref().map(|info| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(rgb(0x1A1A1A))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x3B82F6))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x60A5FA))
                            .child(format!("Selected Anchor: {}", info.element_id))
                            .child(format!(
                                "{} (symbol: {})",
                                info.source_path, info.containing_symbol
                            )),
                    )
                    .child(div().text_xs().text_color(rgb(0x9CA3AF)).child(format!(
                        "Byte Span: {}..{}",
                        info.byte_span.0, info.byte_span.1
                    )))
                    .child(
                        div()
                            .p_2()
                            .bg(rgb(0x111111))
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0x2A2A2A))
                            .text_xs()
                            .text_color(rgb(0x34D399))
                            .child(info.code_snippet.clone()),
                    )
            }))
            .child(self.agent_spike.clone())
            // Actions
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .id("btn-step")
                            .px_3()
                            .py_1()
                            .bg(rgb(0x2563EB))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.step_frame(window, cx);
                            }))
                            .child("Step Reference Frame"),
                    )
                    .child(
                        div()
                            .id("btn-stress")
                            .px_3()
                            .py_1()
                            .bg(rgb(0x059669))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.start_stress_test(cx);
                            }))
                            .child("1,000 Confirmed Presentations"),
                    )
                    .child(
                        div()
                            .id("btn-connect-worker")
                            .px_3()
                            .py_1()
                            .bg(rgb(0x7C3AED))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.launch_real_worker(cx);
                            }))
                            .child("Connect Real Worker"),
                    )
                    .child(
                        div()
                            .id("btn-step-worker")
                            .px_3()
                            .py_1()
                            .bg(rgb(0x9333EA))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.step_real_worker_frame(window, cx);
                            }))
                            .child("Step Worker Frame"),
                    )
                    .child(
                        div()
                            .id("btn-crash-worker")
                            .px_3()
                            .py_1()
                            .bg(rgb(0xD97706))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.force_crash_worker(cx);
                            }))
                            .child("Crash Worker (Test Isolation)"),
                    )
                    .child(
                        div()
                            .id("btn-restart-worker")
                            .px_3()
                            .py_1()
                            .bg(rgb(0x2563EB))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.restart_worker(cx);
                            }))
                            .child("Restart Worker"),
                    )
                    .child(
                        div()
                            .id("btn-quit")
                            .px_3()
                            .py_1()
                            .bg(rgb(0xDC2626))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.quit();
                            }))
                            .child("Close / Exit"),
                    ),
            )
    }
}
