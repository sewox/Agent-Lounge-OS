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

export type DiscoveredTool = {
  id: string;
  name: string;
  kind: "model" | "mcp" | "system" | string;
  source: "claude_desktop" | "cursor" | "ollama" | "system" | string;
  origin_path?: string | null;
  command?: string | null;
  args: string[];
  endpoint?: string | null;
  detail?: string | null;
  available: boolean;
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
  models: DiscoveredTool[];
  mcp_servers: DiscoveredTool[];
  system_tools: SystemTool[];
};

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

export const MOCK_DISCOVERY: DiscoveryReport = {
  scanned_at: new Date().toISOString(),
  sources: [
    {
      id: "claude_desktop",
      available: true,
      origin_path: "~/Library/Application Support/Claude/claude_desktop_config.json",
      detail: "1 araç",
    },
    {
      id: "cursor",
      available: true,
      origin_path: "~/.cursor/mcp.json",
      detail: "2 araç",
    },
    {
      id: "ollama",
      available: true,
      origin_path: "http://127.0.0.1:11434/api/tags",
      detail: "2 model",
    },
    {
      id: "system",
      available: true,
      origin_path: null,
      detail: "2 / 3 PATH",
    },
  ],
  models: [
    {
      id: "ollama:llama3.1:8b",
      name: "llama3.1:8b",
      kind: "model",
      source: "ollama",
      origin_path: "http://127.0.0.1:11434/api/tags",
      command: null,
      args: [],
      endpoint: "http://127.0.0.1:11434",
      detail: null,
      available: true,
    },
    {
      id: "ollama:qwen2.5:7b",
      name: "qwen2.5:7b",
      kind: "model",
      source: "ollama",
      origin_path: "http://127.0.0.1:11434/api/tags",
      command: null,
      args: [],
      endpoint: "http://127.0.0.1:11434",
      detail: null,
      available: true,
    },
  ],
  mcp_servers: [
    {
      id: "claude_desktop:github",
      name: "github",
      kind: "mcp",
      source: "claude_desktop",
      origin_path: "~/Library/Application Support/Claude/claude_desktop_config.json",
      command: "npx",
      args: ["-y", "@modelcontextprotocol/server-github"],
      endpoint: null,
      detail: "env keys: GITHUB_TOKEN",
      available: true,
    },
    {
      id: "cursor:notion",
      name: "notion",
      kind: "mcp",
      source: "cursor",
      origin_path: "~/.cursor/mcp.json",
      command: "npx",
      args: ["-y", "@notionhq/mcp"],
      endpoint: null,
      detail: "env keys: NOTION_TOKEN",
      available: true,
    },
    {
      id: "cursor:codebase-memory",
      name: "codebase-memory",
      kind: "mcp",
      source: "cursor",
      origin_path: ".cursor/mcp.json",
      command: "codebase-memory-mcp",
      args: [],
      endpoint: null,
      detail: null,
      available: true,
    },
  ],
  system_tools: [
    { id: "system:git", name: "git", available: true, path: "/usr/bin/git", detail: null },
    { id: "system:gh", name: "gh", available: true, path: "/opt/homebrew/bin/gh", detail: null },
    { id: "system:docker", name: "docker", available: false, path: null, detail: "PATH'te bulunamadı" },
  ],
  tools: [
    {
      id: "ollama:llama3.1:8b",
      name: "llama3.1:8b",
      kind: "model",
      source: "ollama",
      origin_path: "http://127.0.0.1:11434/api/tags",
      command: null,
      args: [],
      endpoint: "http://127.0.0.1:11434",
      detail: null,
      available: true,
    },
    {
      id: "ollama:qwen2.5:7b",
      name: "qwen2.5:7b",
      kind: "model",
      source: "ollama",
      origin_path: "http://127.0.0.1:11434/api/tags",
      command: null,
      args: [],
      endpoint: "http://127.0.0.1:11434",
      detail: null,
      available: true,
    },
    {
      id: "claude_desktop:github",
      name: "github",
      kind: "mcp",
      source: "claude_desktop",
      origin_path: "~/Library/Application Support/Claude/claude_desktop_config.json",
      command: "npx",
      args: ["-y", "@modelcontextprotocol/server-github"],
      endpoint: null,
      detail: "env keys: GITHUB_TOKEN",
      available: true,
    },
    {
      id: "cursor:notion",
      name: "notion",
      kind: "mcp",
      source: "cursor",
      origin_path: "~/.cursor/mcp.json",
      command: "npx",
      args: ["-y", "@notionhq/mcp"],
      endpoint: null,
      detail: "env keys: NOTION_TOKEN",
      available: true,
    },
    {
      id: "cursor:codebase-memory",
      name: "codebase-memory",
      kind: "mcp",
      source: "cursor",
      origin_path: ".cursor/mcp.json",
      command: "codebase-memory-mcp",
      args: [],
      endpoint: null,
      detail: null,
      available: true,
    },
    {
      id: "system:git",
      name: "git",
      kind: "system",
      source: "system",
      origin_path: "/usr/bin/git",
      command: "/usr/bin/git",
      args: [],
      endpoint: null,
      detail: null,
      available: true,
    },
    {
      id: "system:gh",
      name: "gh",
      kind: "system",
      source: "system",
      origin_path: "/opt/homebrew/bin/gh",
      command: "/opt/homebrew/bin/gh",
      args: [],
      endpoint: null,
      detail: null,
      available: true,
    },
    {
      id: "system:docker",
      name: "docker",
      kind: "system",
      source: "system",
      origin_path: null,
      command: "docker",
      args: [],
      endpoint: null,
      detail: "PATH'te bulunamadı",
      available: false,
    },
  ],
};

export const BUS_UI_EVENT = "lounge://bus";

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

export type QuotaKind = "ai" | "bot";
export type QuotaTone = "ok" | "warn" | "live" | "local";

export type ToolQuota = {
  id: string;
  tool: string;
  kind: QuotaKind;
  unit: string;
  used: string;
  remaining: string;
  reset: string;
  percent: number | null;
  tone: QuotaTone;
  label: string;
  source?: string;
  exhausted?: boolean;
};

export type ProjectSummary = {
  name: string;
  root_path?: string | null;
  nodes: number;
  edges: number;
};

export type IndexSnapshot = {
  project: string;
  status?: string | null;
  nodes: number;
  edges: number;
  files?: number | null;
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

export type ApprovalKind = "agent_switch" | "quota_local_fallback" | "quota_abort";

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
  local_fallback_agent: "ollama",
  local_fallback_model: "llama3.1:8b",
  triggers: [
    { agent_id: "cursor", label: "Cursor", when: "code_analysis", enabled: true },
    { agent_id: "claude", label: "Claude", when: "review", enabled: true },
    { agent_id: "grok", label: "Grok", when: "general", enabled: true },
    { agent_id: "ollama", label: "Ollama (local)", when: "fallback", enabled: true },
  ],
};

export const MOCK_QUOTAS: ToolQuota[] = [
  { id: "cursor", tool: "Cursor · Grok 4.6", kind: "ai", unit: "req/day", used: "412 / 500", remaining: "88 remaining", reset: "00:00 UTC", percent: 82, tone: "warn", label: "warn (82%)" },
  { id: "claude", tool: "Claude · Sonnet", kind: "ai", unit: "tokens", used: "1.24M / 5.00M", remaining: "3.76M left", reset: "01 Oct", percent: 25, tone: "ok", label: "ok (25%)" },
  { id: "xai", tool: "xAI · Grok API", kind: "ai", unit: "tokens", used: "18.4k / 50k", remaining: "31.6k left", reset: "rolling", percent: 37, tone: "ok", label: "ok (37%)" },
  { id: "ollama", tool: "Ollama · llama3.1:8b", kind: "ai", unit: "local", used: "6.1 / 8.0 GB", remaining: "local unlimited", reset: "LOCAL", percent: 76, tone: "ok", label: "ok (76% vram)" },
  { id: "stitch", tool: "Stitch · Gemini", kind: "bot", unit: "gens", used: "14 / 50", remaining: "36 left", reset: "monthly", percent: 28, tone: "ok", label: "ok (28%)" },
  { id: "cbm", tool: "codebase-memory-mcp", kind: "bot", unit: "local", used: "— / ∞", remaining: "unlimited", reset: "LOCAL", percent: null, tone: "local", label: "ok LOCAL" },
  { id: "notion", tool: "Notion MCP", kind: "bot", unit: "req/min", used: "42 / 180", remaining: "138 left", reset: "60s", percent: 23, tone: "ok", label: "ok (23%)" },
  { id: "postman", tool: "Postman MCP", kind: "bot", unit: "req/min", used: "11 / 60", remaining: "49 left", reset: "60s", percent: 18, tone: "ok", label: "ok (18%)" },
  { id: "browser", tool: "Browser tool", kind: "bot", unit: "session", used: "active", remaining: "—", reset: "—", percent: null, tone: "live", label: "live" },
];

export function quotaBarClass(percent: number): string {
  if (percent >= 90) {
    return "bg-error";
  }
  if (percent >= 70) {
    return "bg-tertiary";
  }
  return "bg-primary";
}

export function quotaToneClass(tone: QuotaTone): string {
  if (tone === "warn") {
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
