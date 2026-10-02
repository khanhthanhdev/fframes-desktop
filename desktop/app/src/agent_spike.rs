use crate::{
    frame_image::create_render_image, selection_spike::DisplayedSourceFrame, text_input::TextInput,
    worker_client::WorkerClient, worker_project,
};
use gpui::{
    AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, RenderImage, StatefulInteractiveElement, Styled, Window, div, rgb, white,
};
use parking_lot::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use studio_agent_spike::{AcpSession, AdapterConfig, SessionControl, copy_draft, source_revision};
use studio_bootstrap::ProcessTreeManager;
use studio_sdk::CompatibilityManifest;

pub struct CandidatePreview {
    pub worker: WorkerClient,
    pub image: Arc<RenderImage>,
    pub source: DisplayedSourceFrame,
}
pub struct CandidateReady;
pub struct AgentSpike {
    pub config: Entity<TextInput>,
    pub prompt: Entity<TextInput>,
    pub control: Arc<Mutex<Option<SessionControl>>>,
    pub active: bool,
    pub status: String,
    pub transcript: String,
    pub candidate: Option<CandidatePreview>,
    pub project_root: PathBuf,
    pub sdk_root: PathBuf,
    pub manifest: CompatibilityManifest,
    pub process_tree: ProcessTreeManager,
    cancelled: Arc<AtomicBool>,
    generation: u64,
}
impl EventEmitter<CandidateReady> for AgentSpike {}
impl AgentSpike {
    pub fn new(
        project_root: PathBuf,
        sdk_root: PathBuf,
        manifest: CompatibilityManifest,
        process_tree: ProcessTreeManager,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_app_quit(|panel, cx| {
            panel.cancelled.store(true, Ordering::SeqCst);
            let control = panel.control.lock().clone();
            let manager = panel.process_tree.clone();
            cx.background_executor().spawn(async move {
                if let Some(control) = control {
                    let _ = control.cancel();
                }
                manager.terminate_all(Duration::from_millis(300));
            })
        })
        .detach();
        let config = cx.new(TextInput::new);
        config.update(cx, |input, cx| {
            input.placeholder = "Adapter JSON: executable, args, auth_env_names".into();
            input.set_text(
                "{\"executable\":\"\",\"args\":[],\"auth_env_names\":[]}",
                cx,
            );
        });
        let prompt = cx.new(TextInput::new);
        prompt.update(cx,|input,cx|input.set_text("Change the marked intro.title text to Hello from the agent. Preserve its source markers and SVG ID. Ask for permission before editing.",cx));
        Self {
            config,
            prompt,
            control: Arc::new(Mutex::new(None)),
            active: false,
            status: "NOT_RUN — configure an authenticated ACP adapter".into(),
            transcript: String::new(),
            candidate: None,
            project_root,
            sdk_root,
            manifest,
            process_tree,
            cancelled: Arc::new(AtomicBool::new(false)),
            generation: 1,
        }
    }
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.active {
            return;
        }
        let config: AdapterConfig = match serde_json::from_str(self.config.read(cx).content()) {
            Ok(config) => config,
            Err(_) => {
                self.status = "Invalid adapter JSON configuration".into();
                cx.notify();
                return;
            }
        };
        if config.executable.is_empty() {
            self.status = "Set the ACP adapter executable first".into();
            cx.notify();
            return;
        }
        let prompt = self.prompt.read(cx).content().to_string();
        let source = self.project_root.clone();
        let sdk = self.sdk_root.clone();
        let manifest = self.manifest.clone();
        // Each run owns its cleanup domain, including cancellation still in flight.
        self.process_tree = ProcessTreeManager::new();
        let manager = self.process_tree.clone();
        let control = self.control.clone();
        self.cancelled = Arc::new(AtomicBool::new(false));
        let cancelled = self.cancelled.clone();
        let completion_cancelled = cancelled.clone();
        self.generation += 1;
        let generation = self.generation;
        let baseline_generation = crate::worker_client::allocate_worker_generation();
        self.active = true;
        self.status = "Preparing isolated draft...".into();
        self.transcript.clear();
        *self.control.lock() = None;
        let task = cx.background_executor().spawn(async move {
            (|| -> Result<CandidatePreview, String> {
                worker_project::create_worker_project(&source, &sdk)?;
                let parent = source.parent().ok_or("Project parent missing")?;
                let draft = parent.join(format!(
                    "agent-draft-{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos()
                ));
                copy_draft(&source, &draft).map_err(|e| e.to_string())?;
                if cancelled.load(Ordering::SeqCst) {
                    return Err("Cancelled; draft retained".into());
                }
                // Render a real baseline before letting the adapter mutate the draft.
                let mut baseline = worker_project::launch_worker(
                    &draft,
                    &sdk,
                    manifest.clone(),
                    baseline_generation,
                    &manager,
                )?;
                baseline
                    .request_render_frame(0)
                    .map_err(|e| e.to_string())?;
                let before = baseline
                    .latest_pixels()
                    .ok_or("Baseline pixels missing")?
                    .to_vec();
                baseline.force_crash().map_err(|e| e.to_string())?;
                drop(baseline);
                let before_revision = source_revision(&draft).map_err(|e| e.to_string())?;
                if cancelled.load(Ordering::SeqCst) {
                    return Err("Cancelled; draft retained".into());
                }
                let mut session = AcpSession::spawn(&config, &draft, manager.clone())
                    .map_err(|e| e.to_string())?;
                *control.lock() = Some(session.control.clone());
                if cancelled.load(Ordering::SeqCst) {
                    let _ = session.control.cancel();
                    return Err("Cancelled; draft retained".into());
                }
                let mut next_prompt = prompt;
                let reason = loop {
                    let reason = session
                        .run_prompt(&draft, &next_prompt)
                        .map_err(|e| e.to_string())?;
                    if cancelled.load(Ordering::SeqCst)
                        || reason != "end_turn"
                        || source_revision(&draft).map_err(|e| e.to_string())? != before_revision
                    {
                        break reason;
                    }
                    next_prompt = session
                        .wait_for_reply(Duration::from_secs(900))
                        .map_err(|e| e.to_string())?;
                };
                // Reap the writer before hashing/rebuilding the candidate.
                drop(session);
                if cancelled.load(Ordering::SeqCst) || reason != "end_turn" {
                    return Err(format!(
                        "Task stopped ({reason}); draft retained at {}",
                        draft.display()
                    ));
                }
                let revision = source_revision(&draft).map_err(|e| e.to_string())?;
                if revision == before_revision {
                    return Err("Agent completed without source changes; draft retained".into());
                }
                let mut worker = worker_project::launch_worker(
                    &draft,
                    &sdk,
                    manifest,
                    crate::worker_client::allocate_worker_generation(),
                    &manager,
                )?;
                if source_revision(&draft).map_err(|e| e.to_string())? != revision {
                    return Err("Source changed during candidate build".into());
                }
                worker.request_render_frame(0).map_err(|e| e.to_string())?;
                let header = worker
                    .latest_header()
                    .cloned()
                    .ok_or("Candidate header missing")?;
                let pixels = worker.latest_pixels().ok_or("Candidate pixels missing")?;
                if pixels == before {
                    return Err(
                        "Source changed but frame pixels did not; candidate unqualified".into(),
                    );
                }
                let image = create_render_image(&header, pixels).map_err(|e| e.to_string())?;
                let elements = worker
                    .request_elements(&header)
                    .map_err(|e| e.to_string())?;
                let source = DisplayedSourceFrame {
                    header,
                    elements,
                    project_root: draft,
                };
                let title = source.elements.first().ok_or("Candidate anchor missing")?;
                source
                    .select(
                        title.bounds.x + 1.,
                        title.bounds.y + 1.,
                        worker.generation(),
                        worker.revision(),
                    )
                    .map_err(|e| e.to_string())?;
                if cancelled.load(Ordering::SeqCst) {
                    return Err("Cancelled before candidate presentation; draft retained".into());
                }
                Ok(CandidatePreview {
                    worker,
                    image,
                    source,
                })
            })()
        });
        cx.spawn(async move |this,cx| {
            let result=task.await;
            let _=this.update(cx,|panel,cx| {
                if panel.generation != generation { return; }
                panel.active=false;
                if completion_cancelled.load(Ordering::SeqCst) {
                    drop(result);
                    panel.status="Cancelled; candidate discarded, draft retained".into();
                    cx.notify();
                    return;
                }
                match result {Ok(candidate)=>{panel.project_root=candidate.source.project_root.clone();panel.status="Changed Rust built; frame pixels and refreshed title anchor verified. Live qualification still needs cancellation evidence.".into();panel.candidate=Some(candidate);cx.emit(CandidateReady);},Err(error)=>panel.status=format!("Agent flow stopped: {error}")}
                cx.notify();
            });
        }).detach();
        self.poll_progress(cx);
        cx.notify();
    }
    fn poll_progress(&self, cx: &mut Context<Self>) {
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let active = this
                    .update(cx, |panel, cx| {
                        if panel.generation != generation {
                            return false;
                        }
                        if let Some(control) = panel.control.lock().as_ref() {
                            let progress = control.progress.lock();
                            panel.transcript = progress.transcript.clone();
                            if panel.active {
                                panel.status = progress.status.clone();
                            }
                        }
                        cx.notify();
                        panel.active
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        let generation = self.generation;
        self.cancelled.store(true, Ordering::SeqCst);
        let control = self.control.lock().clone();
        let manager = self.process_tree.clone();
        let task = cx.background_executor().spawn(async move {
            if let Some(control) = control {
                control.cancel().map_err(|e| e.to_string())?;
            }
            manager.terminate_all(Duration::from_millis(300));
            if manager.active_count() != 0 {
                return Err("Owned process tree still active after cancellation".into());
            }
            Ok::<_, String>(())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.generation != generation {
                    return;
                }
                panel.status = match result {
                    Ok(()) => "Cancelled; adapter/build tree reaped, draft retained".into(),
                    Err(e) => format!("Cancellation cleanup failed: {e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for AgentSpike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let permissions = self
            .control
            .lock()
            .as_ref()
            .map(|c| c.progress.lock().permissions.clone())
            .unwrap_or_default();
        let awaiting_reply = self
            .control
            .lock()
            .as_ref()
            .is_some_and(|control| control.progress.lock().awaiting_reply);
        div().flex().flex_col().gap_2().p_2().text_xs().text_color(white())
            .child("ACP adapter — runs with provider-managed authentication and edits an isolated draft; process isolation is not a security sandbox")
            .child(self.config.clone()).child(self.prompt.clone())
            .child(div().flex().gap_2()
                .child(div().id("agent-run").p_2().bg(rgb(0x2563eb)).cursor_pointer().on_click(cx.listener(|panel,_,_,cx|panel.start(cx))).child("Run Agent Edit"))
                .child(div().id("agent-stop").p_2().bg(rgb(0xb45309)).cursor_pointer().on_click(cx.listener(|panel,_,_,cx|panel.cancel(cx))).child("Stop"))
                .children(awaiting_reply.then(|| div().id("agent-reply").p_2().bg(rgb(0x2563eb)).cursor_pointer()
                    .on_click(cx.listener(|panel,_,_,cx| {
                        let reply=panel.prompt.read(cx).content().to_owned();
                        let result=panel.control.lock().as_ref().ok_or("No active adapter".to_owned())
                            .and_then(|control|control.submit_reply(&reply).map_err(|e|e.to_string()));
                        panel.status=match result {Ok(())=>"Reply queued for this session".into(),Err(error)=>error};
                        cx.notify();
                    })).child("Send Reply"))))
            .child(self.status.clone()).child(self.transcript.clone())
            .children(permissions.into_iter().enumerate().map(|(permission_index, permission)| {
                let id=permission.id;
                div().flex().gap_2().child(permission.title).children(permission.options.into_iter().enumerate().map(|(index,option)| {
                    let id=id.clone();
                    div().id(gpui::SharedString::from(format!("agent-permission-{permission_index}-{index}"))).p_2().bg(rgb(0x334155)).cursor_pointer().child(option.name)
                        .on_click(cx.listener(move |panel,_,_,cx| {
                            let control=panel.control.lock().clone();let id=id.clone();let option=option.option_id.clone();
                            let task=cx.background_executor().spawn(async move {control.ok_or("No active adapter".to_string())?.choose_permission(&id,&option).map_err(|e|e.to_string())});
                            cx.spawn(async move |this,cx|{let result=task.await;let _=this.update(cx,|panel,cx|{if let Err(e)=result{panel.status=e;}cx.notify();});}).detach();
                        }))
                }))
            }))
    }
}
