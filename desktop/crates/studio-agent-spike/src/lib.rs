pub mod discovery;
pub mod driver;
pub mod supervisor;

pub use discovery::{
    AdapterLaunch, AdapterStatus, DiscoveryReport, ExecutableSearch, ProbeOptions, ResolveError,
    probe_adapter, resolve_executable,
};
pub use driver::{
    ACP_PROTOCOL_VERSION, AcpAuthoritativeCompletion, AcpDriver, AcpInitializeRequest,
    AcpInitializeResponse, AcpPermissionRequest, AcpPermissionResponse, AcpPromptChunk,
    AcpSessionRequest, AcpSessionResponse, AgentEvent, AgentEventKind, AgentFailure, DriverConfig,
    DriverError, DriverLimits, DriverMode, DriverOutcome, DriverStatus, FailureKind,
    McpConfigError, McpStdioServer, McpStdioSupport, PromptOutcome, ServerInfo,
    redact_sensitive_string,
};
pub use supervisor::{AgentSupervisor, QualificationStatus, SupervisorError};

pub mod session;
pub use session::{
    AcpSession, AdapterConfig, SessionControl, SessionProgress, copy_draft, source_revision,
};
