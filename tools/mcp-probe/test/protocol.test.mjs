import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { JsonlLogger } from "../src/logger.mjs";
import {
  CANCELLED_NO_RESPONSE,
  createSession,
  handleMessage,
  LATEST_PROTOCOL_VERSION,
  negotiateProtocolVersion,
  onConnectionClose,
  PROTOCOL_VERSIONS,
  toolDefs,
} from "../src/protocol.mjs";
import { CI_WAIT_MS, waitFor } from "./helpers.mjs";

function makeSession() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-proto-"));
  const sent = [];
  const logger = new JsonlLogger({
    logDir: dir,
    clientLabel: "proto",
    sessionId: "proto-1",
  });
  const session = createSession({
    logger,
    transport: "stdio",
    sessionId: "proto-1",
    send: (msg) => sent.push(msg),
  });
  return { session, sent, logger, dir };
}

test("toolDefs exposes ping and slow_echo", () => {
  const names = toolDefs().map((t) => t.name).sort();
  assert.deepEqual(names, ["ping", "slow_echo"]);
});

test("protocol negotiation falls back to newest supported", () => {
  assert.ok(PROTOCOL_VERSIONS.includes("2025-11-25"));
  assert.equal(LATEST_PROTOCOL_VERSION, "2025-11-25");
  assert.equal(negotiateProtocolVersion("2024-11-05"), "2024-11-05");
  assert.equal(negotiateProtocolVersion("2025-11-25"), "2025-11-25");
  assert.equal(negotiateProtocolVersion("1999-01-01"), LATEST_PROTOCOL_VERSION);
  assert.equal(negotiateProtocolVersion("unknown"), LATEST_PROTOCOL_VERSION);
});

test("initialize records clientInfo and capabilities", async () => {
  const { session, logger } = makeSession();
  const res = await handleMessage(session, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: { sampling: {} },
      clientInfo: { name: "TestClient", version: "9.9" },
    },
  });
  assert.equal(res.result.serverInfo.name, "mcp-probe");
  assert.equal(res.result.protocolVersion, "2024-11-05");
  assert.equal(session.clientInfo.name, "TestClient");
  assert.equal(session.capabilities.sampling !== undefined, true);
  const events = fs
    .readFileSync(logger.filePath, "utf8")
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l));
  assert.ok(events.some((e) => e.event === "initialize"));
});

test("initialize unknown version negotiates newest", async () => {
  const { session } = makeSession();
  const res = await handleMessage(session, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2099-01-01",
      capabilities: {},
      clientInfo: { name: "p", version: "1" },
    },
  });
  assert.equal(res.result.protocolVersion, LATEST_PROTOCOL_VERSION);
});

test("ping tool returns ok payload", async () => {
  const { session } = makeSession();
  await handleMessage(session, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "p", version: "1" },
    },
  });
  const res = await handleMessage(session, {
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: { name: "ping", arguments: { note: "hi" } },
  });
  const text = res.result.content[0].text;
  assert.match(text, /"ok":true/);
  assert.match(text, /"note":"hi"/);
});

test("slow_echo waits and optionally sends progress", async () => {
  const { session, sent } = makeSession();
  await handleMessage(session, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "p", version: "1" },
    },
  });
  const start = Date.now();
  const res = await handleMessage(session, {
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: {
      name: "slow_echo",
      arguments: { delay_ms: 80, progress: true, message: "x" },
      _meta: { progressToken: "tok-1" },
    },
  });
  const elapsed = Date.now() - start;
  assert.ok(elapsed >= 70, `expected >=70ms, got ${elapsed}`);
  assert.match(res.result.content[0].text, /"message":"x"/);
  assert.ok(
    sent.some((m) => m.method === "notifications/progress"),
    "expected progress notifications",
  );
});

test("cancelled notification does not send -32800 response", async () => {
  const { session, logger } = makeSession();
  await handleMessage(session, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "p", version: "1" },
    },
  });

  const callPromise = handleMessage(session, {
    jsonrpc: "2.0",
    id: 7,
    method: "tools/call",
    params: {
      name: "slow_echo",
      arguments: { delay_ms: 5000, progress: false },
    },
  });

  await waitFor(() => session.activeCalls.has(7), {
    timeoutMs: CI_WAIT_MS,
    label: "slow_echo registered",
  });
  await handleMessage(session, {
    jsonrpc: "2.0",
    method: "notifications/cancelled",
    params: { requestId: 7, reason: "test" },
  });

  const res = await callPromise;
  assert.equal(res, null, "SHOULD NOT respond to cancelled request");
  const events = fs
    .readFileSync(logger.filePath, "utf8")
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l));
  assert.ok(events.some((e) => e.event === "cancelled"));
  assert.ok(
    events.some(
      (e) =>
        e.event === "tools_call_end" &&
        (e.status === "cancelled_silent" || e.abort_reason === "client_cancelled"),
    ),
  );
  assert.ok(!events.some((e) => e.event === "error" && String(e.message).includes("-32800")));
});

test("connection close logs timeout_observed for active calls", async () => {
  const { session, logger } = makeSession();
  await handleMessage(session, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "p", version: "1" },
    },
  });
  const callPromise = handleMessage(session, {
    jsonrpc: "2.0",
    id: 3,
    method: "tools/call",
    params: { name: "slow_echo", arguments: { delay_ms: 10_000 } },
  });
  await waitFor(() => session.activeCalls.has(3), {
    timeoutMs: CI_WAIT_MS,
    label: "active call before close",
  });
  onConnectionClose(session, "test_drop");
  const res = await callPromise;
  assert.equal(res, null);
  assert.equal(CANCELLED_NO_RESPONSE.description, "mcp-probe-cancelled-no-response");
  const events = fs
    .readFileSync(logger.filePath, "utf8")
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l));
  assert.ok(events.some((e) => e.event === "connection_close"));
  assert.ok(events.some((e) => e.event === "timeout_observed"));
});
