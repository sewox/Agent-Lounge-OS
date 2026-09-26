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

/** Çekirdek daemon'lar (Lounge LMR + NATS) ayakta değilse degraded. */
export function coreServicesDegraded(report: ServiceReport | null | undefined): boolean {
  if (!report) {
    return false;
  }
  return !report.ollama.running || !report.nats.running;
}

/** UI banner: hangi servis(ler) down — LMR adı host Ollama değil, Lounge runtime. */
export function degradedCoreServiceNames(report: ServiceReport | null | undefined): string[] {
  if (!report) {
    return [];
  }
  const names: string[] = [];
  if (!report.ollama.running) {
    names.push("LMR");
  }
  if (!report.nats.running) {
    names.push("NATS");
  }
  return names;
}

export type DecisionGatePhase = "loading" | "ready" | "failed" | "available";

export type DecisionGateStatus = {
  phase: DecisionGatePhase;
  title: string;
  message: string;
  detail: string | null;
  device: string | null;
  reason: string | null;
};

export type LayaEnginePhase = "downloading" | "ready" | "failed";

export type LayaEngineStatus = {
  phase: LayaEnginePhase;
  label: string;
  message: string;
  file: string | null;
  completed: number;
  total: number;
  percentage: number;
  path: string;
  error: string | null;
};

export type ExperienceOutcome = "success" | "failure" | "partial";

export type LoungeExperience = {
  id: string;
  type: string;
  agent: string;
  project_id: string;
  adr_summary: string;
  outcome: ExperienceOutcome;
  related_task_id?: string | null;
  tags: string[];
  created_at: string;
};

export type NatsEvent = {
  id: string;
  time: string;
  subject: string;
  from: string;
  to: string;
  payload: string;
  state: "queued" | "ok" | "error" | "retry";
  decisionLabel?: string;
  /** Multi-agent zinciri — örn. `Task A -> Triggered Task B`. */
  chainLabel?: string;
};

export type NatsTone = "task" | "success" | "error";

export type LoungeMessage = {
  id: string;
  type: string;
  subject: string;
  source_agent: string;
  target_agent?: string | null;
  created_at: string;
  payload: unknown;
  payload_bytes: number;
};

export type DiscoverySource = {
  id: string;
  available: boolean;
  origin_path?: string | null;
  detail?: string | null;
};

export type AccessMode = "subscription" | "api" | "plugin" | "local";

export const HOST_LABEL: Record<string, string> = {
  claude_desktop: "Claude Desktop",
  claude_cli: "Claude CLI",
  cursor: "Cursor",
  grok_bot: "Grok Bot",
  antigravity: "Antigravity",
};

export function hostLabelsFor(tool: {
  source: string;
  host_id?: string | null;
  host_ids?: string[] | null;
}): string {
  const ids =
    tool.host_ids && tool.host_ids.length > 0
      ? tool.host_ids
      : [tool.host_id || tool.source];
  return ids.map((id) => HOST_LABEL[id] ?? id).join(" · ");
}

export type DiscoveredTool = {
  id: string;
  name: string;
  kind: "app" | "cli" | "plugin" | "model" | "mcp" | "system" | string;
  source: "claude_desktop" | "claude_cli" | "cursor" | "grok_bot" | "antigravity" | "lmr" | "ollama" | "system" | "api" | string;
  origin_path?: string | null;
  command?: string | null;
  args: string[];
  endpoint?: string | null;
  detail?: string | null;
  available: boolean;
  access_mode?: AccessMode | string;
  host_id?: string | null;
  host_ids?: string[];
};

export type SystemTool = {
  id: string;
  name: string;
  available: boolean;
  path?: string | null;
  detail?: string | null;
};

export type DiscoveryReport = {
  scanned_at: string;
  sources: DiscoverySource[];
  tools: DiscoveredTool[];
  apps?: DiscoveredTool[];
  models: DiscoveredTool[];
  mcp_servers: DiscoveredTool[];
  system_tools: SystemTool[];
};

export type DeviceProfile = {
  total_ram_gb: number;
  available_ram_gb: number;
  usable_budget_gb: number;
  arch: string;
  apple_silicon: boolean;
  metal: boolean;
  summary: string;
  recommended_upper: string;
};

export type HfModelOffer = {
  id: string;
  hf_id: string;
  pull_name: string;
  name: string;
  family: string;
  params: string;
  estimated_ram_gb: number;
  min_ram_gb: number;
  downloads: number;
  recommended: boolean;
  installed: boolean;
  heavy: boolean;
  disabled_reason: string | null;
};

export type RecommendedModels = {
  device: DeviceProfile;
  offers: HfModelOffer[];
};

export type PullProgress = {
  hf_id: string;
  pull_name: string;
  status: string;
  digest?: string | null;
  completed: number;
  total: number;
  done: boolean;
  error?: string | null;
};

export function hfOfferToTool(
  offer: HfModelOffer,
  modelName: string,
  endpoint = "http://127.0.0.1:18790",
): DiscoveredTool {
  return {
    id: `lmr:${modelName}`,
    name: modelName,
    kind: "model",
    source: "lmr",
    origin_path: `${endpoint}/api/tags`,
    command: null,
    args: [],
    endpoint,
    detail: offer.name,
    available: true,
    access_mode: "local",
    host_id: null,
    host_ids: [],
  };
}

export function formatDiscoveryLocation(raw?: string | null): string {
  if (!raw?.trim()) {
    return "—";
  }
  const parts = raw
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
  if (parts.length === 0) {
    return "—";
  }
  const first = formatOneLocation(parts[0]);
  return parts.length === 1 ? first : `${first} +${parts.length - 1}`;
}

function formatOneLocation(raw: string): string {
  if (/^https?:\/\//i.test(raw)) {
    try {
      const url = new URL(raw);
      return url.port ? `${url.hostname}:${url.port}` : url.host;
    } catch {
      return raw;
    }
  }
  return raw
    .replace(/^\/Users\/[^/]+/, "~")
    .replace(/^\/home\/[^/]+/, "~")
    .replace(/^\\Users\\[^\\]+/i, "~");
}

export function humanizeDiscoveryDetail(raw?: string | null): string | null {
  if (!raw?.trim()) {
    return null;
  }
  const text = raw.trim();
  const lower = text.toLowerCase();
  if (
    lower.includes("error sending") ||
    lower.includes("error trying") ||
    lower.includes("connection refused") ||
    lower.includes("timed out") ||
    lower.includes("connect error") ||
    lower.includes("tcp connect")
  ) {
    return "yanıt yok";
  }
  if ((text.includes("/") || text.includes("\\")) && /[,;]/.test(text)) {
    const locations = text
      .split(/[,;]/)
      .map((part) => part.trim())
      .filter((part) => part.includes("/") || part.includes("\\"));
    const note = /mcpServers yok/i.test(text) ? "mcpServers yok" : null;
    if (locations.length > 1) {
      return note ? `${locations.length} konum · ${note}` : `${locations.length} konum`;
    }
  }
  return text;
}

export type ConnectedTool = {
  id: string;
  name: string;
  kind: string;
  source: string;
  origin_path?: string | null;
  command?: string | null;
  args: string[];
  endpoint?: string | null;
  enabled: boolean;
  connected_at: string;
  payload: unknown;
  type: "mcp" | "model" | "cli" | string;
  config_path?: string | null;
  is_active: boolean;
  last_synced: string;
};

function mockTool(
  tool: Omit<DiscoveredTool, "args" | "available"> & Partial<Pick<DiscoveredTool, "args" | "available">>,
): DiscoveredTool {
  return {
    args: [],
    available: true,
    command: null,
    endpoint: null,
    detail: null,
    origin_path: null,
    access_mode: "local",
    host_id: null,
    host_ids: [],
    ...tool,
  };
}

export const MOCK_DISCOVERY: DiscoveryReport = {
  scanned_at: new Date().toISOString(),
  sources: [
    { id: "claude_desktop", available: true, origin_path: "/Applications/Claude.app", detail: "uygulama kurulu" },
    { id: "claude_cli", available: true, origin_path: "~/.claude", detail: "abonelik · kurulu" },
    { id: "cursor", available: true, origin_path: "/Applications/Cursor.app", detail: "uygulama kurulu" },
    { id: "antigravity", available: true, origin_path: "/Applications/Antigravity.app", detail: "uygulama kurulu" },
    { id: "grok_bot", available: true, origin_path: "/Applications/Grok.app", detail: "abonelik · kurulu" },
    { id: "lmr", available: true, origin_path: "http://127.0.0.1:18790/api/tags", detail: "1 model" },
    { id: "ollama", available: true, origin_path: "http://127.0.0.1:11434/api/tags", detail: "2 model" },
    { id: "system", available: true, origin_path: null, detail: "2 / 3 PATH" },
  ],
  apps: [
    mockTool({ id: "app:claude_desktop", name: "Claude Desktop", kind: "app", source: "claude_desktop", origin_path: "/Applications/Claude.app", detail: "abonelik · kurulu", access_mode: "subscription", host_id: "claude_desktop" }),
    mockTool({ id: "app:claude_cli", name: "Claude CLI", kind: "cli", source: "claude_cli", origin_path: "~/.claude", detail: "abonelik · kurulu", access_mode: "subscription", host_id: "claude_cli" }),
    mockTool({ id: "app:cursor", name: "Cursor", kind: "app", source: "cursor", origin_path: "/Applications/Cursor.app", detail: "abonelik · çalışıyor", access_mode: "subscription", host_id: "cursor", host_ids: ["cursor"] }),
    mockTool({ id: "app:antigravity", name: "Antigravity", kind: "app", source: "antigravity", origin_path: "/Applications/Antigravity.app", detail: "abonelik · kurulu", access_mode: "subscription", host_id: "antigravity", host_ids: ["antigravity"] }),
    mockTool({ id: "app:grok_bot", name: "Grok Bot", kind: "app", source: "grok_bot", origin_path: "/Applications/Grok.app", detail: "abonelik · kurulu", access_mode: "subscription", host_id: "grok_bot", host_ids: ["grok_bot"] }),
  ],
  models: [
    mockTool({ id: "lmr:llama3.1:8b", name: "llama3.1:8b", kind: "model", source: "lmr", origin_path: "http://127.0.0.1:18790/api/tags", endpoint: "http://127.0.0.1:18790", detail: "Lounge Model Runner", access_mode: "local" }),
    mockTool({ id: "ollama:llama3.1:8b", name: "llama3.1:8b", kind: "model", source: "ollama", origin_path: "http://127.0.0.1:11434/api/tags", endpoint: "http://127.0.0.1:11434", detail: "Ollama Sunucusu", access_mode: "local" }),
    mockTool({ id: "ollama:qwen2.5:7b", name: "qwen2.5:7b", kind: "model", source: "ollama", origin_path: "http://127.0.0.1:11434/api/tags", endpoint: "http://127.0.0.1:11434", detail: "Ollama Sunucusu", access_mode: "local" }),
  ],
  mcp_servers: [
    mockTool({ id: "claude_desktop:github", name: "github", kind: "plugin", source: "claude_desktop", origin_path: "~/Library/Application Support/Claude/claude_desktop_config.json", command: "npx", args: ["-y", "@modelcontextprotocol/server-github"], detail: "env keys: GITHUB_TOKEN", access_mode: "plugin", host_id: "claude_desktop" }),
    mockTool({ id: "plugin:notion", name: "notion", kind: "plugin", source: "cursor", origin_path: "~/.cursor/mcp.json", command: "npx", args: ["-y", "@notionhq/mcp"], detail: "Cursor · Antigravity", access_mode: "plugin", host_id: "cursor", host_ids: ["cursor", "antigravity"] }),
    mockTool({ id: "plugin:codebase-memory", name: "codebase-memory", kind: "plugin", source: "cursor", origin_path: ".cursor/mcp.json", command: "codebase-memory-mcp", detail: "Cursor · Antigravity", access_mode: "plugin", host_id: "cursor", host_ids: ["cursor", "antigravity"] }),
  ],
  system_tools: [
    { id: "system:git", name: "git", available: true, path: "/usr/bin/git", detail: null },
    { id: "system:gh", name: "gh", available: true, path: "/opt/homebrew/bin/gh", detail: null },
    { id: "system:docker", name: "docker", available: false, path: null, detail: "PATH'te bulunamadı" },
  ],
  tools: [],
};

MOCK_DISCOVERY.tools = [
  ...(MOCK_DISCOVERY.apps ?? []),
  ...MOCK_DISCOVERY.models,
  ...MOCK_DISCOVERY.mcp_servers,
  mockTool({ id: "system:git", name: "git", kind: "system", source: "system", origin_path: "/usr/bin/git", command: "/usr/bin/git", access_mode: "local" }),
  mockTool({ id: "system:gh", name: "gh", kind: "system", source: "system", origin_path: "/opt/homebrew/bin/gh", command: "/opt/homebrew/bin/gh", access_mode: "local" }),
  mockTool({ id: "system:docker", name: "docker", kind: "system", source: "system", command: "docker", detail: "PATH'te bulunamadı", available: false, access_mode: "local" }),
];

export const MOCK_DEVICE_PROFILE: DeviceProfile = {
  total_ram_gb: 16,
  available_ram_gb: 10,
  usable_budget_gb: 7.2,
  arch: "aarch64",
  apple_silicon: true,
  metal: true,
  summary: "16 GB birleşik bellek · Apple Silicon · önerilen üst sınır 8B Q4",
  recommended_upper: "8B Q4",
};

export const MOCK_HF_OFFERS: HfModelOffer[] = [
  {
    id: "bartowski/Llama-3.2-1B-Instruct-GGUF",
    hf_id: "bartowski/Llama-3.2-1B-Instruct-GGUF",
    pull_name: "hf.co/bartowski/Llama-3.2-1B-Instruct-GGUF",
    name: "Llama 3.2 1B Instruct",
    family: "Llama",
    params: "1B",
    estimated_ram_gb: 1.2,
    min_ram_gb: 4,
    downloads: 184000,
    recommended: true,
    installed: false,
    heavy: false,
    disabled_reason: null,
  },
  {
    id: "bartowski/Qwen2.5-3B-Instruct-GGUF",
    hf_id: "bartowski/Qwen2.5-3B-Instruct-GGUF",
    pull_name: "hf.co/bartowski/Qwen2.5-3B-Instruct-GGUF",
    name: "Qwen2.5 3B Instruct",
    family: "Qwen",
    params: "3B",
    estimated_ram_gb: 2.6,
    min_ram_gb: 6,
    downloads: 92000,
    recommended: true,
    installed: false,
    heavy: false,
    disabled_reason: null,
  },
  {
    id: "bartowski/Qwen2.5-14B-Instruct-GGUF",
    hf_id: "bartowski/Qwen2.5-14B-Instruct-GGUF",
    pull_name: "hf.co/bartowski/Qwen2.5-14B-Instruct-GGUF",
    name: "Qwen2.5 14B Instruct",
    family: "Qwen",
    params: "14B",
    estimated_ram_gb: 9,
    min_ram_gb: 16,
    downloads: 41000,
    recommended: false,
    installed: false,
    heavy: true,
    disabled_reason: "cihaz için ağır (tahmini 9.0 GB > bütçe 7.2 GB)",
  },
];

export const MOCK_RECOMMENDED_MODELS: RecommendedModels = {
  device: MOCK_DEVICE_PROFILE,
  offers: MOCK_HF_OFFERS,
};

export const BUS_UI_EVENT = "nats-event";
export const QUOTA_UI_EVENT = "quota-update";
export const SERVICE_UI_EVENT = "service-status";
export const MODEL_PULL_EVENT = "model-pull";
export const DECISION_GATE_EVENT = "decision-gate";
export const LAYA_ENGINE_EVENT = "laya-engine";
export const INFRA_STATUS = "lounge.infra.status";
export const TELEMETRY_DECISION = "lounge.telemetry.decision";
export const ALERT_SECURITY = "lounge.alert.security";
export const ALERT_QUOTA = "lounge.alert.quota";
export const TASK_RESUME = "lounge.task.resume";
export const CONTEXT_WHISPER = "lounge.context.whisper";
export const AGENT_PROMPT = "lounge.agent.prompt";
export const SECURITY_OVERLAY_PROMPT =
  "Ajan kritik bir dosyaya erişmek istiyor. Onaylıyor musunuz?";
export const QUOTA_ALERT_PROMPT =
  "Kota limiti aşıldı. Görev durduruldu. Yerel LMR ile devam edebilirsiniz.";
export const QUOTA_CONTINUE_LOCAL_LABEL = "Yerel Model (Ollama) ile devam et";
export const LATENCY_SPARK_CAP = 24;
export const MSG_MIN_WINDOW_MS = 60_000;
export const PAGE_SIZE = 25;
export const AMBER_THRESHOLD = 80;
export const LIMIT_POLICY_PERCENT = 90;

/** NATS `lounge.context.whisper` — canlı Cross-Project Memory fısıltısı. */
export type ContextWhisper = {
  taskId: string;
  targetAgent: string;
  knowledgeHit: number;
  experienceIds: string[];
  experiences: LoungeExperience[];
  prompt: string;
};

export function parseContextWhisper(message: LoungeMessage): ContextWhisper | null {
  if (message.subject !== CONTEXT_WHISPER) {
    return null;
  }
  if (!message.payload || typeof message.payload !== "object" || Array.isArray(message.payload)) {
    return null;
  }
  const payload = message.payload as Record<string, unknown>;
  const msgType = typeof payload.type === "string" ? payload.type : "";
  if (msgType && msgType !== "system_prompt") {
    return null;
  }
  const context =
    payload.context && typeof payload.context === "object" && !Array.isArray(payload.context)
      ? (payload.context as Record<string, unknown>)
      : null;
  const rawHits = Array.isArray(context?.experiences)
    ? context.experiences
    : Array.isArray(payload.experiences)
      ? payload.experiences
      : [];
  const experiences: LoungeExperience[] = [];
  const experienceIds: string[] = [];
  for (const row of rawHits) {
    if (!row || typeof row !== "object" || Array.isArray(row)) {
      continue;
    }
    const hit = row as Record<string, unknown>;
    const id = typeof hit.id === "string" ? hit.id.trim() : "";
    if (!id) {
      continue;
    }
    experienceIds.push(id);
    const adr =
      (typeof hit.adr_record === "string" && hit.adr_record) ||
      (typeof hit.solution_summary === "string" && hit.solution_summary) ||
      (typeof hit.topic === "string" && hit.topic) ||
      "";
    experiences.push({
      id,
      type: "experience",
      agent: typeof hit.agent_id === "string" ? hit.agent_id : "kernel",
      project_id: typeof hit.project_id === "string" ? hit.project_id : "",
      adr_summary: adr,
      outcome: "success",
      related_task_id: typeof payload.task_id === "string" ? payload.task_id : null,
      tags: ["whisper", "cross-project"],
      created_at: message.created_at || new Date().toISOString(),
    });
  }
  if (experienceIds.length === 0 && typeof payload.experience_id === "string") {
    const id = payload.experience_id.trim();
    if (id) {
      experienceIds.push(id);
    }
  }
  return {
    taskId: typeof payload.task_id === "string" ? payload.task_id : message.id,
    targetAgent: typeof payload.target_agent === "string" ? payload.target_agent : "",
    knowledgeHit: asFiniteNumber(payload.knowledge_hit) ?? 0,
    experienceIds,
    experiences,
    prompt: typeof payload.prompt === "string" ? payload.prompt : "",
  };
}

export function mergeWhisperedExperiences(
  current: LoungeExperience[],
  whisper: ContextWhisper,
): LoungeExperience[] {
  if (whisper.experiences.length === 0) {
    return current;
  }
  const byId = new Map(current.map((row) => [row.id, row]));
  for (const row of whisper.experiences) {
    if (!byId.has(row.id)) {
      byId.set(row.id, row);
    }
  }
  const merged = Array.from(byId.values());
  const whispered = new Set(whisper.experienceIds);
  merged.sort((left, right) => {
    const leftLive = whispered.has(left.id) ? 1 : 0;
    const rightLive = whispered.has(right.id) ? 1 : 0;
    if (leftLive !== rightLive) {
      return rightLive - leftLive;
    }
    return right.created_at.localeCompare(left.created_at);
  });
  return merged;
}

export type LoungeTelemetry = {
  kind: string;
  message_id: string;
  subject: string;
  latency_us: number;
  latency_ms: number;
  routing?: string;
  security?: string;
  knowledge_hit?: number;
  context_match?: number;
  device?: string;
  timestamp?: string;
  msg_per_min?: number;
};

export type MsgTick = {
  id: string;
  at: number;
};

export function isDecisionTelemetrySubject(subject: string): boolean {
  return subject === TELEMETRY_DECISION;
}

function asFiniteNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  return null;
}

export function parseLayaEngineStatus(message: LoungeMessage): LayaEngineStatus | null {
  if (message.subject !== INFRA_STATUS) {
    return null;
  }
  if (!message.payload || typeof message.payload !== "object" || Array.isArray(message.payload)) {
    return null;
  }
  const payload = message.payload as Record<string, unknown>;
  const phase = payload.phase;
  if (phase !== "downloading" && phase !== "ready" && phase !== "failed") {
    return null;
  }
  if (
    typeof payload.label !== "string" ||
    typeof payload.message !== "string" ||
    typeof payload.path !== "string"
  ) {
    return null;
  }
  const completed = asFiniteNumber(payload.completed) ?? 0;
  const total = asFiniteNumber(payload.total) ?? 0;
  const percentage =
    asFiniteNumber(payload.percentage) ??
    (phase === "ready" ? 100 : total > 0 ? Math.min(100, Math.round((completed / total) * 100)) : 0);
  return {
    phase,
    label: payload.label,
    message: payload.message,
    file: typeof payload.file === "string" ? payload.file : null,
    completed,
    total,
    percentage: Math.min(100, Math.max(0, Math.round(percentage))),
    path: payload.path,
    error: typeof payload.error === "string" ? payload.error : null,
  };
}

export function layaEnginePercentage(status: LayaEngineStatus | null | undefined): number | null {
  if (!status) {
    return null;
  }
  if (status.phase === "ready") {
    return 100;
  }
  if (typeof status.percentage === "number" && Number.isFinite(status.percentage) && status.total > 0) {
    return Math.min(100, Math.max(0, Math.round(status.percentage)));
  }
  if (status.total > 0) {
    return Math.min(100, Math.round((status.completed / status.total) * 100));
  }
  return null;
}

export function formatLayaEngineFleetStatus(status: LayaEngineStatus | null | undefined): string {
  if (!status) {
    return "Laya Engine: …";
  }
  if (status.phase === "downloading") {
    const pct = layaEnginePercentage(status);
    return pct != null ? `Laya Engine: Downloading ${pct}%` : "Laya Engine: Downloading";
  }
  if (status.label) {
    return status.label;
  }
  if (status.phase === "failed") {
    return "Laya Engine: Failed";
  }
  if (status.phase === "ready") {
    return "Laya Engine: Ready";
  }
  return "Laya Engine: …";
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  return value as Record<string, unknown>;
}

export function extractDecisionLatencyMs(payload: unknown): number | null {
  const record = asRecord(payload);
  if (!record) {
    return null;
  }
  const nested = asRecord(record.decision);
  const latencyUs =
    asFiniteNumber(record.latency_us) ?? (nested ? asFiniteNumber(nested.elapsed_us) : null);
  const latencyMsRaw =
    asFiniteNumber(record.latency_ms) ?? (nested ? asFiniteNumber(nested.elapsed_ms) : null);
  const latencyMs = latencyMsRaw ?? (latencyUs != null ? latencyUs / 1000 : null);
  return latencyMs != null && Number.isFinite(latencyMs) ? latencyMs : null;
}

export function parseDecisionTelemetry(message: LoungeMessage): LoungeTelemetry | null {
  if (!isDecisionTelemetrySubject(message.subject)) {
    return null;
  }
  const payload = asRecord(message.payload);
  if (!payload) {
    return null;
  }
  const nested = asRecord(payload.decision);
  const latencyUs =
    asFiniteNumber(payload.latency_us) ?? (nested ? asFiniteNumber(nested.elapsed_us) : null);
  const latencyMs = extractDecisionLatencyMs(payload);
  if (latencyUs == null && latencyMs == null) {
    return null;
  }
  const resolvedUs = latencyUs ?? Math.round((latencyMs ?? 0) * 1000);
  const resolvedMs = latencyMs ?? resolvedUs / 1000;
  const nestedHit = nested ? asFiniteNumber(nested.knowledge_hit) : null;
  const nestedMatch = nested ? asFiniteNumber(nested.context_match) : null;
  const hit =
    asFiniteNumber(payload.knowledge_hit) ??
    asFiniteNumber(payload.context_match) ??
    nestedHit ??
    nestedMatch;
  return {
    kind: typeof payload.kind === "string" ? payload.kind : "decision",
    message_id:
      typeof payload.message_id === "string" && payload.message_id
        ? payload.message_id
        : message.id,
    subject: typeof payload.subject === "string" ? payload.subject : message.subject,
    latency_us: resolvedUs,
    latency_ms: resolvedMs,
    routing: typeof payload.routing === "string" ? payload.routing : undefined,
    security: typeof payload.security === "string" ? payload.security : undefined,
    knowledge_hit: hit ?? undefined,
    context_match:
      asFiniteNumber(payload.context_match) ?? nestedMatch ?? hit ?? undefined,
    device: typeof payload.device === "string" ? payload.device : undefined,
    timestamp:
      typeof payload.timestamp === "string" && payload.timestamp
        ? payload.timestamp
        : message.created_at,
    msg_per_min: asFiniteNumber(payload.msg_per_min) ?? undefined,
  };
}

export function formatLatencyMs(ms: number): string {
  if (!Number.isFinite(ms)) {
    return "—";
  }
  return `${ms.toFixed(1)}ms`;
}

export function formatLayaDecision(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) {
    return "Laya Decision: —";
  }
  return `Laya Decision: ${formatLatencyMs(ms)}`;
}

export function formatDecisionStreamLabel(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) {
    return "Decision: —";
  }
  return `Decision: ${Math.round(ms)}ms`;
}

/** Payload / zarftan workflow zincir etiketini çıkarır. */
export function extractWorkflowChainLabel(payload: unknown): string | undefined {
  const record = asRecord(payload);
  if (!record) {
    return undefined;
  }
  const direct =
    (typeof record.workflow_chain === "string" && record.workflow_chain.trim()) ||
    (typeof record.chain_label === "string" && record.chain_label.trim()) ||
    "";
  if (direct) {
    return direct;
  }
  const nested = asRecord(record.task);
  if (nested && typeof nested.workflow_chain === "string" && nested.workflow_chain.trim()) {
    return nested.workflow_chain.trim();
  }
  return undefined;
}

/**
 * Görev zinciri — örn. `implement feature -> Triggered Auto-Test after Code: implement feature`.
 * Rust `format_workflow_chain` ile aynı sözleşme.
 */
export function formatWorkflowChainLabel(parentLabel: string, childLabel: string): string {
  const parent = parentLabel.trim() || "Task A";
  const child = childLabel.trim() || "Task B";
  return `${parent} -> Triggered ${child}`;
}

export function eventDecisionLabel(
  event: Pick<NatsEvent, "decisionLabel">,
  latestMs?: number | null,
): string {
  if (event.decisionLabel) {
    return event.decisionLabel;
  }
  return formatDecisionStreamLabel(latestMs);
}

export function pruneMsgWindow(ticks: MsgTick[], now = Date.now()): MsgTick[] {
  const cutoff = now - MSG_MIN_WINDOW_MS;
  const seen = new Set<string>();
  const next: MsgTick[] = [];
  for (const tick of ticks) {
    if (tick.at < cutoff || seen.has(tick.id)) {
      continue;
    }
    seen.add(tick.id);
    next.push(tick);
  }
  return next;
}

export function recordMsgTick(ticks: MsgTick[], id: string, now = Date.now()): MsgTick[] {
  const pruned = pruneMsgWindow(ticks, now);
  if (!id || pruned.some((tick) => tick.id === id)) {
    return pruned;
  }
  return [...pruned, { id, at: now }];
}

export function pageCount(total: number, pageSize = PAGE_SIZE): number {
  return Math.max(1, Math.ceil(Math.max(0, total) / pageSize));
}

export function pageSlice<T>(items: T[], page: number, pageSize = PAGE_SIZE): T[] {
  const pages = pageCount(items.length, pageSize);
  const safe = Math.min(Math.max(0, page), pages - 1);
  const start = safe * pageSize;
  return items.slice(start, start + pageSize);
}

/** Semantic map düğümü seçimi — experience filtresi için. */
export type SemanticMapSelection = {
  id: string;
  name: string;
  kind: string;
  file?: string | null;
  project?: string | null;
};

export function pathBasename(path: string | null | undefined): string | null {
  if (!path?.trim()) {
    return null;
  }
  return path.split(/[/\\]/).filter(Boolean).at(-1) ?? null;
}

/** Canlı dead sayımı: deadSymbols → map.dead → lastIndex.dead; indeks yoksa 0 (KPI "—" UI'da). */
export function resolveDeadSymbolCount(input: {
  deadSymbols: DeadSymbol[];
  semanticMap: SemanticMap;
  lastIndex: IndexSnapshot | null;
  projects: ProjectSummary[];
}): number {
  const hasIndex =
    Boolean(input.lastIndex) ||
    input.projects.length > 0 ||
    input.semanticMap.projects.length > 0;
  if (!hasIndex) {
    return 0;
  }
  const fromList = input.deadSymbols.length;
  const fromMap = input.semanticMap.projects.reduce((sum, row) => sum + row.dead.length, 0);
  return fromList || fromMap || input.lastIndex?.dead || 0;
}

/** True when there is no indexed project / map data to show. */
export function hasIndexedWorkspace(input: {
  lastIndex: IndexSnapshot | null;
  projects: ProjectSummary[];
  semanticMap: SemanticMap;
}): boolean {
  return (
    Boolean(input.lastIndex) ||
    input.projects.length > 0 ||
    input.semanticMap.projects.length > 0
  );
}

/**
 * K9 UI: Claude Desktop + Claude CLI are one subscription account.
 * Compatible with PR-1 merged backend rows (single card) and legacy dual rows.
 */
export function mergeClaudeQuotaRows(quotas: ToolQuota[]): ToolQuota[] {
  const claudeRows = quotas.filter((row) => {
    const host = (row.host_id || "").toLowerCase();
    const tool = row.tool.toLowerCase();
    const id = row.id.toLowerCase();
    return (
      host === "claude_desktop" ||
      host === "claude_cli" ||
      host === "claude" ||
      id.includes("claude_desktop") ||
      id.includes("claude_cli") ||
      ((tool.includes("claude desktop") || tool.includes("claude cli")) &&
        (row.access_mode || row.kind) === "subscription")
    );
  });
  if (claudeRows.length <= 1) {
    // Already merged (or single) — normalize label when it's a Claude subscription.
    return quotas.map((row) => {
      const host = (row.host_id || "").toLowerCase();
      const tool = row.tool.toLowerCase();
      if (
        (host === "claude" || host.startsWith("claude") || tool.includes("claude")) &&
        (row.access_mode || row.kind) === "subscription" &&
        !tool.includes("plugin")
      ) {
        return { ...row, tool: "Claude", host_id: row.host_id || "claude" };
      }
      return row;
    });
  }

  const subscription = claudeRows.filter(
    (row) => (row.access_mode || row.kind) === "subscription",
  );
  const primary = subscription[0] ?? claudeRows[0]!;
  const mergedIds = new Set(claudeRows.map((row) => row.id));
  const worstTone = claudeRows.reduce<ToolQuota["tone"]>((tone, row) => {
    if (row.tone === "amber" || row.exhausted) return "amber";
    if (row.tone === "warn" && tone !== "amber") return "warn";
    return tone;
  }, primary.tone);
  const maxPercent = claudeRows.reduce<number | null>((max, row) => {
    if (row.percent == null) return max;
    if (max == null) return row.percent;
    return Math.max(max, row.percent);
  }, null);

  const merged: ToolQuota = {
    ...primary,
    id: primary.id.startsWith("app:claude") ? "app:claude" : primary.id,
    tool: "Claude",
    host_id: "claude",
    tone: worstTone,
    percent: maxPercent,
    label: worstTone === "amber" || worstTone === "warn" ? primary.label : primary.label,
    exhausted: claudeRows.some((row) => row.exhausted),
  };

  return [merged, ...quotas.filter((row) => !mergedIds.has(row.id))];
}

/** Heartbeat / bus ping subjects — hidden by default in Event Stream (SR-02). */
export function isHeartbeatSubject(subject: string): boolean {
  return /heartbeat/i.test(subject);
}

/**
 * Düğüm tıklanınca: name / file basename / project, adr_summary + tags (+ agent, project_id) içinde.
 * Seçim yoksa yalnızca metin sorgusu uygulanır.
 */
export function experiencesMatchingSelection(
  experiences: LoungeExperience[],
  selection: SemanticMapSelection | null,
  query = "",
): LoungeExperience[] {
  const q = query.trim().toLowerCase();
  let rows = experiences;
  if (q) {
    rows = rows.filter((item) =>
      `${item.project_id} ${item.adr_summary} ${item.agent} ${item.tags.join(" ")}`
        .toLowerCase()
        .includes(q),
    );
  }
  if (!selection) {
    return rows;
  }
  const tokens = [
    selection.name,
    selection.id,
    selection.file,
    pathBasename(selection.file),
    // Proje adı yalnızca project düğümünde; fn/file seçiminde tüm repo experience'larını şişirmesin.
    selection.kind === "project" ? selection.project : null,
  ]
    .map((token) => token?.trim().toLowerCase() ?? "")
    .filter((token) => token.length >= 2);
  if (tokens.length === 0) {
    return rows;
  }
  return rows.filter((item) => {
    const hay = [
      item.project_id,
      item.adr_summary,
      item.agent,
      item.related_task_id ?? "",
      ...item.tags,
    ]
      .join(" ")
      .toLowerCase();
    return tokens.some((token) => hay.includes(token));
  });
}

/** Seçili düğüm/dosya ile ilişkili dead symbols (liste + map.dead birleşimi). */
export function deadSymbolsMatchingSelection(
  deadSymbols: DeadSymbol[],
  semanticMap: SemanticMap,
  selection: SemanticMapSelection | null,
): DeadSymbol[] {
  if (!selection) {
    return [];
  }
  const fromMap = semanticMap.projects.flatMap((project) => {
    if (selection.project && project.name && project.name !== selection.project) {
      return [];
    }
    return project.dead;
  });
  const pool = deadSymbols.length > 0 ? deadSymbols : fromMap;
  if (pool.length === 0) {
    return [];
  }

  if (selection.kind === "project") {
    const projectKey = (selection.project || selection.name || "").toLowerCase();
    return pool.filter((symbol) => {
      const pid = (symbol.project_id ?? "").toLowerCase();
      return !pid || !projectKey || pid === projectKey || pid.includes(projectKey);
    });
  }

  const nameTokens = [selection.name, selection.id]
    .map((token) => token?.trim().toLowerCase() ?? "")
    .filter((token) => token.length >= 2);
  const fileTokens = [selection.file, pathBasename(selection.file)]
    .map((token) => token?.trim().toLowerCase() ?? "")
    .filter((token) => token.length >= 2);

  return pool.filter((symbol) => {
    const symbolName = (symbol.name ?? "").toLowerCase();
    const symbolFile = (symbol.file ?? "").toLowerCase();
    const symbolBase = (pathBasename(symbol.file) ?? "").toLowerCase();
    const detail = (symbol.detail ?? "").toLowerCase();
    if (nameTokens.some((token) => symbolName.includes(token) || detail.includes(token))) {
      return true;
    }
    if (
      fileTokens.some(
        (token) =>
          symbolFile.includes(token) ||
          symbolBase.includes(token) ||
          (token.includes("/") && symbolFile.endsWith(token)),
      )
    ) {
      return true;
    }
    return false;
  });
}

/** CALLS kenarı hedefi: kısa isim (qualified_name son segment). */
export function shortSymbolName(id: string | null | undefined): string {
  if (!id?.trim()) {
    return "";
  }
  const trimmed = id.trim();
  const parts = trimmed.split(/[.:/\\]/).filter(Boolean);
  return parts.at(-1) ?? trimmed;
}

export function natsEventTone(subject: string, state?: NatsEvent["state"]): NatsTone {
  const text = `${subject} ${state ?? ""}`.toLowerCase();
  if (state === "error" || text.includes("failed") || text.includes(".error")) {
    return "error";
  }
  if (
    text.includes("completed") ||
    text.includes("success") ||
    text.includes("experience") ||
    text.includes("commit") ||
    text.includes("heartbeat") ||
    text.includes("telemetry")
  ) {
    return "success";
  }
  return "task";
}

export function natsToneLabel(tone: NatsTone): string {
  if (tone === "error") {
    return "Error";
  }
  if (tone === "success") {
    return "Success";
  }
  return "Task";
}

export function loungeMessageToEvent(message: LoungeMessage): NatsEvent {
  const kb = (message.payload_bytes / 1024).toFixed(1);
  let state: NatsEvent["state"] = "ok";
  if (message.subject.endsWith(".failed")) {
    state = "error";
  } else if (message.subject.endsWith(".requested")) {
    state = "queued";
  }
  const created = new Date(message.created_at);
  let time = message.created_at;
  if (!Number.isNaN(created.getTime())) {
    time = `${istanbulClockParts(created, true)}.${String(created.getMilliseconds()).padStart(3, "0")}`;
  }
  const telemetry = parseDecisionTelemetry(message);
  const ownMs = telemetry?.latency_ms ?? extractDecisionLatencyMs(message.payload);
  return {
    id: message.id,
    time,
    subject: message.subject,
    from: message.source_agent,
    to: message.target_agent?.trim() ? message.target_agent : "bus",
    payload: `${kb}kb`,
    state,
    decisionLabel: ownMs != null ? formatDecisionStreamLabel(ownMs) : undefined,
    chainLabel: extractWorkflowChainLabel(message.payload),
  };
}

export const MOCK_EVENTS: NatsEvent[] = [
  { id: "d1", time: "14:09:29.004", subject: "lounge.telemetry.decision", from: "decision_engine", to: "bus", payload: "0.2kb", state: "ok", decisionLabel: "Decision: 4ms" },
  { id: "1", time: "14:09:18.441", subject: "lounge.task.requested", from: "kernel", to: "dispatcher", payload: "1.2kb", state: "queued" },
  { id: "2", time: "14:09:18.512", subject: "lounge.task.assigned", from: "dispatcher", to: "ollama", payload: "0.4kb", state: "ok" },
  { id: "3", time: "14:09:19.108", subject: "lounge.task.completed", from: "dispatcher", to: "nats", payload: "3.8kb", state: "ok", chainLabel: "implement workflow engine -> Triggered Auto-Test after Code: implement workflow engine" },
  { id: "w1", time: "14:09:19.220", subject: "lounge.task.requested", from: "workflow_engine", to: "grok_bot", payload: "0.8kb", state: "queued", chainLabel: "implement workflow engine -> Triggered Auto-Test after Code: implement workflow engine" },
  { id: "4", time: "14:09:19.140", subject: "lounge.experience.reported", from: "kernel", to: "vault", payload: "2.1kb", state: "ok" },
  { id: "5", time: "14:09:21.002", subject: "lounge.task.requested", from: "alice", to: "kernel", payload: "0.9kb", state: "queued" },
  { id: "6", time: "14:09:22.774", subject: "lounge.task.failed", from: "dispatcher", to: "nats", payload: "0.6kb", state: "error" },
  { id: "7", time: "14:09:23.112", subject: "lounge.task.retry", from: "dispatcher", to: "kernel", payload: "0.6kb", state: "retry" },
  { id: "8", time: "14:09:24.089", subject: "lounge.vault.query", from: "agent_mcp", to: "vault", payload: "1.4kb", state: "ok" },
  { id: "9", time: "14:09:25.421", subject: "lounge.heartbeat.ping", from: "worker-01", to: "nats", payload: "0.1kb", state: "ok" },
  { id: "10", time: "14:09:26.115", subject: "lounge.experience.commit", from: "vault", to: "storage", payload: "4.2kb", state: "ok" },
  { id: "11", time: "14:09:27.802", subject: "lounge.task.assigned", from: "dispatcher", to: "ollama", payload: "0.5kb", state: "ok" },
  { id: "12", time: "14:09:28.190", subject: "lounge.task.completed", from: "dispatcher", to: "nats", payload: "2.9kb", state: "ok" },
];

export const MOCK_EXPERIENCES: LoungeExperience[] = [
  {
    id: "e1",
    type: "experience",
    agent: "lounge-kernel",
    project_id: "Agent-Lounge-OS",
    adr_summary: "Indexed dispatcher.rs + NATS subjects",
    outcome: "success",
    tags: [],
    created_at: "2026-09-18T11:09:00.000Z",
  },
  {
    id: "e2",
    type: "experience",
    agent: "lounge-kernel",
    project_id: "EchoMind",
    adr_summary: "Ollama tags probe, nats-server missing PATH",
    outcome: "partial",
    tags: [],
    created_at: "2026-09-18T10:51:00.000Z",
  },
  {
    id: "e3",
    type: "experience",
    agent: "lounge-kernel",
    project_id: "Agent-Lounge-OS",
    adr_summary: "MemoryBridge stdout MCP unwrap",
    outcome: "success",
    tags: [],
    created_at: "2026-09-18T09:02:00.000Z",
  },
  {
    id: "e4",
    type: "experience",
    agent: "lounge-kernel",
    project_id: "shared/lounge_protocol",
    adr_summary: "task.schema.json kind+repo_path",
    outcome: "success",
    tags: [],
    created_at: "2026-09-18T08:18:00.000Z",
  },
];

export type QuotaKind = AccessMode;
export type QuotaTone = "ok" | "warn" | "live" | "local" | "amber";

export type ToolQuota = {
  id: string;
  tool: string;
  kind: QuotaKind | string;
  unit: string;
  used: string;
  remaining: string;
  reset: string;
  percent: number | null;
  tone: QuotaTone;
  label: string;
  source?: string;
  exhausted?: boolean;
  access_mode?: AccessMode | string;
  host_id?: string | null;
};

export type QuotaState = {
  scanned_at: string;
  amber_alert: boolean;
  amber_tools: string[];
  quotas: ToolQuota[];
};

export type ProjectSummary = {
  name: string;
  root_path?: string | null;
  nodes: number;
  edges: number;
  files?: number | null;
};

export type IndexSnapshot = {
  project: string;
  status?: string | null;
  nodes: number;
  edges: number;
  files?: number | null;
  dead?: number;
};

export type IndexNotice = {
  tone: "success" | "error";
  text: string;
};

export function mockIndexSnapshot(repoPath: string): IndexSnapshot {
  const project = repoPath.split(/[/\\]/).filter(Boolean).at(-1) || "workspace";
  return {
    project,
    status: "ok",
    nodes: 42,
    edges: 18,
    files: 24,
    dead: 3,
  };
}

export function indexBusMessage(
  subject: "lounge.index.completed" | "lounge.index.failed",
  payload: Record<string, unknown>,
): LoungeMessage {
  const body = JSON.stringify(payload);
  return {
    id: crypto.randomUUID(),
    type: "index",
    subject,
    source_agent: "memory_bridge",
    target_agent: "vault",
    created_at: new Date().toISOString(),
    payload,
    payload_bytes: body.length,
  };
}

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
  /** Caller (CBM CALLS source). */
  from_id: string;
  /** Callee (CBM CALLS target). */
  to_id: string;
  /** Optional wire aliases from memory_bridge / CBM. */
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

export type QuotaExhaustedAction = "stop" | "ask_then_local" | "ask_then_abort";

export type AgentTrigger = {
  agent_id: string;
  label: string;
  when: string;
  enabled: boolean;
};

export type RoutingPolicy = {
  require_user_approval: boolean;
  on_quota_exhausted: QuotaExhaustedAction;
  local_fallback_agent: string;
  local_fallback_model: string;
  triggers: AgentTrigger[];
};

export type ApprovalKind =
  | "agent_switch"
  | "quota_local_fallback"
  | "quota_abort"
  | "security_critical"
  | "security_risky";

export type ApprovalRequest = {
  task_id: string;
  summary: string;
  from_agent: string;
  to_agent: string;
  kind: ApprovalKind;
  reason: string;
  expires_at?: string | null;
  timeout_secs?: number | null;
};

export type ApprovalCleared = {
  task_id: string;
  reason: string;
};

export const ROUTING_APPROVAL_EVENT = "lounge://routing-approval";
export const ROUTING_APPROVAL_CLEARED_EVENT = "lounge://routing-approval-cleared";

export function approvalRemainingSecs(approval: ApprovalRequest, nowMs = Date.now()): number | null {
  if (approval.expires_at) {
    const ends = Date.parse(approval.expires_at);
    if (!Number.isNaN(ends)) {
      return Math.max(0, Math.ceil((ends - nowMs) / 1000));
    }
  }
  if (typeof approval.timeout_secs === "number" && approval.timeout_secs > 0) {
    return approval.timeout_secs;
  }
  return null;
}

export function formatApprovalClearReason(reason: string): string {
  switch (reason) {
    case "timeout":
      return "Onay süresi doldu — banner kapatıldı";
    case "failed":
      return "Görev başarısız oldu — onay iptal edildi";
    case "channel_closed":
      return "Onay kanalı kapandı";
    default:
      return `Onay temizlendi (${reason})`;
  }
}

export function isSecurityApproval(kind: ApprovalKind | string | undefined): boolean {
  return kind === "security_critical" || kind === "security_risky";
}

export function isQuotaApproval(kind: ApprovalKind | string | undefined): boolean {
  return kind === "quota_local_fallback" || kind === "quota_abort";
}

export function parseSecurityAlert(message: LoungeMessage): ApprovalRequest | null {
  if (message.subject !== ALERT_SECURITY) {
    return null;
  }
  const payload = message.payload;
  if (!payload || typeof payload !== "object") {
    return null;
  }
  const row = payload as Record<string, unknown>;
  const taskId = typeof row.task_id === "string" ? row.task_id : "";
  if (!taskId) {
    return null;
  }
  const kindRaw = typeof row.kind === "string" ? row.kind : "security_critical";
  const kind: ApprovalKind =
    kindRaw === "security_risky" ? "security_risky" : "security_critical";
  return {
    task_id: taskId,
    summary: typeof row.summary === "string" ? row.summary : "",
    from_agent: typeof row.from_agent === "string" ? row.from_agent : message.source_agent,
    to_agent: typeof row.to_agent === "string" ? row.to_agent : "lounge-kernel",
    kind,
    reason:
      typeof row.reason === "string"
        ? row.reason
        : typeof row.message === "string"
          ? row.message
          : SECURITY_OVERLAY_PROMPT,
  };
}

export function parseQuotaAlert(message: LoungeMessage): ApprovalRequest | null {
  if (message.subject !== ALERT_QUOTA) {
    return null;
  }
  const payload = message.payload;
  if (!payload || typeof payload !== "object") {
    return null;
  }
  const row = payload as Record<string, unknown>;
  const taskId = typeof row.task_id === "string" ? row.task_id : "";
  if (!taskId) {
    return null;
  }
  const kindRaw = typeof row.kind === "string" ? row.kind : "quota_local_fallback";
  const kind: ApprovalKind =
    kindRaw === "quota_abort" ? "quota_abort" : "quota_local_fallback";
  const lmrDown =
    row.lmr_available === false
      ? " · Lounge LMR (127.0.0.1:18790) ayakta değil; host Ollama kullanılmaz."
      : "";
  const baseReason =
    typeof row.reason === "string"
      ? row.reason
      : typeof row.message === "string"
        ? row.message
        : QUOTA_ALERT_PROMPT;
  return {
    task_id: taskId,
    summary: typeof row.summary === "string" ? row.summary : "",
    from_agent: typeof row.from_agent === "string" ? row.from_agent : message.source_agent,
    to_agent: typeof row.to_agent === "string" ? row.to_agent : "lmr",
    kind,
    reason: `${baseReason}${lmrDown}`,
  };
}

export type RoutingVote = "approve" | "approve_local" | "deny";

export const DEFAULT_POLICY: RoutingPolicy = {
  require_user_approval: true,
  on_quota_exhausted: "ask_then_local",
  local_fallback_agent: "lmr",
  local_fallback_model: "llama3.1:8b",
  triggers: [
    { agent_id: "cursor", label: "Cursor", when: "code_analysis", enabled: true },
    { agent_id: "claude", label: "Claude", when: "review", enabled: true },
    { agent_id: "grok", label: "Grok", when: "general", enabled: true },
    { agent_id: "lmr", label: "LMR", when: "fallback", enabled: true },
  ],
};

export const MOCK_QUOTAS: ToolQuota[] = [
    {
        id: "app:cursor",
        tool: "Cursor",
        kind: "subscription",
        unit: "subscription",
        used: "15% · $232 / $400",
        remaining: "$168",
        reset: "—",
        percent: 15,
        tone: "ok",
        label: "15% plan",
        access_mode: "subscription",
        host_id: "cursor",
    },
    {
        id: "app:claude_desktop",
        tool: "Claude Desktop",
        kind: "subscription",
        unit: "subscription",
        used: "0% 5s · 56% 7g",
        remaining: "100% 5s · 44% 7g",
        reset: "kullanınca · —",
        percent: 56,
        tone: "ok",
        label: "56%",
        access_mode: "subscription",
        host_id: "claude_desktop",
    },
    {
        id: "app:claude_cli",
        tool: "Claude CLI",
        kind: "subscription",
        unit: "subscription",
        used: "0% 5s · 56% 7g",
        remaining: "100% 5s · 44% 7g",
        reset: "kullanınca · —",
        percent: 56,
        tone: "ok",
        label: "56%",
        access_mode: "subscription",
        host_id: "claude_cli",
    },
    {
        id: "app:grok_bot",
        tool: "Grok Bot",
        kind: "subscription",
        unit: "subscription",
        used: "42%",
        remaining: "58%",
        reset: "—",
        percent: 42,
        tone: "ok",
        label: "42%",
        access_mode: "subscription",
        host_id: "grok_bot",
    },
    {
        id: "app:antigravity",
        tool: "Antigravity",
        kind: "subscription",
        unit: "subscription",
        used: "20% 5s · 35% 7g · 10% 3p 5s",
        remaining: "80% 5s · 65% 7g · 90% 3p 5s",
        reset: "kullanınca · — · kullanınca",
        percent: 35,
        tone: "ok",
        label: "35%",
        access_mode: "subscription",
        host_id: "antigravity",
    },
  {
    id: "lmr",
    tool: "LMR · llama3.1:8b",
    kind: "local",
    unit: "ram/vram",
    used: "6.1 / 8.0 GB",
    remaining: "1.9 GB",
    reset: "LOCAL",
    percent: 76,
    tone: "ok",
    label: "76% ram",
    access_mode: "local",
    host_id: null,
  },
  {
    id: "nats",
    tool: "NATS broker",
    kind: "local",
    unit: "conn",
    used: "2 conn · 40 in_msgs",
    remaining: "local",
    reset: "LOCAL",
    percent: null,
    tone: "ok",
    label: "ok LOCAL",
    access_mode: "local",
    host_id: null,
  },
  {
    id: "cbm",
    tool: "codebase-memory-mcp",
    kind: "local",
    unit: "local",
    used: "ready",
    remaining: "unlimited",
    reset: "LOCAL",
    percent: null,
    tone: "local",
    label: "ok LOCAL",
    access_mode: "local",
    host_id: null,
  },
  {
    id: "plugin:notion",
    tool: "notion",
    kind: "plugin",
    unit: "plugin",
    used: "2 host",
    remaining: "Cursor · Antigravity",
    reset: "HOST",
    percent: null,
    tone: "ok",
    label: "plugin",
    access_mode: "plugin",
    host_id: "cursor",
  },
  {
    id: "claude_desktop:github",
    tool: "github",
    kind: "plugin",
    unit: "plugin",
    used: "host üzerinden",
    remaining: "Claude Desktop",
    reset: "HOST",
    percent: null,
    tone: "ok",
    label: "plugin",
    access_mode: "plugin",
    host_id: "claude_desktop",
  },
];

export function quotaBarClass(percent: number): string {
  if (percent >= 90) {
    return "bg-error";
  }
  if (percent >= AMBER_THRESHOLD) {
    return "bg-tertiary";
  }
  return "bg-primary";
}

export function quotaToneClass(tone: QuotaTone): string {
  if (tone === "warn" || tone === "amber") {
    return "bg-error-container/40 text-error-dim border-error-container";
  }
  if (tone === "live") {
    return "bg-secondary-container/60 text-secondary-dim border-secondary-container";
  }
  return "bg-secondary-container/40 text-secondary-dim border-secondary-container";
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Ajan Verimlilik Raporu — Rust `AgentEfficiencyReport` ile snake/camel uyumlu. */
export type AgentFailureRow = {
  agentId: string;
  failureCount: number;
  metricLabel: string;
};

export type DeadCleanupStats = {
  remaining: number;
  cleaned: number | null;
  rate: number | null;
  previousCount: number | null;
  note: string;
};

export type AgentEfficiencyReport = {
  generatedAt: string;
  scopeLabel: string;
  weekly: boolean;
  projectId: string | null;
  windowStart: string | null;
  windowEnd: string;
  failuresByAgent: AgentFailureRow[];
  totalFailures: number;
  crossProjectWhisperEvents: number;
  crossProjectExperienceHits: number;
  dead: DeadCleanupStats;
  markdown: string;
  sourceNotes: string[];
};

function asCamelReport(raw: Record<string, unknown>): AgentEfficiencyReport {
  const deadRaw =
    raw.dead && typeof raw.dead === "object" && !Array.isArray(raw.dead)
      ? (raw.dead as Record<string, unknown>)
      : {};
  const failuresRaw = Array.isArray(raw.failuresByAgent)
    ? raw.failuresByAgent
    : Array.isArray(raw.failures_by_agent)
      ? raw.failures_by_agent
      : [];
  const failuresByAgent: AgentFailureRow[] = [];
  for (const row of failuresRaw) {
    if (!row || typeof row !== "object" || Array.isArray(row)) continue;
    const r = row as Record<string, unknown>;
    failuresByAgent.push({
      agentId: String(r.agentId ?? r.agent_id ?? "unknown"),
      failureCount: Number(r.failureCount ?? r.failure_count ?? 0) || 0,
      metricLabel: String(r.metricLabel ?? r.metric_label ?? "hata/failure sayısı"),
    });
  }
  const notesRaw = Array.isArray(raw.sourceNotes)
    ? raw.sourceNotes
    : Array.isArray(raw.source_notes)
      ? raw.source_notes
      : [];
  return {
    generatedAt: String(raw.generatedAt ?? raw.generated_at ?? new Date().toISOString()),
    scopeLabel: String(raw.scopeLabel ?? raw.scope_label ?? ""),
    weekly: Boolean(raw.weekly ?? true),
    projectId: (raw.projectId ?? raw.project_id ?? null) as string | null,
    windowStart: (raw.windowStart ?? raw.window_start ?? null) as string | null,
    windowEnd: String(raw.windowEnd ?? raw.window_end ?? new Date().toISOString()),
    failuresByAgent,
    totalFailures: Number(raw.totalFailures ?? raw.total_failures ?? 0) || 0,
    crossProjectWhisperEvents:
      Number(raw.crossProjectWhisperEvents ?? raw.cross_project_whisper_events ?? 0) || 0,
    crossProjectExperienceHits:
      Number(raw.crossProjectExperienceHits ?? raw.cross_project_experience_hits ?? 0) || 0,
    dead: {
      remaining: Number(deadRaw.remaining ?? 0) || 0,
      cleaned: (() => {
        const v = deadRaw.cleaned;
        if (v === null || v === undefined) return null;
        return Number(v) || 0;
      })(),
      rate: (() => {
        const v = deadRaw.rate;
        if (v === null || v === undefined) return null;
        const n = Number(v);
        return Number.isFinite(n) ? n : null;
      })(),
      previousCount: (() => {
        const v = deadRaw.previousCount ?? deadRaw.previous_count;
        if (v === null || v === undefined) return null;
        return Number(v) || 0;
      })(),
      note: String(deadRaw.note ?? ""),
    },
    markdown: String(raw.markdown ?? ""),
    sourceNotes: notesRaw.map((n) => String(n)),
  };
}

/** Tarayıcı / mock: bellek içi event + deadSymbols'dan dürüst boş/örnek rapor. */
export function buildBrowserEfficiencyReport(input: {
  events: NatsEvent[];
  experiences: LoungeExperience[];
  deadSymbols: DeadSymbol[];
  weekly?: boolean;
  projectId?: string | null;
}): AgentEfficiencyReport {
  const weekly = input.weekly !== false;
  const projectId = input.projectId?.trim() || null;
  const now = new Date();
  const windowStart = weekly
    ? new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000).toISOString()
    : null;
  const since = windowStart ? Date.parse(windowStart) : 0;

  const failCounts = new Map<string, number>();
  for (const ev of input.events) {
    if (!ev.subject?.includes("task.failed") && ev.state !== "error") continue;
    const agent = ev.from || "unknown";
    failCounts.set(agent, (failCounts.get(agent) ?? 0) + 1);
  }
  for (const exp of input.experiences) {
    if (exp.outcome !== "failure") continue;
    if (projectId && exp.project_id !== projectId) continue;
    const created = Date.parse(exp.created_at || "");
    if (windowStart && Number.isFinite(created) && created < since) continue;
    const agent = exp.agent || "unknown";
    failCounts.set(agent, (failCounts.get(agent) ?? 0) + 1);
  }
  const failuresByAgent: AgentFailureRow[] = [...failCounts.entries()]
    .map(([agentId, failureCount]) => ({
      agentId,
      failureCount,
      metricLabel: "hata/failure sayısı",
    }))
    .sort((a, b) => b.failureCount - a.failureCount);

  let crossHits = 0;
  let crossEvents = 0;
  for (const ev of input.events) {
    if (ev.subject !== CONTEXT_WHISPER && !ev.subject?.includes("context.whisper")) continue;
    crossEvents += 1;
    crossHits += 1;
  }
  for (const exp of input.experiences) {
    if (exp.tags?.includes("cross-project") || exp.tags?.includes("whisper")) {
      if (projectId && exp.project_id === projectId) continue;
      crossHits += 1;
    }
  }

  const remaining = projectId
    ? input.deadSymbols.filter((d) => !d.project_id || d.project_id === projectId).length
    : input.deadSymbols.length;

  const scopeLabel = weekly
    ? projectId
      ? `haftalık + proje (${projectId}) · tarayıcı mock`
      : "haftalık (son 7 gün) · tarayıcı mock"
    : projectId
      ? `proje (${projectId}) · tarayıcı mock`
      : "tüm zamanlar · tarayıcı mock";

  const report: AgentEfficiencyReport = {
    generatedAt: now.toISOString(),
    scopeLabel,
    weekly,
    projectId,
    windowStart,
    windowEnd: now.toISOString(),
    failuresByAgent,
    totalFailures: failuresByAgent.reduce((s, r) => s + r.failureCount, 0),
    crossProjectWhisperEvents: crossEvents,
    crossProjectExperienceHits: crossHits,
    dead: {
      remaining,
      cleaned: null,
      rate: null,
      previousCount: null,
      note: "Tarayıcı modu: ölü sembol snapshot geçmişi yok; yalnızca mevcut sayı.",
    },
    markdown: "",
    sourceNotes: [
      "Tarayıcı mock: Tauri SQLite yok — NATS event / bellek içi experiences kullanıldı.",
      "Hata: task.failed event + outcome=failure experiences.",
      "Cross-Project: lounge.context.whisper event + whisper etiketli experiences.",
      "Ölü sembol oranı: snapshot yok → % uydurulmadı.",
    ],
  };
  report.markdown = renderEfficiencyMarkdown(report);
  return report;
}

export function renderEfficiencyMarkdown(report: AgentEfficiencyReport): string {
  const lines: string[] = [
    "# Agent Efficiency Report / Ajan Verimlilik Raporu",
    "",
    `- Üretilme: ${report.generatedAt}`,
    `- Kapsam: ${report.scopeLabel}`,
  ];
  if (report.windowStart) {
    lines.push(`- Pencere: ${report.windowStart} → ${report.windowEnd}`);
  }
  if (report.projectId) {
    lines.push(`- Proje: \`${report.projectId}\``);
  }
  lines.push("", "## 1. Ajan başına hata / failure", "");
  lines.push("Metrik: failure sayısı (uydurma bug sayacı değil).", "");
  if (report.failuresByAgent.length === 0) {
    lines.push("_Kayıt yok._", "");
  } else {
    lines.push("| Ajan | Hata/failure |", "| --- | ---: |");
    for (const row of report.failuresByAgent) {
      lines.push(`| ${row.agentId} | ${row.failureCount} |`);
    }
    lines.push("", `**Toplam:** ${report.totalFailures}`, "");
  }
  lines.push(
    "## 2. Cross-Project Experience kullanımı",
    "",
    `- Fısıltı olayları: **${report.crossProjectWhisperEvents}**`,
    `- Çapraz-proje tecrübe satırı: **${report.crossProjectExperienceHits}**`,
    "",
    "## 3. Ölü sembol (Dead Symbol) temizleme",
    "",
    `- Kalan (mevcut): **${report.dead.remaining}**`,
    report.dead.cleaned == null
      ? "- Temizlenen: _hesaplanamadı_"
      : `- Temizlenen: **${report.dead.cleaned}**`,
    report.dead.rate == null
      ? "- Oran: _yok (sahte % üretilmedi)_"
      : `- Oran: **${(report.dead.rate * 100).toFixed(1)}%**`,
    `- Not: ${report.dead.note}`,
    "",
    "## Kaynak notları",
    "",
  );
  for (const note of report.sourceNotes) {
    lines.push(`- ${note}`);
  }
  lines.push("", "---", "_Agent Lounge OS · Markdown dışa aktarım (PDF için yazdırın)._");
  return lines.join("\n");
}

export async function fetchAgentEfficiencyReport(options?: {
  weekly?: boolean;
  projectId?: string | null;
}): Promise<AgentEfficiencyReport> {
  if (!isTauri()) {
    throw new Error("Tauri yok");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  const raw = await invoke<Record<string, unknown>>("agent_efficiency_report", {
    weekly: options?.weekly ?? true,
    projectId: options?.projectId?.trim() || null,
  });
  const report = asCamelReport(raw ?? {});
  if (!report.markdown) {
    report.markdown = renderEfficiencyMarkdown(report);
  }
  return report;
}

export function downloadMarkdownFile(filename: string, content: string): void {
  const blob = new Blob([content], { type: "text/markdown;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename.endsWith(".md") ? filename : `${filename}.md`;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

export async function pickWorkspaceFolder(): Promise<string | null> {
  if (!isTauri()) {
    return null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Index Workspace",
  });
  if (typeof selected !== "string" || selected.length === 0) {
    return null;
  }
  return selected;
}

function istanbulClockParts(date: Date, withSeconds = false): string {
  const parts = new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    second: withSeconds ? "2-digit" : undefined,
    hour12: false,
    timeZone: "Europe/Istanbul",
  }).formatToParts(date);
  const pick = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((part) => part.type === type)?.value ?? "00";
  const clock = `${pick("hour")}:${pick("minute")}`;
  return withSeconds ? `${clock}:${pick("second")}` : clock;
}

export function formatClock(date: Date): string {
  return istanbulClockParts(date);
}

export function formatExperienceTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return iso.slice(11, 16) || iso;
  }
  return istanbulClockParts(date);
}
