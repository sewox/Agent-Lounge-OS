import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { HttpFakeClient } from "../src/fake-client.mjs";
import { startHttpServer } from "../src/http-transport.mjs";
import { readJsonl, summarizeEvents } from "../src/logger.mjs";
import { CI_WAIT_MS, waitFor } from "./helpers.mjs";

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

test("http rejects path-traversal Mcp-Session-Id (no log outside dir)", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-http-trav-"));
  const parent = path.dirname(dir);
  const { url, close, port } = await startHttpServer({
    logDir: dir,
    host: "127.0.0.1",
    port: 0,
    clientLabel: "http-trav",
  });
  try {
    const malicious = [
      "x/../../../escaped-poc",
      "..\\escaped-poc",
      "/tmp/abs-session",
    ];
    for (const sid of malicious) {
      const res = await fetch(url, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "mcp-session-id": sid,
          accept: "application/json",
        },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "tools/list",
        }),
      });
      assert.equal(res.status, 400, `expected 400 for sid=${JSON.stringify(sid)}`);
      const body = await res.json();
      assert.match(body.error, /invalid Mcp-Session-Id/i);
    }

    // NUL cannot be sent via fetch headers; covered in logger.isValidSessionId tests.

    // Unknown but syntactically valid session id without initialize → 404, no new log.
    const unknown = await fetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "mcp-session-id": "unknown-but-valid-id",
        accept: "application/json",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "tools/list",
      }),
    });
    assert.equal(unknown.status, 404);

    const escaped = path.join(parent, "escaped-poc.jsonl");
    const escapedAlt = path.join(parent, "http-trav-escaped-poc.jsonl");
    assert.equal(fs.existsSync(escaped), false, `must not create ${escaped}`);
    assert.equal(fs.existsSync(escapedAlt), false);
    // Only files inside logDir (none expected — no successful session).
    const inside = fs.readdirSync(dir).filter((f) => f.endsWith(".jsonl"));
    assert.equal(inside.length, 0, `unexpected logs: ${inside.join(",")}`);
    assert.ok(port > 0);
  } finally {
    await close();
  }
});

test("http rejects non-loopback Origin and oversized body", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-http-sec-"));
  const { close, port } = await startHttpServer({
    logDir: dir,
    host: "127.0.0.1",
    port: 0,
    maxBodyBytes: 1024,
  });
  const http = await import("node:http");
  try {
    const forbidden = await new Promise((resolve, reject) => {
      const req = http.request(
        {
          host: "127.0.0.1",
          port,
          path: "/mcp",
          method: "POST",
          headers: {
            "content-type": "application/json",
            origin: "https://evil.example",
            accept: "application/json",
            "content-length": 2,
          },
        },
        (res) => {
          const chunks = [];
          res.on("data", (c) => chunks.push(c));
          res.on("end", () =>
            resolve({ status: res.statusCode, body: Buffer.concat(chunks).toString("utf8") }),
          );
        },
      );
      req.on("error", reject);
      req.end("{}");
    });
    assert.equal(forbidden.status, 403);

    const bigBody = JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2025-03-26",
        capabilities: {},
        clientInfo: { name: "x".repeat(2048), version: "1" },
      },
    });
    assert.ok(Buffer.byteLength(bigBody) > 1024);
    const tooBig = await new Promise((resolve) => {
      const req = http.request(
        {
          host: "127.0.0.1",
          port,
          path: "/mcp",
          method: "POST",
          headers: {
            "content-type": "application/json",
            accept: "application/json",
            "content-length": Buffer.byteLength(bigBody),
          },
        },
        (res) => {
          const chunks = [];
          res.on("data", (c) => chunks.push(c));
          res.on("end", () =>
            resolve({ status: res.statusCode, body: Buffer.concat(chunks).toString("utf8") }),
          );
        },
      );
      req.on("error", (err) => resolve({ status: "conn_error", err: String(err) }));
      req.end(bigBody);
    });
    assert.ok(
      tooBig.status === 413 || tooBig.status === "conn_error",
      `unexpected oversized-body outcome: ${JSON.stringify(tooBig)}`,
    );
  } finally {
    await close();
  }
});

test("http POST SSE progress is not duplicated to GET SSE", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-http-sse-"));
  const { url, close } = await startHttpServer({
    logDir: dir,
    host: "127.0.0.1",
    port: 0,
  });
  try {
    const initRes = await fetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: "2025-03-26",
          capabilities: {},
          clientInfo: { name: "sse-dup", version: "1" },
        },
      }),
    });
    const sid = initRes.headers.get("mcp-session-id");
    assert.ok(sid);

    const getChunks = [];
    const ac = new AbortController();
    const getP = fetch(url, {
      method: "GET",
      headers: { "mcp-session-id": sid, accept: "text/event-stream" },
      signal: ac.signal,
    }).then(async (res) => {
      const reader = res.body.getReader();
      const dec = new TextDecoder();
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        getChunks.push(dec.decode(value));
      }
    }).catch(() => {});

    await waitFor(
      () => getChunks.join("").includes("event: ready"),
      { timeoutMs: CI_WAIT_MS, label: "GET SSE ready" },
    );

    const postRes = await fetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "text/event-stream",
        "mcp-session-id": sid,
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: {
          name: "slow_echo",
          arguments: { delay_ms: 120, progress: true, message: "dup" },
          _meta: { progressToken: "pt-dup" },
        },
      }),
    });
    const postText = await postRes.text();
    const postProgress = (postText.match(/notifications\/progress/g) || []).length;
    assert.ok(postProgress >= 1, "POST SSE should carry progress");

    // Wait until POST completed and any mistaken GET fan-out would have arrived.
    await waitFor(
      () => postProgress >= 1 && getChunks.join("").includes("event: ready"),
      { timeoutMs: CI_WAIT_MS, label: "POST progress + GET ready settled" },
    );
    const getText = getChunks.join("");
    assert.equal(
      (getText.match(/notifications\/progress/g) || []).length,
      0,
      "GET SSE must not duplicate POST progress",
    );
    ac.abort();
    await getP;
  } finally {
    await close();
  }
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
          e.event === "connection_close" ||
          e.event === "timeout_observed" ||
          e.event === "cancelled",
      );
    }, { timeoutMs: CI_WAIT_MS, label: "disconnect evidence in logs" });
  } finally {
    await client.close();
    await close();
  }
});
