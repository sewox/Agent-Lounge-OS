/**
 * Browser-harness fixtures only. Never import into Tauri/live fallback paths.
 * mock-ban allowlists this directory for MOCK_* identifiers.
 */
import type { LoungeExperience, NatsEvent, ToolQuota } from "@/lib/lounge";

const browserEvents: NatsEvent[] = [
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

const browserExperiences: LoungeExperience[] = [
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

const browserQuotas: ToolQuota[] = [
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

/** @deprecated Prefer browserEvents — kept for mock-ban allowlist clarity */
export const MOCK_EVENTS = browserEvents;
/** @deprecated Prefer browserExperiences */
export const MOCK_EXPERIENCES = browserExperiences;
/** @deprecated Prefer browserQuotas */
export const MOCK_QUOTAS = browserQuotas;

export { browserEvents, browserExperiences, browserQuotas };

export function searchBrowserExperiences(query: string, limit = 8): LoungeExperience[] {
  const q = query.trim().toLowerCase();
  const rows = !q
    ? browserExperiences
    : browserExperiences.filter((row) =>
        `${row.project_id} ${row.adr_summary} ${row.agent} ${row.tags.join(" ")}`
          .toLowerCase()
          .includes(q),
      );
  return rows.slice(0, limit);
}
