use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_OLLAMA_MODEL: &str = "llama3.1:8b";

pub const TASK_REQUESTED: &str = "lounge.task.requested";
pub const TASK_ASSIGNED: &str = "lounge.task.assigned";
pub const TASK_COMPLETED: &str = "lounge.task.completed";
pub const TASK_FAILED: &str = "lounge.task.failed";
pub const TASK_RESUME: &str = "lounge.task.resume";
pub const ALERT_SECURITY: &str = "lounge.alert.security";
pub const EXPERIENCE_REPORTED: &str = "lounge.experience.reported";
pub const AGENT_PROMPT: &str = "lounge.agent.prompt";
/// Cross-Project Memory "fısıltı" — `subjects.json` `agent.prompt` ile aynı NATS konusu.
pub const CONTEXT_WHISPER: &str = AGENT_PROMPT;
pub const TEST_REQUESTED: &str = "lounge.test.requested";
pub const TEST_COMPLETED: &str = "lounge.test.completed";
pub const TELEMETRY_DECISION: &str = "lounge.telemetry.decision";
pub const INFRA_STATUS: &str = "lounge.infra.status";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    #[default]
    General,
    CodeAnalysis,
    Review,
    Test,
    Orchestration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceOutcome {
    Success,
    Failure,
    Partial,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoungeTask {
    pub id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub source_agent: String,
    #[serde(default)]
    pub target_agent: Option<String>,
    pub project_id: String,
    pub summary: String,
    #[serde(default)]
    pub ast_refs: Vec<String>,
    #[serde(default)]
    pub priority: TaskPriority,
    pub created_at: String,
    #[serde(default)]
    pub kind: TaskKind,
    #[serde(default)]
    pub repo_path: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Zincirleme workflow: bu görevi tetikleyen tamamlanmış ebeveyn id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    /// Event Stream etiketi — örn. `Claude (Code) -> Grok (Test)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_chain: Option<String>,
}

impl LoungeTask {
    pub fn new(
        source_agent: impl Into<String>,
        project_id: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            msg_type: "task".into(),
            source_agent: source_agent.into(),
            target_agent: None,
            project_id: project_id.into(),
            summary: summary.into(),
            ast_refs: Vec::new(),
            priority: TaskPriority::Normal,
            created_at: now_rfc3339(),
            kind: TaskKind::General,
            repo_path: None,
            model: None,
            parent_task_id: None,
            workflow_chain: None,
        }
    }

    pub fn resolved_model<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(fallback)
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            TaskKind::General => "general",
            TaskKind::CodeAnalysis => "code_analysis",
            TaskKind::Review => "review",
            TaskKind::Test => "test",
            TaskKind::Orchestration => "orchestration",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoungeExperience {
    pub id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub agent: String,
    pub project_id: String,
    pub adr_summary: String,
    pub outcome: ExperienceOutcome,
    #[serde(default)]
    pub related_task_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: String,
}

impl LoungeExperience {
    pub fn from_task(
        task: &LoungeTask,
        adr_summary: impl Into<String>,
        outcome: ExperienceOutcome,
        tags: Vec<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            msg_type: "experience".into(),
            agent: task.source_agent.clone(),
            project_id: task.project_id.clone(),
            adr_summary: adr_summary.into(),
            outcome,
            related_task_id: Some(task.id.clone()),
            tags,
            created_at: now_rfc3339(),
        }
    }
}

/// SQLite `experiences` satırı: id, project_id, agent_id, topic, solution_summary, adr_record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExperienceRecord {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub topic: String,
    pub solution_summary: String,
    pub adr_record: String,
    pub outcome: ExperienceOutcome,
    #[serde(default)]
    pub related_task_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embedding: Vec<f32>,
}

impl ExperienceRecord {
    pub fn from_lounge(experience: &LoungeExperience, topic: impl Into<String>) -> Self {
        Self {
            id: experience.id.clone(),
            project_id: experience.project_id.clone(),
            agent_id: experience.agent.clone(),
            topic: topic.into(),
            solution_summary: experience.adr_summary.clone(),
            adr_record: experience.adr_summary.clone(),
            outcome: experience.outcome.clone(),
            related_task_id: experience.related_task_id.clone(),
            tags: experience.tags.clone(),
            created_at: experience.created_at.clone(),
            embedding: Vec::new(),
        }
    }

    pub fn from_task(
        task: &LoungeTask,
        solution_summary: impl Into<String>,
        adr_record: impl Into<String>,
        outcome: ExperienceOutcome,
        tags: Vec<String>,
    ) -> Self {
        let solution_summary = solution_summary.into();
        let adr_record = adr_record.into();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: task.project_id.clone(),
            agent_id: task.source_agent.clone(),
            topic: task.summary.clone(),
            solution_summary,
            adr_record,
            outcome,
            related_task_id: Some(task.id.clone()),
            tags,
            created_at: now_rfc3339(),
            embedding: Vec::new(),
        }
    }

    pub fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.topic, self.solution_summary, self.adr_record
        )
    }

    pub fn to_lounge(&self) -> LoungeExperience {
        LoungeExperience {
            id: self.id.clone(),
            msg_type: "experience".into(),
            agent: self.agent_id.clone(),
            project_id: self.project_id.clone(),
            adr_summary: if self.adr_record.trim().is_empty() {
                self.solution_summary.clone()
            } else {
                self.adr_record.clone()
            },
            outcome: self.outcome.clone(),
            related_task_id: self.related_task_id.clone(),
            tags: self.tags.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExperienceHit {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub topic: String,
    pub solution_summary: String,
    pub adr_record: String,
    pub score: f32,
    #[serde(default)]
    pub source: String,
}

impl ExperienceHit {
    pub fn source_or_sqlite(&self) -> &str {
        if self.source.is_empty() {
            "sqlite"
        } else {
            self.source.as_str()
        }
    }
}

/// AST özetinden kırpılmış kod parçası — tam dosya değil.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CodeSnippet {
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ExperienceContext {
    #[serde(default)]
    pub experiences: Vec<ExperienceHit>,
    #[serde(default)]
    pub snippets: Vec<CodeSnippet>,
    #[serde(default)]
    pub knowledge_hit: Option<f32>,
}

impl ExperienceContext {
    pub fn from_hits(hits: Vec<ExperienceHit>) -> Self {
        Self {
            experiences: hits,
            snippets: Vec::new(),
            knowledge_hit: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.experiences.is_empty() && self.snippets.is_empty()
    }

    pub fn prompt_block(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut block = String::from("\n\n## Cross-Project Memory\n");
        if let Some(hit) = self.knowledge_hit {
            block.push_str(&format!("KNOWLEDGE_HIT={hit:.2}\n"));
        }
        if !self.experiences.is_empty() {
            block.push_str("### Tecrübeler\n");
            for hit in &self.experiences {
                block.push_str(&format!(
                    "- [{}|{}|{}] {} (score={:.2})\n  solution: {}\n  adr: {}\n",
                    hit.agent_id,
                    hit.project_id,
                    hit.source_or_sqlite(),
                    hit.topic,
                    hit.score,
                    hit.solution_summary,
                    hit.adr_record
                ));
            }
        }
        if !self.snippets.is_empty() {
            block.push_str("### İlgili kod (AST özeti)\n");
            for snippet in &self.snippets {
                let loc = snippet
                    .line
                    .map(|line| format!("{}:{}", snippet.file, line))
                    .unwrap_or_else(|| snippet.file.clone());
                block.push_str(&format!(
                    "- {} `{}` {}\n",
                    snippet.kind, snippet.symbol, loc
                ));
                if !snippet.body.is_empty() {
                    block.push_str("```\n");
                    block.push_str(&snippet.body);
                    if !snippet.body.ends_with('\n') {
                        block.push('\n');
                    }
                    block.push_str("```\n");
                }
            }
        }
        block
    }
}

/// Çalışan ajana (Cursor/Claude) NATS üzerinden giden system prompt eklentisi.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemPromptAddon {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub task_id: String,
    pub target_agent: String,
    pub knowledge_hit: f32,
    pub context: ExperienceContext,
    pub prompt: String,
}

impl SystemPromptAddon {
    pub fn from_context(
        task_id: impl Into<String>,
        target_agent: impl Into<String>,
        knowledge_hit: f32,
        context: ExperienceContext,
    ) -> Self {
        let prompt = context.prompt_block();
        Self {
            msg_type: "system_prompt".into(),
            task_id: task_id.into(),
            target_agent: target_agent.into(),
            knowledge_hit,
            context,
            prompt,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskAssignment {
    pub task: LoungeTask,
    #[serde(default)]
    pub context: ExperienceContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AnalysisDecision {
    #[serde(default)]
    pub intent: String,
    #[serde(default)]
    pub is_code_analysis: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub adr_summary: String,
    #[serde(default)]
    pub outcome: Option<ExperienceOutcome>,
    #[serde(default)]
    pub target_agent: Option<String>,
    #[serde(default)]
    pub repo_path: Option<String>,
}

impl AnalysisDecision {
    pub fn needs_code_analysis(&self, task: &LoungeTask) -> bool {
        self.is_code_analysis
            || matches!(task.kind, TaskKind::CodeAnalysis)
            || self.intent.eq_ignore_ascii_case("code_analysis")
    }
}

pub fn default_ollama_model() -> String {
    std::env::var("LOUNGE_OLLAMA_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_string())
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_roundtrip_matches_shared_schema_shape() {
        let mut task = LoungeTask::new("cursor", "agent-lounge-os", "index kernel dispatcher");
        task.kind = TaskKind::CodeAnalysis;
        task.repo_path = Some("/tmp/repo".into());
        task.model = Some("llama3.1:8b".into());
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["type"], "task");
        assert_eq!(json["kind"], "code_analysis");
        let parsed: LoungeTask = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.kind, TaskKind::CodeAnalysis);
        assert_eq!(parsed.resolved_model("other"), "llama3.1:8b");
    }

    #[test]
    fn subjects_match_shared_file() {
        let raw = include_str!("../../../shared/lounge_protocol/subjects.json");
        let json: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(json["task"]["requested"], TASK_REQUESTED);
        assert_eq!(json["task"]["assigned"], TASK_ASSIGNED);
        assert_eq!(json["task"]["completed"], TASK_COMPLETED);
        assert_eq!(json["task"]["failed"], TASK_FAILED);
        assert_eq!(json["task"]["resume"], TASK_RESUME);
        assert_eq!(json["alert"]["security"], ALERT_SECURITY);
        assert_eq!(json["experience"]["reported"], EXPERIENCE_REPORTED);
        assert_eq!(json["agent"]["prompt"], AGENT_PROMPT);
        assert_eq!(json["context"]["whisper"], CONTEXT_WHISPER);
        assert_eq!(CONTEXT_WHISPER, AGENT_PROMPT);
        assert_eq!(json["test"]["requested"], TEST_REQUESTED);
        assert_eq!(json["test"]["completed"], TEST_COMPLETED);
        assert_eq!(json["telemetry"]["decision"], TELEMETRY_DECISION);
        assert_eq!(json["infra"]["status"], INFRA_STATUS);
        assert_eq!(json["bus"]["connected"], "lounge.bus.connected");
        assert_eq!(json["bus"]["heartbeat"], "lounge.bus.heartbeat");
    }

    #[test]
    fn prompt_block_is_system_addon() {
        let context = ExperienceContext {
            experiences: vec![ExperienceHit {
                id: "e1".into(),
                project_id: "other".into(),
                agent_id: "cursor".into(),
                topic: "NATS listen".into(),
                solution_summary: "spawn_blocking".into(),
                adr_record: "sync client".into(),
                score: 0.7,
                source: "sqlite".into(),
            }],
            snippets: vec![CodeSnippet {
                project_id: "lounge".into(),
                file: "src/dispatcher.rs".into(),
                line: Some(12),
                symbol: "listen_once".into(),
                kind: "fn".into(),
                body: "fn listen_once() {}".into(),
            }],
            knowledge_hit: Some(0.81),
        };
        let prompt = context.prompt_block();
        assert!(prompt.contains("Cross-Project Memory"));
        assert!(prompt.contains("KNOWLEDGE_HIT=0.81"));
        assert!(prompt.contains("listen_once"));
        let addon = SystemPromptAddon::from_context("task-1", "cursor", 0.81, context);
        assert_eq!(addon.msg_type, "system_prompt");
        assert_eq!(addon.target_agent, "cursor");
        assert!(addon.prompt.contains("spawn_blocking"));
    }
}
