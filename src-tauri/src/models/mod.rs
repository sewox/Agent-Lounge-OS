//! Domain modelleri ve mesaj şemaları.

mod discovery;
mod memory;
mod policy;
mod protocol;
mod quota;
mod service;

pub use discovery::{
    sqlite_tool_type, tool_id, ConnectedTool, DiscoveredTool, DiscoveryReport, DiscoverySource,
    SystemTool,
};
pub use memory::{IndexSnapshot, ProjectList, ProjectSummary};
pub use policy::{
    decide_route, is_kernel, AgentTrigger, ApprovalKind, ApprovalRequest, QuotaExhaustedAction,
    RouteIntent, RoutingPolicy, RoutingVote, KERNEL_AGENT,
};
pub use protocol::{
    default_ollama_model, now_rfc3339, AnalysisDecision, ExperienceContext, ExperienceHit,
    ExperienceOutcome, ExperienceRecord, LoungeExperience, LoungeTask, TaskAssignment, TaskKind,
    TaskPriority, DEFAULT_OLLAMA_MODEL, EXPERIENCE_REPORTED, TASK_ASSIGNED, TASK_COMPLETED,
    TASK_FAILED, TASK_REQUESTED,
};
pub use quota::{NatsUiEvent, ToolQuota};
pub use service::{ServiceHealth, ServiceId, ServiceReport};
