//! Görev dağıtıcı ve ajan orkestrasyon mantığı (Lounge Kernel).

pub mod decision_engine;
pub mod dispatcher;
pub mod guarded_command;
pub mod laya;
pub mod memory_prompt;
pub mod policy_gate;
pub mod policy_manager;
pub mod worker_registry;
pub mod workflow_engine;

pub use decision_engine::{
    apply_user_bias, DecisionGate, DecisionGatePhase, DecisionGateStatus, DecisionResult,
    InferMeter, LoungeTelemetry, UserBiasStats, DECISION_GATE_EVENT, USER_BIAS_APPROVE_THRESHOLD,
};
pub use dispatcher::{default_model_lock, Dispatcher};
pub use guarded_command::GuardedCommand;
pub use memory_prompt::{build_knowledge_whisper, inject_knowledge_hit, whisper_publish_subjects};
pub use policy_gate::{ActionSource, DestructiveClass, PolicyDecision, PolicyGate};
pub use policy_manager::{ALERT_SECURITY, PENDING_APPROVAL, TASK_CANCEL, TASK_RESUME};
pub use worker_registry::{
    route_subject_for, workers_status_json, RegisteredWorker, WorkerRegistration, WorkerRegistry,
    HEARTBEAT_TTL,
};
pub use workflow_engine::WorkflowEngine;
