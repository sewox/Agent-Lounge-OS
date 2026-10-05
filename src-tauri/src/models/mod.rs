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
    merge_project_summaries, AstNode, CodeReference, DeadSymbol, FileSymbol, IndexGraph,
    IndexSnapshot, ProjectList, ProjectPage, ProjectPageList, ProjectSummary, SemanticMap,
    SemanticProject, VaultProjectAggregate,
};
pub use policy::{
    decide_route, is_kernel, AgentTrigger, ApprovalKind, ApprovalRequest, QuotaExhaustedAction,
    RouteIntent, RoutingPolicy, RoutingVote, KERNEL_AGENT,
};
pub use protocol::{
    default_ollama_model, now_rfc3339, worker_tasks_subject, AgentSession, AnalysisDecision,
    CodeSnippet, ExperienceContext, ExperienceHit, ExperienceOutcome, ExperienceRecord,
    LoungeExperience, LoungeTask, SystemPromptAddon, TaskAssignment, TaskKind, TaskPriority,
    TaskStatus, AGENT_HEARTBEAT, AGENT_PROMPT, AGENT_STATUS, ALERT_QUOTA, ALERT_SECURITY,
    ARCHIVED_BY_TTL, ARCHIVED_BY_USER, CONTEXT_WHISPER, CONTROL_STOP, DEFAULT_MAX_HOPS,
    DEFAULT_OLLAMA_MODEL, EXPERIENCE_REPORTED, EXPERIENCE_STATUS_ACTIVE,
    EXPERIENCE_STATUS_ARCHIVED, INFRA_STATUS, TASKS_INBOX_PREFIX, TASK_ASSIGNED, TASK_COMPLETED,
    TASK_FAILED, TASK_REQUESTED, TASK_RESUME, TELEMETRY_DECISION, TEST_COMPLETED, TEST_REQUESTED,
    WORKERS_HEARTBEAT, WORKERS_REGISTER, WORKERS_UNREGISTER,
};
pub use quota::{
    NatsUiEvent, QuotaState, QuotaVerdict, ToolQuota, AMBER_THRESHOLD, LIMIT_POLICY_PERCENT,
    QUOTA_EVENT,
};
pub use service::{
    ServiceHealth, ServiceId, ServiceReport, AVAIL_NOT_INSTALLED, CODE_RESTART_EXHAUSTED,
    CODE_RESTART_RETRYING, SERVICE_EVENT,
};
