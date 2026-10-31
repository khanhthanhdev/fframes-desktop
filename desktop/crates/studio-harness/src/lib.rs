pub mod adapter_install;
pub mod detect;
pub mod discovery;
pub mod driver;
pub mod supervisor;

pub use detect::{
    Detected, DetectionEnv, DetectionSpec, NodeEntry, detect_agents, find_executable, node_entry,
};
pub use discovery::{
    AdapterConfig, AdapterLaunch, AdapterStatus, DiscoveryReport, ExecutableSearch, ProbeOptions,
    ResolveError, probe_adapter, probe_adapter_on, resolve_executable,
};
pub use driver::{
    AcpDriver, AgentDriver, AgentEvent, AgentEventKind, AgentFailure, AgentWire,
    DriverCapabilities, DriverConfig, DriverError, DriverLimits, DriverMode, DriverOutcome,
    DriverStatus, FailureKind, McpConfigError, McpHttpServer, McpStdioServer, McpStdioSupport,
    McpTransport, OwnershipSource, PromptOutcome, SteeringMode, UserMcpLaunch,
    redact_sensitive_string, start_driver,
};
pub use supervisor::{AgentSupervisor, SupervisorError};
