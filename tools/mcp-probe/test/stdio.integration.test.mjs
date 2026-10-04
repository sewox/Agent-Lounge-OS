import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { StdioFakeClient } from "../src/fake-client.mjs";
import { readJsonl, summarizeEvents } from "../src/logger.mjs";
import { CI_WAIT_MS, waitFor } from "./helpers.mjs";

test("stdio fake client: initialize + ping + slow_echo with progress", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-stdio-"));
  const client = new StdioFakeClient({
    logDir: dir,
    clientLabel: "stdio-int",
    clientName: "IntegrationStdio",
    clientVersion: "1.2.3",
    capabilities: { elicitation: {} },
  });
  try {
    await client.start();
    const pong = await client.ping("roundtrip");
    assert.match(pong.content[0].text, /roundtrip/);

    const echo = await client.slowEcho({
      delayMs: 100,
      progress: true,
      message: "hello",
    });
    assert.match(echo.content[0].text, /hello/);
    assert.ok(
      client.notifications.some((n) => n.method === "notifications/progress"),
    );
  } finally {
    await client.close();
  }

  const logFile = fs.readdirSync(dir).find((f) => f.endsWith(".jsonl"));
  assert.ok(logFile);
  const summary = summarizeEvents(readJsonl(path.join(dir, logFile)));
  assert.equal(summary.clientInfo.name, "IntegrationStdio");
  assert.equal(summary.clientInfo.version, "1.2.3");
  assert.ok(summary.capabilities.elicitation !== undefined);
  assert.equal(summary.progress_token_supported, true);
  assert.ok(summary.calls.some((c) => c.tool === "slow_echo" && c.status === "ok"));
});

test("stdio fake client: timeout cancels long-running call", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-stdio-to-"));
  const client = new StdioFakeClient({
    logDir: dir,
    clientLabel: "stdio-timeout",
  });
  try {
    await client.start();
    await assert.rejects(
      () =>
        client.slowEcho({
          delayMs: 5000,
          progress: false,
          timeoutMs: 200,
        }),
      (err) => err && err.code === "CLIENT_TIMEOUT",
    );
    await waitFor(() => {
      const logFile = fs.readdirSync(dir).find((f) => f.endsWith(".jsonl"));
      if (!logFile) return false;
      const events = readJsonl(path.join(dir, logFile));
      return events.some(
        (e) =>
          e.event === "cancelled" ||
          (e.event === "tools_call_end" &&
            (e.status === "cancelled_silent" || e.status === "aborted_silent")) ||
          e.event === "timeout_observed" ||
          e.event === "call_aborted" ||
          e.event === "connection_close",
      );
    }, { timeoutMs: CI_WAIT_MS, label: "cancel/disconnect log evidence" });
  } finally {
    await client.close();
  }
});
