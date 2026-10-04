import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  JsonlLogger,
  readJsonl,
  sanitizeLabel,
  summarizeEvents,
} from "../src/logger.mjs";

test("sanitizeLabel strips unsafe chars", () => {
  assert.equal(sanitizeLabel("Cursor IDE"), "Cursor_IDE");
  assert.equal(sanitizeLabel("../../../x"), "x");
  assert.equal(sanitizeLabel(""), "client");
});

test("JsonlLogger writes session events and summarizeEvents extracts fields", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-log-"));
  const logger = new JsonlLogger({
    logDir: dir,
    clientLabel: "Unit Client",
    sessionId: "sess-1",
  });
  logger.write("session_start", { transport: "stdio" });
  logger.write("initialize", {
    clientInfo: { name: "unit", version: "1" },
    capabilities: { tools: {} },
    protocolVersion: "2024-11-05",
  });
  logger.write("progress_token_present", { progress_token: "pt-1" });
  logger.write("cancelled", { request_id: 2 });
  logger.write("connection_close", { reason: "test" });
  logger.write("timeout_observed", { elapsed_s: 30, request_id: 2 });
  logger.write("tools_call_end", {
    tool: "slow_echo",
    status: "cancelled",
    delay_ms: 60000,
    duration_ms: 30000,
    progress_token_present: true,
    progress_notifications_sent: 3,
  });
  logger.close("done");

  assert.ok(fs.existsSync(logger.filePath));
  const events = readJsonl(logger.filePath);
  assert.ok(events.some((e) => e.event === "session_start"));
  assert.ok(events.some((e) => e.event === "session_end"));

  const summary = summarizeEvents(events);
  assert.equal(summary.clientInfo.name, "unit");
  assert.equal(summary.progress_token_supported, true);
  assert.equal(summary.cancelled_count, 1);
  assert.equal(summary.connection_close_count, 1);
  assert.deepEqual(summary.timeout_seconds, [30]);
  assert.equal(summary.calls[0].tool, "slow_echo");
});
