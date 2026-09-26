/** Shared types for e2e Tauri IPC fixtures (mirrors app wire shapes). */

export type ServiceHealth = {
  id: string;
  name: string;
  running: boolean;
  started_by_us: boolean;
  endpoint: string;
  detail: string | null;
  error: string | null;
};

export type ServiceReport = {
  ollama: ServiceHealth;
  nats: ServiceHealth;
  memory: ServiceHealth;
  plugin: ServiceHealth;
};

export type LoungeExperience = {
  id: string;
  type: string;
  agent: string;
  project_id: string;
  adr_summary: string;
  outcome: "success" | "failure" | "partial";
  related_task_id?: string | null;
  tags: string[];
  created_at: string;
};

export type DeadSymbol = {
  name: string;
  kind: "unused" | "broken" | string;
  file?: string | null;
  line?: number | null;
  detail?: string | null;
  project_id?: string | null;
};

export type AstNode = {
  id: string;
  name: string;
  kind: string;
  file?: string | null;
  line?: number | null;
  ref_count: number;
};

export type CodeReference = {
  from_id: string;
  to_id: string;
  caller?: string;
  callee?: string;
  file?: string | null;
  line?: number | null;
};

export type SemanticProject = {
  name: string;
  repo_path: string;
  files: number;
  node_count: number;
  edge_count: number;
  nodes: AstNode[];
  references: CodeReference[];
  dead: DeadSymbol[];
};

export type SemanticMap = {
  projects: SemanticProject[];
};

export type ProjectSummary = {
  name: string;
  root_path?: string | null;
  nodes: number;
  edges: number;
  files?: number | null;
};

export type ToolQuota = {
  id: string;
  tool: string;
  kind: string;
  unit: string;
  used: string;
  remaining: string;
  reset: string;
  percent: number | null;
  tone: string;
  label: string;
  access_mode?: string;
  host_id?: string | null;
};

export type QuotaState = {
  scanned_at: string;
  amber_alert: boolean;
  amber_tools: string[];
  quotas: ToolQuota[];
};

export type RoutingPolicy = {
  require_user_approval: boolean;
  on_quota_exhausted: string;
  local_fallback_agent: string;
  local_fallback_model: string;
  triggers: { agent_id: string; label: string; when: string; enabled: boolean }[];
};

export type DecisionGateStatus = {
  phase: string;
  title: string;
  message: string;
  detail: string | null;
  device: string | null;
  reason: string | null;
};

export type LayaEngineStatus = {
  phase: string;
  label: string;
  message: string;
  file: string | null;
  completed: number;
  total: number;
  percentage: number;
  path: string;
  error: string | null;
};

export type IndexSnapshot = {
  project: string;
  status?: string | null;
  nodes: number;
  edges: number;
  files?: number | null;
  dead?: number;
};

export type ApprovalRequest = {
  task_id: string;
  summary: string;
  from_agent: string;
  to_agent: string;
  kind: string;
  reason: string;
  expires_at?: string | null;
  timeout_secs?: number | null;
};

export type DiscoveryReport = {
  scanned_at: string;
  sources: { id: string; available: boolean; origin_path?: string | null; detail?: string | null }[];
  apps?: unknown[];
  models?: unknown[];
  mcp_servers?: unknown[];
  tools?: unknown[];
};

export type FixtureDataset = {
  name: "full" | "empty";
  experiences: LoungeExperience[];
  deadSymbols: DeadSymbol[];
  semanticMap: SemanticMap;
  projects: ProjectSummary[];
  quotas: ToolQuota[];
  quotaState: QuotaState;
  events: {
    id: string;
    time: string;
    subject: string;
    from: string;
    to: string;
    payload: string;
    state: "queued" | "ok" | "error" | "retry";
  }[];
  serviceReport: ServiceReport;
  models: string[];
  kernelModel: string;
  policy: RoutingPolicy;
  decisionGate: DecisionGateStatus;
  layaEngine: LayaEngineStatus;
  pendingApprovals: ApprovalRequest[];
  discovery: DiscoveryReport;
  connectedTools: { id: string; name: string; kind: string; source: string; selected?: boolean }[];
  graphUiPort: number;
  graphUiStatus: { enabled: boolean; running: boolean; url: string | null; detail: string | null };
  workers: { id: string; name: string; kind: string; is_active: boolean; enabled: boolean; model?: string }[];
};
