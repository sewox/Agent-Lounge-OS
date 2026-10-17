import type { LoungeHealthReportFixture } from "./types";

const checkedAt = "2026-10-17T12:00:00.000Z";

const baseTitle = {
  queue_backlog: 0,
  db_backlog: 1,
  give_ups: 0,
  last_error_type: null,
  lmr_reachable: true,
  model_installed: true,
  model: "fixture-model",
  lmr_endpoint: "http://127.0.0.1:18790",
  probe_error_type: null,
};

const baseCbm = {
  daemon_mode: false,
  daemon_presence: "inactive",
  polluted_18749: false,
  config_path: "~/.cache/codebase-memory-mcp/config.json",
  ui_enabled: false,
  ui_port: null as number | null,
};

const basePlatform = {
  os: "linux",
  arch: "x86_64",
  distro_id: "ubuntu",
  distro_id_like: "debian",
  package_manager: "apt",
};

/** All 8 AgentMcpStatus values — paths already masked (backend style). */
export const HEALTH_AGENTS_ALL_STATUSES: LoungeHealthReportFixture = {
  schema_version: "1.1.0",
  checked_at: checkedAt,
  platform: basePlatform,
  agents: [
    {
      client: "cursor",
      status: "ok",
      severity: "ok",
      config_path: "~/.cursor/mcp.json",
      command: "~/bin/lounge-mcp",
      configured_version: "0.1.0",
      expected_version: "0.1.0",
      configured_sha256: "aaa",
      expected_sha256: "aaa",
      detail: null,
    },
    {
      client: "antigravity",
      status: "missing",
      severity: "warning",
      config_path: "~/.gemini/antigravity/mcp_config.json",
      detail: "lounge-mcp entry absent",
    },
    {
      client: "claude_desktop",
      status: "stale",
      severity: "warning",
      config_path: "~/Library/Application Support/Claude/claude_desktop_config.json",
      command: "~/bin/lounge-mcp",
      configured_version: "0.0.9",
      expected_version: "0.1.0",
      detail: "configured version behind expected",
    },
    {
      client: "claude_code",
      status: "hash_mismatch",
      severity: "info",
      config_path: "~/.claude.json",
      command: "~/bin/lounge-mcp",
      configured_version: "0.1.0",
      expected_version: "0.1.0",
      configured_sha256: "bbb",
      expected_sha256: "aaa",
      detail: "sha256 mismatch",
    },
    {
      client: "grok_bot",
      status: "non_canonical",
      severity: "warning",
      config_path: "~/.grok/mcp.json",
      command: "not-lounge-mcp",
      detail: "command is not lounge-mcp",
    },
    {
      client: "vscode",
      status: "unreadable",
      severity: "warning",
      config_path: "~/.config/Code/User/mcp.json",
      detail: "read failed",
    },
    {
      client: "windsurf",
      status: "not_installed",
      severity: "warning",
      detail: "client not installed",
    },
    {
      client: "cursor-remote",
      status: "unverified",
      severity: "info",
      config_path: "~/.cursor/mcp-remote.json",
      detail: "url-only entry, not probed",
    },
  ],
  title: baseTitle,
  dead_symbols: { kritik: 2, potansiyel: 5, total: 7, ignored: 0 },
  cbm: baseCbm,
  expected_lounge_mcp_version: "0.1.0",
  expected_lounge_mcp_sha256: "aaa",
};

/**
 * Fixture with *unmasked* home paths on path-like fields only.
 * Free-text `detail` stays free of absolute homes (UI masks paths, not prose).
 */
export const HEALTH_AGENTS_UNMASKED_PATH: LoungeHealthReportFixture = {
  ...HEALTH_AGENTS_ALL_STATUSES,
  agents: [
    {
      client: "cursor",
      status: "unverified",
      severity: "info",
      config_path: "/Users/sercan/.cursor/mcp.json",
      detail: "url-only entry, not probed",
    },
    {
      client: "claude_desktop",
      status: "ok",
      severity: "ok",
      config_path: "C:\\Users\\sercan\\AppData\\Roaming\\Claude\\claude_desktop_config.json",
      command: "C:\\Users\\sercan\\bin\\lounge-mcp.exe",
      configured_version: "0.1.0",
      expected_version: "0.1.0",
    },
    {
      client: "grok_bot",
      status: "missing",
      severity: "warning",
      config_path: "/home/sercan/.grok/mcp.json",
      detail: "lounge-mcp entry absent",
    },
  ],
  cbm: {
    ...baseCbm,
    config_path: "/home/sercan/.cache/codebase-memory-mcp/config.json",
  },
};

export const HEALTH_AGENTS_EMPTY: LoungeHealthReportFixture = {
  schema_version: "1.1.0",
  checked_at: checkedAt,
  platform: basePlatform,
  agents: [],
  title: baseTitle,
  dead_symbols: { kritik: 0, potansiyel: 0, total: 0, ignored: 0 },
  cbm: baseCbm,
  expected_lounge_mcp_version: "0.1.0",
  expected_lounge_mcp_sha256: null,
};

export const HEALTH_SCAN_PARTIAL = {
  partial: true,
  indexed_at: "2020-01-01T00:00:00.000Z",
};
