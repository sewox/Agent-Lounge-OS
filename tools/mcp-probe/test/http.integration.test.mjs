import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { HttpFakeClient } from "../src/fake-client.mjs";
import { startHttpServer } from "../src/http-transport.mjs";
import { readJsonl, summarizeEvents } from "../src/logger.mjs";

test("http fake client: initialize, ping, slow_echo, session header", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-http-"));
  const { url, close } = await startHttpServer({
    logDir: dir,
    host: "127.0.0.1",
    port: 0,
    clientLabel: "http-int",
  });
  const client = new HttpFakeClient({
    baseUrl: url,
    clientName: "IntegrationHttp",
  });
  try {
    const init = await client.initialize();
    assert.equal(init.serverInfo.name, "mcp-probe");
    assert.ok(client.sessionId);

    const pong = await client.ping();
    assert.match(pong.content[0].text, /"ok":true/);

    const echo = await client.slowEcho({
      delayMs: 100,
      progress: true,
      message: "http-hi",
    });
    assert.match(echo.content[0].text, /http-hi/);
    assert.ok(
      client.notifications.some((n) => n.method === "notifications/progress"),
    );

    const healthUrl = url.replace(/\/mcp$/, "/health");
    const health = await (await fetch(healthUrl)).json();
    assert.equal(health.ok, true);
  } finally {
    await client.close();
    await close();
  }

  const logFile = fs.readdirSync(dir).find((f) => f.endsWith(".jsonl"));
  assert.ok(logFile);
  const summary = summarizeEvents(readJsonl(path.join(dir, logFile)));
  assert.equal(summary.clientInfo.name, "IntegrationHttp");
  assert.equal(summary.progress_token_supported, true);
});

test("http client abort mid slow_echo logs connection_close / timeout", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-http-ab-"));
  const { url, close } = await startHttpServer({
    logDir: dir,
    host: "127.0.0.1",
    port: 0,
    clientLabel: "http-abort",
  });
  const client = new HttpFakeClient({ baseUrl: url });
  try {
    await client.initialize();
    await assert.rejects(
      () =>
        client.slowEcho({
          delayMs: 5000,
          progress: true,
          timeoutMs: 150,
        }),
      (err) => err && err.code === "CLIENT_TIMEOUT",
    );
    // Give logger a moment to flush abort handlers
    await new Promise((r) => setTimeout(r, 50));
  } finally {
    await client.close();
    await close();
  }

  const logFile = fs.readdirSync(dir).find((f) => f.endsWith(".jsonl"));
  const events = readJsonl(path.join(dir, logFile));
  assert.ok(
    events.some(
      (e) =>
        e.event === "connection_close" ||
        e.event === "timeout_observed" ||
        e.event === "cancelled",
    ),
    `expected disconnect/cancel evidence, got: ${events.map((e) => e.event).join(",")}`,
  );
});
