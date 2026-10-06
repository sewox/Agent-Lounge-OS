import type { FixtureDataset } from "./types";

const mkHealth = (
  id: string,
  name: string,
  running: boolean,
  endpoint: string,
  opts?: { availability?: string | null; error?: string | null; detail?: string | null },
): FixtureDataset["serviceReport"]["ollama"] => ({
  id,
  name,
  running,
  started_by_us: false,
  endpoint,
  detail:
    opts?.detail ??
    (running ? "ok" : opts?.availability === "not_installed" ? "optional" : null),
  error:
    opts?.error ?? (running || opts?.availability === "not_installed" ? null : "disconnected"),
  availability: opts?.availability ?? null,
});

/** Empty / fresh-install dataset — optional LMR/NATS absent, never MOCK_* totals. */
export const EMPTY_FIXTURE: FixtureDataset = {
  name: "empty",
  experiences: [],
  deadSymbols: [],
  semanticMap: { projects: [] },
  projects: [],
  quotas: [],
  quotaState: {
    scanned_at: new Date().toISOString(),
    amber_alert: false,
    amber_tools: [],
    quotas: [],
  },
  events: [],
  serviceReport: {
    ollama: mkHealth("ollama", "LMR", false, "http://127.0.0.1:18790", {
      availability: "not_installed",
      detail: "optional — place an Ollama-compatible binary at data/lmr/ollama",
    }),
    nats: mkHealth("nats", "NATS", false, "nats://127.0.0.1:4222", {
      availability: "not_installed",
      detail: "optional — install nats-server on PATH",
    }),
    memory: mkHealth("memory", "Memory Bridge", false, "http://127.0.0.1:7432"),
    plugin: mkHealth("plugin", "MCP Plugin", false, "stdio"),
  },
  models: [],
  kernelModel: "",
  policy: {
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
  },
  decisionGate: {
    phase: "available",
    title: "DecisionGate",
    message: "Laya available",
    detail: null,
    device: null,
    reason: null,
  },
  layaEngine: {
    phase: "failed",
    label: "Laya",
    message: "Not installed",
    file: null,
    completed: 0,
    total: 0,
    percentage: 0,
    path: "",
    error: "missing",
  },
  pendingApprovals: [],
  discovery: {
    scanned_at: new Date().toISOString(),
    sources: [],
    apps: [],
    models: [],
    mcp_servers: [],
    tools: [],
  },
  connectedTools: [],
  graphUiPort: 18749,
  graphUiPortMode: "auto",
  graphUiStatus: {
    enabled: false,
    running: false,
    url: null,
    detail: null,
    port: 18749,
    port_mode: "auto",
    owned_by_lounge: false,
  },
  workers: [],
  agentSessions: [],
  backgroundTasks: [],
};
