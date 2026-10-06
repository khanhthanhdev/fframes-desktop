//! Per-project owner of the task MCP route: `ProjectToolBackend` + `ToolBroker` +
//! `WriterGate`. Created lazily on the first task that can use it and shut down with the
//! workflow; every task registers, is granted a capability and is revoked/unregistered on
//! every exit path.
use super::model::{EvidenceArtifact, EvidenceState, EvidenceView};
use crate::{
    agent_tools::{
        Assertions, BoundRevision, TaskLiveness, ToolBackend, ToolBinding, ToolCall,
        ToolDispatcher, ToolRequest,
        backend::{ProjectToolBackend, ToolBackendConfig, WriterGate},
        broker::{BrokerConfig, MAX_GRANT_TTL, ToolBroker, ToolGrant},
        mcp_server_for,
    },
    build_service::BuildService,
};
use parking_lot::Mutex;
use std::{path::PathBuf, sync::Arc};
use studio_agent_spike::McpStdioServer;
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{AgentTaskContext, CompiledScope, TaskIdentity};
use studio_project::{ProjectId, SourceRevision};
use studio_sdk::CompatibilityManifest;

/// What the tool backend compiles against.
#[derive(Clone)]
pub struct BuildSettings {
    pub service: BuildService,
    pub sdk: PathBuf,
    pub compatibility: CompatibilityManifest,
}

/// Tasks whose capability is still honoured. The workflow sets exactly the active task.
#[derive(Default)]
pub(crate) struct LiveTasks(Mutex<Option<TaskIdentity>>);

impl LiveTasks {
    pub(crate) fn set(&self, identity: Option<TaskIdentity>) {
        *self.0.lock() = identity;
    }
}

impl TaskLiveness for LiveTasks {
    fn is_live(&self, task: &TaskIdentity) -> bool {
        self.0.lock().as_ref() == Some(task)
    }
}

pub(crate) struct ToolRuntime {
    backend: Arc<ProjectToolBackend>,
    broker: ToolBroker,
    gate: WriterGate,
}

/// Where a project's tool runtime keeps its state.
pub(crate) struct ToolPlacement {
    pub project: ProjectId,
    /// The project's checkpoint object store.
    pub history: PathBuf,
    pub artifacts: PathBuf,
    pub builds: PathBuf,
    pub runtime_dir: PathBuf,
}

impl ToolRuntime {
    pub(crate) fn start(
        placement: ToolPlacement,
        build: &BuildSettings,
        processes: ProcessTreeManager,
        liveness: Arc<LiveTasks>,
    ) -> Result<Self, String> {
        let ToolPlacement {
            project,
            history,
            artifacts,
            builds,
            runtime_dir,
        } = placement;
        let gate = WriterGate::default();
        let backend = Arc::new(
            ProjectToolBackend::new(ToolBackendConfig {
                project_id: project,
                service: build.service.clone(),
                sdk: build.sdk.clone(),
                compatibility: build.compatibility.clone(),
                builds,
                history,
                artifacts,
                processes,
                gate: gate.clone(),
            })
            .map_err(|e| e.message)?,
        );
        let broker = ToolBroker::start(BrokerConfig {
            runtime_dir,
            dispatcher: ToolDispatcher::new(backend.clone()),
            liveness,
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            backend,
            broker,
            gate,
        })
    }

    /// A handle for a background evidence job. The job only ever executes read-only
    /// calls bound to one immutable revision of one registered task.
    pub(crate) fn backend(&self) -> Arc<ProjectToolBackend> {
        self.backend.clone()
    }

    pub(crate) fn gate(&self) -> &WriterGate {
        &self.gate
    }

    /// Registers the task and grants its capability. The capability is independent of
    /// the route the agent uses: the MCP server and the `studio-tools` command both only
    /// carry its file path.
    pub(crate) fn grant(&self, context: &AgentTaskContext) -> Result<ToolGrant, String> {
        self.backend.register_task(
            &context.identity,
            context.draft.clone(),
            context.source_base.revision().clone(),
        );
        self.broker
            .grant(
                ToolBinding {
                    task: context.identity.clone(),
                    revision: BoundRevision::Draft,
                },
                MAX_GRANT_TTL,
            )
            .map_err(|e| {
                self.backend.unregister_task(&context.identity.task);
                e.to_string()
            })
    }

    /// The `mcpServers` entry for a granted capability.
    pub(crate) fn mcp_server(
        grant: &ToolGrant,
        studio_mcp: &std::path::Path,
    ) -> Result<McpStdioServer, String> {
        mcp_server_for(grant, studio_mcp).map_err(|e| e.to_string())
    }

    /// The agent's session is over: its capability dies.
    pub(crate) fn revoke(&self, identity: &TaskIdentity) {
        self.broker.revoke(identity);
    }

    /// The task ended: capability, workers, restored trees and artifacts are released.
    pub(crate) fn end_task(&self, identity: &TaskIdentity) {
        self.broker.revoke(identity);
        self.backend.unregister_task(&identity.task);
    }

    pub(crate) fn grants(&self) -> usize {
        self.broker.stats().grants
    }

    pub(crate) fn workers(&self) -> usize {
        self.backend.worker_count()
    }

    pub(crate) fn shutdown(self) {
        self.broker.shutdown();
        self.backend.close();
    }
}

/// Most selected frames tiled into the one selected strip artifact.
const MAX_EVIDENCE_STRIP_FRAMES: usize = 6;
/// Tile scale of evidence renders: small, so a strip stays far below the artifact bound.
const EVIDENCE_SCALE: f64 = 0.25;

/// Renders the actual frames around a frozen scope from ONE immutable `revision` of the
/// registered task: a strip of up to [`MAX_EVIDENCE_STRIP_FRAMES`] evenly spaced selected
/// frames plus the frames just outside the selection on either side (the adjacent
/// boundaries). Blocking: run it on a job thread. Any refusal makes the whole set
/// `Unavailable` with the tool's own message; nothing is ever substituted.
pub(crate) fn render_scope_evidence(
    backend: &ProjectToolBackend,
    identity: &TaskIdentity,
    revision: &SourceRevision,
    scope: &CompiledScope,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> EvidenceView {
    let short = super::present::short(revision.as_str());
    let binding = ToolBinding {
        task: identity.clone(),
        revision: BoundRevision::Fixed(revision.clone()),
    };
    let mut plan: Vec<(&str, ToolCall)> = Vec::new();
    if scope.start_frame < scope.end_frame {
        let count = (scope.end_frame - scope.start_frame).min(MAX_EVIDENCE_STRIP_FRAMES);
        plan.push((
            "selected",
            ToolCall::RenderStrip {
                start: scope.start_frame,
                end: scope.end_frame - 1,
                count,
                scale: EVIDENCE_SCALE,
            },
        ));
    }
    if scope.start_frame > 0 && scope.start_frame <= scope.total_frames {
        plan.push((
            "before-boundary",
            ToolCall::RenderFrame {
                frame: scope.start_frame - 1,
                scale: EVIDENCE_SCALE,
            },
        ));
    }
    if scope.end_frame < scope.total_frames {
        plan.push((
            "after-boundary",
            ToolCall::RenderFrame {
                frame: scope.end_frame,
                scale: EVIDENCE_SCALE,
            },
        ));
    }
    let unavailable = |note: String| EvidenceView {
        state: EvidenceState::Unavailable,
        revision: short.clone(),
        artifacts: Vec::new(),
        note: Some(super::present::message(&note)),
    };
    if plan.is_empty() {
        return unavailable("the scope selects no frame and has no neighbouring frame".into());
    }
    let mut artifacts = Vec::new();
    for (label, call) in plan {
        let frames: Vec<usize> = match &call {
            ToolCall::RenderStrip {
                start, end, count, ..
            } => (0..*count)
                .map(|i| {
                    if *count == 1 {
                        *start
                    } else {
                        start + (end - start) * i / (count - 1)
                    }
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            ToolCall::RenderFrame { frame, .. } => vec![*frame],
            _ => Vec::new(),
        };
        let request = ToolRequest {
            assertions: Assertions::default(),
            call,
        };
        match backend.execute(&binding, &request, cancelled) {
            Ok(reply) => {
                let Some(artifact) = reply.artifacts.into_iter().next() else {
                    return unavailable(format!("the {label} render returned no image"));
                };
                artifacts.push(EvidenceArtifact {
                    label: label.to_owned(),
                    frames,
                    id: artifact.id,
                    bytes: artifact.bytes,
                    sha256: artifact.sha256,
                    width: artifact.width,
                    height: artifact.height,
                });
            }
            Err(error) => return unavailable(format!("{label}: {error}")),
        }
    }
    EvidenceView {
        state: EvidenceState::Ready,
        revision: short,
        artifacts,
        note: None,
    }
}
