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
export const TELEMETRY_DECISION = "lounge.telemetry.decision";
export const LATENCY_SPARK_CAP = 24;
export const MSG_MIN_WINDOW_MS = 60_000;
export const AMBER_THRESHOLD = 80;

export type LoungeTelemetry = {
  kind: string;
  message_id: string;
  subject: string;
  latency_us: number;
  latency_ms: number;
  routing?: string;
  security?: string;
  knowledge_hit?: number;
  device?: string;
  timestamp?: string;
  msg_per_min?: number;
};

export function isDecisionTelemetrySubject(subject: string): boolean {
  return subject === TELEMETRY_DECISION || subject.startsWith("lounge.telemetry.");
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

export function parseDecisionTelemetry(message: LoungeMessage): LoungeTelemetry | null {
  if (!isDecisionTelemetrySubject(message.subject)) {
    return null;
  }
  if (!message.payload || typeof message.payload !== "object" || Array.isArray(message.payload)) {
    return null;
  }
  const payload = message.payload as Record<string, unknown>;
  const latencyUs = asFiniteNumber(payload.latency_us);
  const latencyMsRaw = asFiniteNumber(payload.latency_ms);
  const latencyMs = latencyMsRaw ?? (latencyUs != null ? latencyUs / 1000 : null);
  if (latencyUs == null && latencyMs == null) {
    return null;
  }
  const resolvedUs = latencyUs ?? Math.round((latencyMs ?? 0) * 1000);
  const resolvedMs = latencyMs ?? resolvedUs / 1000;
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
    knowledge_hit: asFiniteNumber(payload.knowledge_hit) ?? undefined,
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

export function pruneMsgWindow(timestamps: number[], now = Date.now()): number[] {
  const cutoff = now - MSG_MIN_WINDOW_MS;
  return timestamps.filter((at) => at >= cutoff);
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
    const clock = new Intl.DateTimeFormat("tr-TR", {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hour12: false,
      timeZone: "Europe/Istanbul",
    }).format(created);
    time = `${clock}.${String(created.getMilliseconds()).padStart(3, "0")}`;
  }
  return {
    id: message.id,
    time,
    subject: message.subject,
    from: message.source_agent,
    to: message.target_agent?.trim() ? message.target_agent : "bus",
    payload: `${kb}kb`,
    state,
  };
}

export type SemanticNode = {
  name: string;
  edges: number;
  modules: string[];
};

export type ProjectHealthRow = {
  name: string;
  indexed: number;
  files: string;
  nodes: string;
  stale: number;
  dead: number;
  sync: string;
};

export const MOCK_EVENTS: NatsEvent[] = [
  { id: "1", time: "14:09:18.441", subject: "lounge.task.requested", from: "kernel", to: "dispatcher", payload: "1.2kb", state: "queued" },
  { id: "2", time: "14:09:18.512", subject: "lounge.task.assigned", from: "dispatcher", to: "ollama", payload: "0.4kb", state: "ok" },
  { id: "3", time: "14:09:19.108", subject: "lounge.task.completed", from: "dispatcher", to: "nats", payload: "3.8kb", state: "ok" },
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

export const MOCK_NODES: SemanticNode[] = [
  { name: "Agent-Lounge-OS", edges: 42, modules: ["kernel.rs", "dispatcher.rs", "memory_bridge.rs"] },
  { name: "EchoMind", edges: 118, modules: ["ollama_client.py", "embeddings.py"] },
  { name: "codebase-memory-mcp", edges: 87, modules: ["indexer.ts", "server.ts"] },
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

export const MOCK_HEALTH: ProjectHealthRow[] = [
  { name: "Agent-Lounge-OS", indexed: 100, files: "9,421", nodes: "4,810", stale: 0, dead: 12, sync: "14:09" },
  { name: "EchoMind", indexed: 86, files: "4,120", nodes: "1,940", stale: 3, dead: 41, sync: "13:44" },
  { name: "codebase-memory-mcp", indexed: 100, files: "4,861", nodes: "2,210", stale: 0, dead: 74, sync: "09:12" },
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
  from_id: string;
  to_id: string;
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
  | "security_critical";

export type ApprovalRequest = {
  task_id: string;
  summary: string;
  from_agent: string;
  to_agent: string;
  kind: ApprovalKind;
  reason: string;
};

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

export function formatClock(date: Date): string {
  return new Intl.DateTimeFormat("tr-TR", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    timeZone: "Europe/Istanbul",
  }).format(date);
}

export function formatExperienceTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return iso.slice(11, 16) || iso;
  }
  return new Intl.DateTimeFormat("tr-TR", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    timeZone: "Europe/Istanbul",
  }).format(date);
}
