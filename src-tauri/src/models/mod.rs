//! Domain modelleri ve mesaj şemaları.

mod discovery;
mod hf;
mod memory;
mod policy;
mod protocol;
mod quota;
mod service;

pub use discovery::{
    access_mode_for, format_host_labels, host_display_name, sqlite_tool_type, tool_id,
    ConnectedTool, DiscoveredTool, DiscoveryReport, DiscoverySource, SystemTool,
};
pub use hf::{DeviceProfile, HfModelOffer, PullProgress, RecommendedModels, MODEL_PULL_EVENT};
pub use memory::{
    merge_project_summaries, AstNode, CodeReference, DeadSymbol, IndexGraph, IndexSnapshot,
    ProjectList, ProjectSummary, SemanticMap, SemanticProject,
};
pub use policy::{
    decide_route, is_kernel, AgentTrigger, ApprovalKind, ApprovalRequest, QuotaExhaustedAction,
    RouteIntent, RoutingPolicy, RoutingVote, KERNEL_AGENT,
};
pub use protocol::{
    default_ollama_model, now_rfc3339, AnalysisDecision, CodeSnippet, ExperienceContext,
    ExperienceHit, ExperienceOutcome, ExperienceRecord, LoungeExperience, LoungeTask,
    SystemPromptAddon, TaskAssignment, TaskKind, TaskPriority, AGENT_PROMPT, ALERT_SECURITY,
    CONTEXT_WHISPER, DEFAULT_OLLAMA_MODEL, EXPERIENCE_REPORTED, INFRA_STATUS, TASK_ASSIGNED,
    TASK_COMPLETED, TASK_FAILED, TASK_REQUESTED, TASK_RESUME, TELEMETRY_DECISION, TEST_COMPLETED,
    TEST_REQUESTED,
};
pub use quota::{NatsUiEvent, QuotaState, ToolQuota, AMBER_THRESHOLD, QUOTA_EVENT};
pub use service::{ServiceHealth, ServiceId, ServiceReport, SERVICE_EVENT};
