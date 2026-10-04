import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";

/** Safe session / file-id fragment (no path separators, no NUL). */
export const SESSION_ID_RE = /^[A-Za-z0-9._-]{1,64}$/;

/** @param {unknown} id */
export function isValidSessionId(id) {
  return typeof id === "string" && SESSION_ID_RE.test(id) && !id.includes("\0");
}

/**
 * Ensure resolvedFile stays inside resolvedDir (path.relative guard).
 * @param {string} resolvedDir
 * @param {string} resolvedFile
 */
export function assertPathInsideDir(resolvedDir, resolvedFile) {
  const rel = path.relative(resolvedDir, resolvedFile);
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    throw new Error(
      `Refusing path outside directory: file=${resolvedFile} dir=${resolvedDir}`,
    );
  }
  return rel;
}

/**
 * Append-only JSONL logger. One file per client/session label.
 * Cross-platform path handling via path.join / path.resolve.
 */
export class JsonlLogger {
  /**
   * @param {object} opts
   * @param {string} opts.logDir
   * @param {string} [opts.clientLabel]
   * @param {string} [opts.sessionId]
   * @param {(line: object) => void} [opts.onEvent] test hook
   */
  constructor(opts) {
    this.logDir = path.resolve(opts.logDir);
    this.clientLabel = sanitizeLabel(opts.clientLabel || "client");
    const rawSession = opts.sessionId || randomUUID();
    if (!isValidSessionId(rawSession)) {
      throw new Error(`Invalid session id for log filename: ${String(rawSession)}`);
    }
    this.sessionId = rawSession;
    this.onEvent = opts.onEvent || null;
    this.filePath = path.resolve(
      this.logDir,
      `${this.clientLabel}-${this.sessionId}.jsonl`,
    );
    assertPathInsideDir(this.logDir, this.filePath);
    fs.mkdirSync(this.logDir, { recursive: true });
    this.closed = false;
  }

  /** @param {string} event @param {Record<string, unknown>} [fields] */
  write(event, fields = {}) {
    if (this.closed) return;
    assertPathInsideDir(this.logDir, this.filePath);
    const line = {
      ts: new Date().toISOString(),
      event,
      session_id: this.sessionId,
      client_label: this.clientLabel,
      ...fields,
    };
    fs.appendFileSync(this.filePath, `${JSON.stringify(line)}\n`, "utf8");
    if (this.onEvent) this.onEvent(line);
    return line;
  }

  close(reason = "normal") {
    if (this.closed) return;
    this.write("session_end", { reason });
    this.closed = true;
  }
}

/** @param {string} label */
export function sanitizeLabel(label) {
  const cleaned = String(label)
    .trim()
    .replace(/[\\/]+/g, "_")
    .replace(/\.\.+/g, "_")
    .replace(/[^a-zA-Z0-9._-]+/g, "_")
    .replace(/_+/g, "_")
    .replace(/^[_.,-]+|[_.,-]+$/g, "")
    .slice(0, 64);
  return cleaned || "client";
}

/**
 * Parse a JSONL log file into events.
 * @param {string} filePath
 */
export function readJsonl(filePath) {
  const text = fs.readFileSync(filePath, "utf8");
  return text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => JSON.parse(l));
}

/**
 * Summarize probe events for reporting.
 * @param {object[]} events
 */
export function summarizeEvents(events) {
  const init = events.find((e) => e.event === "initialize");
  const cancelled = events.filter((e) => e.event === "cancelled");
  const closes = events.filter((e) => e.event === "connection_close");
  const reconnects = events.filter((e) => e.event === "connection_reconnect");
  const calls = events.filter((e) => e.event === "tools_call_end");
  const progressSupport = events.some(
    (e) => e.event === "progress_token_present" || e.progress_token != null,
  );
  const timeouts = events.filter((e) => e.event === "timeout_observed");

  const callSummaries = calls.map((c) => ({
    tool: c.tool,
    status: c.status,
    delay_ms: c.delay_ms ?? null,
    duration_ms: c.duration_ms ?? null,
    progress_token_present: Boolean(c.progress_token_present),
    progress_notifications_sent: c.progress_notifications_sent ?? 0,
  }));

  return {
    session_id: events[0]?.session_id ?? null,
    client_label: events[0]?.client_label ?? null,
    started_at: events[0]?.ts ?? null,
    ended_at: events.find((e) => e.event === "session_end")?.ts
      ?? events[events.length - 1]?.ts
      ?? null,
    clientInfo: init?.clientInfo ?? null,
    capabilities: init?.capabilities ?? null,
    protocolVersion: init?.protocolVersion ?? null,
    progress_token_supported: progressSupport,
    cancelled_count: cancelled.length,
    connection_close_count: closes.length,
    connection_reconnect_count: reconnects.length,
    timeout_seconds: timeouts.map((t) => t.elapsed_s),
    calls: callSummaries,
  };
}
