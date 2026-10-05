//! Blocking work the workflow actor never does itself: adapter probing, the
//! quiesce -> capture -> validate pipeline, publication and Undo. Each runs on its own
//! owned thread and reports back with one message; none ever waits on the actor.
use super::{
    actor::Msg,
    model::{ChangeCard, HandoffKind, TaskPhase, ValidationCard},
    present,
    tools::BuildSettings,
};
use crate::{
    agent_tools::backend::{WriterGate, WriterGuard},
    candidate_runner::{CandidateRun, CandidateRunConfig, RunScopes, run_candidate_validation},
    preview_coordinator::StagedPreview,
};
use parking_lot::{Mutex, MutexGuard};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc::Sender},
    time::{Duration, Instant},
};
use studio_agent_spike::{
    AcpDriver, AdapterConfig, AgentFailure, DiscoveryReport, DriverOutcome, ExecutableSearch,
    ProbeOptions, probe_adapter,
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    Controller, EngineError, Promotion, PromotionError, QuiescenceEvidence, TaskIdentity,
    TaskState, TurnCompletion, WriterGeneration, WriterObservation,
    candidate_validation::{CapturedCandidate, FailureContext, NextStep},
};
use studio_project::checkpoint::Checkpoints;

/// How often a wait for the controller re-checks Stop.
const LOCK_POLL: Duration = Duration::from_millis(5);

/// Receives the staged preview and the committed promotion. Called on a workflow thread,
/// never the UI thread; a UI implementation hands the value to its own thread.
pub trait PreviewHandoff: Send + Sync + 'static {
    /// `Ok` when the coordinator adopted the staged preview; `Err` is shown as the reason
    /// the accepted revision is awaiting its preview (the old preview keeps playing).
    fn handoff(&self, handoff: PromotionHandoff) -> Result<(), String>;
}

pub struct PromotionHandoff {
    pub kind: HandoffKind,
    pub promotion: Promotion,
    /// Present when validation staged playback for the candidate; `None` means the
    /// accepted source needs an ordinary preview build.
    pub staged: Option<StagedPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HandoffOutcome {
    /// No preview sink is configured: the revision awaits an ordinary preview build.
    NoSink,
    Adopted,
    Failed(String),
}

fn deliver(
    sink: &Option<Arc<dyn PreviewHandoff>>,
    kind: HandoffKind,
    promotion: &Promotion,
    staged: Option<StagedPreview>,
) -> HandoffOutcome {
    let Some(sink) = sink else {
        // Nobody will adopt it: this is a workflow thread, so the worker is reaped (and
        // its exit verified) before the commit is reported, not left to a background drop.
        if let Some(staged) = staged {
            let _ = staged.teardown_now();
        }
        return HandoffOutcome::NoSink;
    };
    match sink.handoff(PromotionHandoff {
        kind,
        promotion: promotion.clone(),
        staged,
    }) {
        Ok(()) => HandoffOutcome::Adopted,
        Err(reason) => HandoffOutcome::Failed(present::message(&reason)),
    }
}

// ---- cancellation / commit boundary --------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateState {
    Open,
    Cancelled,
    /// Publication is running: it cannot be cancelled any more.
    Committing,
}

/// Stop vs. publication. `cancel` wins until a publication has begun; afterwards Stop is
/// refused (`false`) and the caller requests recovery instead of pretending to cancel.
#[derive(Debug)]
pub(crate) struct Gate(Mutex<GateState>);

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(GateState::Open)))
    }

    /// `true`: cancellation is in effect. `false`: publication already began.
    pub(crate) fn cancel(&self) -> bool {
        let mut state = self.0.lock();
        match *state {
            GateState::Committing => false,
            _ => {
                *state = GateState::Cancelled;
                true
            }
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        *self.0.lock() == GateState::Cancelled
    }

    /// Enters the uncancellable boundary; `false` if Stop already won.
    pub(crate) fn begin_commit(&self) -> bool {
        let mut state = self.0.lock();
        if *state == GateState::Cancelled {
            return false;
        }
        *state = GateState::Committing;
        true
    }

    /// The publication ended without committing; Stop works again.
    pub(crate) fn reopen(&self) {
        let mut state = self.0.lock();
        if *state == GateState::Committing {
            *state = GateState::Open;
        }
    }
}

/// What every job needs to reach the project.
#[derive(Clone)]
pub(crate) struct Env {
    pub controller: Arc<Mutex<Controller>>,
    /// The project's checkpoint object store.
    pub history: PathBuf,
    pub build: BuildSettings,
    pub builds: PathBuf,
    pub tx: Sender<Msg>,
}

impl Env {
    fn config(&self) -> CandidateRunConfig {
        CandidateRunConfig {
            service: self.build.service.clone(),
            sdk: self.build.sdk.clone(),
            compatibility: self.build.compatibility.clone(),
            builds: self.builds.clone(),
        }
    }
}

// ---- adapter probe --------------------------------------------------------------------------

pub(crate) fn run_probe(
    adapter: AdapterConfig,
    search: ExecutableSearch,
    auth_method: Option<String>,
    scratch: PathBuf,
    timeout: Duration,
    processes: ProcessTreeManager,
) -> DiscoveryReport {
    let _ = std::fs::create_dir_all(&scratch);
    let mut options = ProbeOptions::new(scratch.clone());
    options.auth_method = auth_method;
    options.timeout = timeout;
    let report = probe_adapter(&adapter, &search, &options, &processes);
    processes.shutdown(Duration::from_millis(300));
    let _ = std::fs::remove_dir_all(&scratch);
    report
}

// ---- the editing -> candidate pipeline --------------------------------------------------------

pub(crate) struct PipelineInput {
    pub env: Env,
    pub identity: TaskIdentity,
    pub driver: AcpDriver,
    pub session: Option<String>,
    pub writer: WriterGeneration,
    pub gate_of_tools: Option<WriterGate>,
    pub scopes: RunScopes,
    pub gate: Arc<Gate>,
    pub playhead: usize,
    pub repair_used: u32,
}

/// The task's retained candidate: immutable bytes, the passing report, the staged
/// preview (reaped when dropped) and the scopes that own validation's processes.
pub(crate) struct Retained {
    pub captured: CapturedCandidate,
    pub run: CandidateRun,
    pub scopes: RunScopes,
}

impl Drop for Retained {
    /// A retained candidate is only ever dropped on a workflow thread: its staged worker
    /// is reaped (and verified) here, so ending the task leaves no process behind.
    fn drop(&mut self) {
        if let Some(staged) = self.run.staged.take() {
            let _ = staged.teardown_now();
        }
    }
}

pub(crate) enum PipelineEnd {
    /// The adapter failed (or crashed); nothing was captured. The task is still open.
    ProviderFailed(AgentFailure),
    /// Quiescence or capture was refused; the task is still open (`Quiescing`).
    Blocked(EngineError),
    /// An unexpected engine error while routing the report; the task is still open.
    Error(EngineError),
    Cancelled,
    /// Validated; the engine task is `CandidateReady`.
    Candidate {
        retained: Box<Retained>,
        changes: ChangeCard,
        card: Box<ValidationCard>,
    },
    /// Failed validation with the automatic repair available; the task is `RepairNeeded`.
    Repair {
        context: Box<FailureContext>,
        changes: ChangeCard,
        card: Box<ValidationCard>,
    },
    /// Failed validation, terminal; the engine already ended the task `Failed`.
    Retained {
        reason: String,
        changes: ChangeCard,
        card: Box<ValidationCard>,
    },
}

fn progress(env: &Env, phase: TaskPhase) {
    let _ = env.tx.send(Msg::Progress(phase));
}

/// Reaps the adapter and hands the engine what the driver saw of the writer. Runs on
/// every exit that reaps a driver: the observation (ownership demotion, escaped
/// descendants) cannot be recovered later, once the parent process is gone.
pub(crate) fn reap_and_observe(
    env: &Env,
    identity: &TaskIdentity,
    driver: AcpDriver,
) -> (DriverOutcome, Result<(), EngineError>) {
    let outcome = driver.shutdown();
    let observed = observe_writer(&env.controller, identity, &outcome);
    (outcome, observed)
}

/// Propagates a driver outcome's writer facts to the engine task.
pub(crate) fn observe_writer(
    controller: &Mutex<Controller>,
    identity: &TaskIdentity,
    outcome: &DriverOutcome,
) -> Result<(), EngineError> {
    let observed = WriterObservation {
        ownership: outcome.ownership.clone(),
        escaped_pids: outcome.escaped_pids.clone(),
    };
    controller.lock().agent_observe_writer(identity, &observed)
}

pub(crate) fn run_pipeline(input: PipelineInput) -> PipelineEnd {
    let PipelineInput {
        env,
        identity,
        driver,
        session,
        writer,
        gate_of_tools,
        scopes,
        gate,
        playhead,
        repair_used,
    } = input;
    progress(&env, TaskPhase::Quiescing);
    // Reap the adapter first: quiescence is only provable once its tree is gone. What the
    // driver saw of the writer reaches the engine before anything else is decided, so a
    // failed, cancelled or closed task can never be classified from a blind teardown.
    let (outcome, observed) = reap_and_observe(&env, &identity, driver);
    if let Err(error) = observed {
        return PipelineEnd::Blocked(error);
    }
    if let Some(failure) = outcome.failure.clone() {
        return PipelineEnd::ProviderFailed(failure);
    }
    if gate.is_cancelled() {
        return PipelineEnd::Cancelled;
    }
    // Tool snapshots and capture are serialized through the writer gate. A snapshot in
    // flight is short; waiting is bounded and capture verifies the tree twice anyway.
    let guard = gate_of_tools.and_then(|gate| acquire(&gate));
    let last = outcome.last_prompt.clone();
    let evidence = QuiescenceEvidence {
        identity: identity.clone(),
        provider_session: session,
        writer,
        completion: last.as_ref().map_or(TurnCompletion::None, |p| {
            TurnCompletion::from_stop_reason(p.stop_reason.as_str())
        }),
        cancel_requested: last.as_ref().is_some_and(|p| p.cancel_requested),
        unresolved_requests: last.as_ref().map_or(0, |p| p.unresolved_permissions),
        observed: WriterObservation {
            ownership: outcome.ownership.clone(),
            escaped_pids: outcome.escaped_pids.clone(),
        },
    };
    let captured = {
        let Some(mut controller) = lock_unless_cancelled(&env.controller, &gate) else {
            return PipelineEnd::Cancelled;
        };
        let ticket = (|| {
            controller.agent_task_transition(&identity, TaskState::Quiescing)?;
            controller.agent_observe_writer(&identity, &evidence.observed)?;
            controller.agent_complete_quiescence(&identity, &evidence)
        })();
        let ticket = match ticket {
            Ok(ticket) => ticket,
            Err(error) => return PipelineEnd::Blocked(error),
        };
        progress(&env, TaskPhase::Capturing);
        match controller.agent_capture_candidate(ticket) {
            Ok(captured) => captured,
            Err(error) => return PipelineEnd::Blocked(error),
        }
    };
    drop(guard);
    let changes = present::change_card(captured.changes());
    if gate.is_cancelled() {
        return PipelineEnd::Cancelled;
    }
    progress(&env, TaskPhase::Validating);
    let checkpoints = match Checkpoints::new(&env.history) {
        Ok(checkpoints) => checkpoints,
        Err(error) => return PipelineEnd::Error(error.into()),
    };
    let run = run_candidate_validation(
        &captured,
        &checkpoints,
        &env.config(),
        &scopes,
        playhead,
        repair_used,
    );
    if gate.is_cancelled() {
        return PipelineEnd::Cancelled;
    }
    let card = Box::new(present::validation_card(&run.report));
    let step = env
        .controller
        .lock()
        .agent_apply_validation(&identity, &run.report);
    match step {
        Err(error) => PipelineEnd::Error(error),
        Ok(NextStep::Accept) => PipelineEnd::Candidate {
            retained: Box::new(Retained {
                captured,
                run,
                scopes,
            }),
            changes,
            card,
        },
        Ok(NextStep::Repair { context, .. }) => PipelineEnd::Repair {
            context: Box::new(context),
            changes,
            card,
        },
        Ok(NextStep::Retain { reason, .. }) => PipelineEnd::Retained {
            reason,
            changes,
            card,
        },
    }
}

fn acquire(gate: &WriterGate) -> Option<WriterGuard> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(guard) = gate.try_acquire() {
            return Some(guard);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---- job admission ---------------------------------------------------------------------------

/// Which background operation a job thread belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobKind {
    Probe,
    Pipeline,
    Promote,
    Undo,
}

impl JobKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Pipeline => "pipeline",
            Self::Promote => "promote",
            Self::Undo => "undo",
        }
    }
}

/// Deterministic failure injection for background jobs (tests): the job named by the
/// callback's argument is refused admission or panics as it starts.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFault {
    None,
    /// The thread cannot be created.
    Refuse,
    /// The job panics before doing any work.
    Panic,
}

#[doc(hidden)]
pub type JobFaults = Arc<dyn Fn(&str) -> JobFault + Send + Sync>;

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "the worker panicked".to_owned())
}

/// Starts a job thread. On refusal the payload is handed back so the owning operation
/// can settle (and release what the payload held) at once; a job that panics reports
/// [`Msg::JobAborted`] instead of vanishing without a result.
pub(crate) fn spawn_job<T: Send + 'static>(
    kind: JobKind,
    faults: Option<&JobFaults>,
    tx: Sender<Msg>,
    payload: T,
    run: impl FnOnce(T) + Send + 'static,
) -> Result<std::thread::JoinHandle<()>, (String, T)> {
    let fault = faults.map_or(JobFault::None, |f| f(kind.name()));
    if fault == JobFault::Refuse {
        return Err(("injected thread creation failure".to_owned(), payload));
    }
    let slot = Arc::new(Mutex::new(Some(payload)));
    let inner = slot.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("studio-workflow-{}", kind.name()))
        .spawn(move || {
            let Some(payload) = inner.lock().take() else {
                return;
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if fault == JobFault::Panic {
                    panic!("injected worker failure");
                }
                run(payload)
            }));
            if let Err(panic) = outcome {
                let _ = tx.send(Msg::JobAborted {
                    kind,
                    message: panic_text(panic.as_ref()),
                });
            }
        });
    match spawned {
        Ok(handle) => Ok(handle),
        Err(error) => Err((
            error.to_string(),
            slot.lock()
                .take()
                .expect("a job that never ran keeps its payload"),
        )),
    }
}

// ---- promotion --------------------------------------------------------------------------------

pub(crate) enum PromoteEnd {
    Committed {
        promotion: Box<Promotion>,
        handoff: HandoffOutcome,
        scopes: RunScopes,
    },
    /// Stop won before the commit boundary.
    Cancelled(Box<Retained>),
    Failed {
        error: EngineError,
        retained: Box<Retained>,
    },
}

/// Takes the controller unless Stop wins first: the wait is polled so a cancelled gate
/// is noticed within a few milliseconds, whoever holds the lock.
fn lock_unless_cancelled<'a>(
    controller: &'a Mutex<Controller>,
    gate: &Gate,
) -> Option<MutexGuard<'a, Controller>> {
    loop {
        if gate.is_cancelled() {
            return None;
        }
        if let Some(guard) = controller.try_lock_for(LOCK_POLL) {
            return Some(guard);
        }
    }
}

pub(crate) fn run_promote(
    env: Env,
    retained: Retained,
    gate: Arc<Gate>,
    sink: Option<Arc<dyn PreviewHandoff>>,
) -> PromoteEnd {
    // Waiting for the controller, reconciling, verifying objects and planning are all
    // cancellable: the uncancellable boundary starts inside the engine, after every
    // refusal check and immediately before the first durable mutation.
    let result = {
        let Some(mut controller) = lock_unless_cancelled(&env.controller, &gate) else {
            return PromoteEnd::Cancelled(Box::new(retained));
        };
        controller.apply_candidate_gated(&retained.captured, &retained.run.report, &|| {
            gate.begin_commit()
        })
    };
    match result {
        Ok(promotion) => {
            let mut retained = retained;
            let staged = retained.run.staged.take();
            let scopes = RunScopes {
                compiler: retained.scopes.compiler.clone(),
                worker: retained.scopes.worker.clone(),
            };
            let _ = env.tx.send(Msg::Committed {
                published: promotion.record.published.as_str().to_owned(),
                kind: HandoffKind::Apply,
            });
            let handoff = deliver(&sink, HandoffKind::Apply, &promotion, staged);
            PromoteEnd::Committed {
                promotion: Box::new(promotion),
                handoff,
                scopes,
            }
        }
        Err(EngineError::Promotion(PromotionError::Declined)) => {
            PromoteEnd::Cancelled(Box::new(retained))
        }
        Err(error) => {
            gate.reopen();
            PromoteEnd::Failed {
                error,
                retained: Box::new(retained),
            }
        }
    }
}

// ---- Undo -------------------------------------------------------------------------------------

pub(crate) enum UndoEnd {
    Committed {
        promotion: Box<Promotion>,
        handoff: HandoffOutcome,
        scopes: RunScopes,
    },
    Cancelled,
    ValidationFailed(Box<ValidationCard>),
    Failed(EngineError),
}

pub(crate) fn run_undo(
    env: Env,
    target: Option<String>,
    scopes: RunScopes,
    gate: Arc<Gate>,
    playhead: usize,
    sink: Option<Arc<dyn PreviewHandoff>>,
) -> UndoEnd {
    let prepared = {
        let Some(mut controller) = lock_unless_cancelled(&env.controller, &gate) else {
            return UndoEnd::Cancelled;
        };
        controller.prepare_undo(target.as_deref())
    };
    let preparation = match prepared {
        Ok(preparation) => preparation,
        Err(error) => return UndoEnd::Failed(error),
    };
    if gate.is_cancelled() {
        return UndoEnd::Cancelled;
    }
    let _ = env.tx.send(Msg::UndoProgress("Validating the Undo".into()));
    let checkpoints = match Checkpoints::new(&env.history) {
        Ok(checkpoints) => checkpoints,
        Err(error) => return UndoEnd::Failed(error.into()),
    };
    let mut run = run_candidate_validation(
        preparation.captured(),
        &checkpoints,
        &env.config(),
        &scopes,
        playhead,
        0,
    );
    if gate.is_cancelled() {
        return UndoEnd::Cancelled;
    }
    if !run.report.passed() {
        return UndoEnd::ValidationFailed(Box::new(present::validation_card(&run.report)));
    }
    let _ = env.tx.send(Msg::UndoProgress("Publishing the Undo".into()));
    let result = {
        let Some(mut controller) = lock_unless_cancelled(&env.controller, &gate) else {
            return UndoEnd::Cancelled;
        };
        controller.undo_task_gated(&preparation, &run.report, &|| gate.begin_commit())
    };
    match result {
        Ok(promotion) => {
            let _ = env.tx.send(Msg::Committed {
                published: promotion.record.published.as_str().to_owned(),
                kind: HandoffKind::Undo,
            });
            let handoff = deliver(&sink, HandoffKind::Undo, &promotion, run.staged.take());
            UndoEnd::Committed {
                promotion: Box::new(promotion),
                handoff,
                scopes,
            }
        }
        Err(EngineError::Promotion(PromotionError::Declined)) => UndoEnd::Cancelled,
        Err(error) => {
            gate.reopen();
            UndoEnd::Failed(error)
        }
    }
}
