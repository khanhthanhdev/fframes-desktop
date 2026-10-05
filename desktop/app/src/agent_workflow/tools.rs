//! Per-project owner of the task MCP route: `ProjectToolBackend` + `ToolBroker` +
//! `WriterGate`. Created lazily on the first task that can use it and shut down with the
//! workflow; every task registers, is granted a capability and is revoked/unregistered on
//! every exit path.
use crate::{
    agent_tools::{
        BoundRevision, TaskLiveness, ToolBinding, ToolDispatcher,
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
use studio_engine::{AgentTaskContext, TaskIdentity};
use studio_project::ProjectId;
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
