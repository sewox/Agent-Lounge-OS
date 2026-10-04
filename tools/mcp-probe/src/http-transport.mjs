import http from "node:http";
import { randomUUID } from "node:crypto";
import {
  createSession,
  handleMessage,
  onConnectionClose,
  parseRpc,
  SERVER_NAME,
  SERVER_VERSION,
} from "./protocol.mjs";
import { JsonlLogger } from "./logger.mjs";

const HDR_SESSION = "mcp-session-id";
const HDR_PROTOCOL = "mcp-protocol-version";

/**
 * Streamable HTTP + SSE MCP probe server.
 *
 * - POST /mcp  — JSON-RPC (JSON or SSE response when progress/long-running)
 * - GET  /mcp  — SSE stream for server→client messages
 * - DELETE /mcp — end session
 * - GET /health — health
 *
 * @param {object} opts
 * @param {string} opts.logDir
 * @param {string} [opts.host]
 * @param {number} [opts.port]
 * @param {string} [opts.clientLabel]
 * @returns {Promise<{server: import('node:http').Server, url: string, close: () => Promise<void>}>}
 */
export async function startHttpServer(opts) {
  const host = opts.host || "127.0.0.1";
  const port = opts.port ?? 19891;
  const logDir = opts.logDir;
  const defaultLabel = opts.clientLabel || process.env.MCP_PROBE_CLIENT || "http";

  /** @type {Map<string, ReturnType<typeof makeHttpSession>>} */
  const sessions = new Map();

  function makeHttpSession(sessionId, clientLabel) {
    /** @type {import('node:http').ServerResponse[]} */
    const sseClients = [];
    const logger = new JsonlLogger({
      logDir,
      clientLabel,
      sessionId,
    });

    const send = (msg) => {
      const data = `event: message\ndata: ${JSON.stringify(msg)}\n\n`;
      for (const res of [...sseClients]) {
        try {
          res.write(data);
        } catch {
          /* drop */
        }
      }
    };

    const session = createSession({
      logger,
      transport: "http",
      sessionId,
      send,
    });

    return { session, logger, sseClients, send };
  }

  function getOrCreateSession(req, isInitialize) {
    const headerId = header(req, HDR_SESSION);
    if (headerId && sessions.has(headerId)) {
      const existing = sessions.get(headerId);
      existing.session.lastSeenAt = Date.now();
      if (existing.session.closed) {
        // Reopen bookkeeping for reconnect observation
        existing.session.closed = false;
        existing.logger.write("connection_reconnect", {
          previous_session_id: headerId,
          reconnected_at: new Date().toISOString(),
        });
      }
      return existing;
    }
    if (headerId && !sessions.has(headerId) && !isInitialize) {
      // Unknown session id on non-initialize → new session tagged as reconnect attempt
      const created = makeHttpSession(headerId, defaultLabel);
      created.logger.write("connection_reconnect", {
        previous_session_id: headerId,
        note: "client presented session id unknown to server; new session created",
        reconnected_at: new Date().toISOString(),
      });
      sessions.set(headerId, created);
      return created;
    }
    const sessionId = headerId || randomUUID();
    const created = makeHttpSession(sessionId, defaultLabel);
    sessions.set(sessionId, created);
    return created;
  }

  const server = http.createServer(async (req, res) => {
    try {
      await route(req, res);
    } catch (err) {
      if (!res.headersSent) {
        res.writeHead(500, { "content-type": "application/json" });
      }
      res.end(
        JSON.stringify({
          error: err instanceof Error ? err.message : String(err),
        }),
      );
    }
  });

  async function route(req, res) {
    const url = new URL(req.url || "/", `http://${host}:${port}`);
    const pathname = url.pathname.replace(/\/+$/, "") || "/";

    if (req.method === "GET" && (pathname === "/health" || pathname === "/mcp/health")) {
      json(res, 200, {
        ok: true,
        server: SERVER_NAME,
        version: SERVER_VERSION,
        transport: "http",
        sessions: sessions.size,
      });
      return;
    }

    if (pathname !== "/mcp") {
      json(res, 404, { error: "not found" });
      return;
    }

    if (req.method === "GET") {
      handleSseGet(req, res);
      return;
    }

    if (req.method === "DELETE") {
      const sid = header(req, HDR_SESSION);
      if (!sid || !sessions.has(sid)) {
        json(res, 404, { error: "unknown session" });
        return;
      }
      const entry = sessions.get(sid);
      for (const client of entry.sseClients) {
        try {
          client.end();
        } catch {
          /* ignore */
        }
      }
      entry.sseClients.length = 0;
      onConnectionClose(entry.session, "client_delete");
      sessions.delete(sid);
      res.writeHead(204);
      res.end();
      return;
    }

    if (req.method === "POST") {
      await handlePost(req, res);
      return;
    }

    res.writeHead(405, { allow: "GET, POST, DELETE" });
    res.end("method not allowed");
  }

  function handleSseGet(req, res) {
    const sid = header(req, HDR_SESSION);
    if (!sid || !sessions.has(sid)) {
      json(res, 400, { error: "Mcp-Session-Id required for SSE" });
      return;
    }
    const entry = sessions.get(sid);
    res.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
      connection: "keep-alive",
      [HDR_SESSION]: sid,
    });
    res.write(`event: ready\ndata: ${JSON.stringify({ ok: true, session_id: sid })}\n\n`);
    entry.sseClients.push(res);
    entry.logger.write("sse_attached", {});

    const onClose = () => {
      const idx = entry.sseClients.indexOf(res);
      if (idx >= 0) entry.sseClients.splice(idx, 1);
      entry.logger.write("sse_detached", {
        remaining_sse_clients: entry.sseClients.length,
      });
      // If no SSE left and there are active long calls, note possible mid-call drop
      if (entry.sseClients.length === 0 && entry.session.activeCalls.size > 0) {
        entry.logger.write("connection_close", {
          reason: "sse_drop_during_active_call",
          closed_at: new Date().toISOString(),
          active_calls: [...entry.session.activeCalls.values()].map((c) => ({
            request_id: c.requestId,
            tool: c.tool,
            elapsed_ms: Date.now() - c.startedAt,
          })),
        });
      }
    };
    req.on("close", onClose);
  }

  async function handlePost(req, res) {
    const body = await readBody(req);
    let messages;
    try {
      messages = parseRpc(body);
    } catch (err) {
      json(res, 400, {
        jsonrpc: "2.0",
        id: null,
        error: {
          code: -32700,
          message: `Parse error: ${err instanceof Error ? err.message : err}`,
        },
      });
      return;
    }

    const isInitialize = messages.some((m) => m?.method === "initialize");
    const entry = getOrCreateSession(req, isInitialize);
    const sid = entry.session.sessionId;

    // Label override from header for multi-client HTTP probes
    const labelHdr = header(req, "x-mcp-probe-client");
    if (labelHdr) {
      entry.logger.clientLabel = labelHdr;
    }

    const accept = (header(req, "accept") || "").toLowerCase();
    const wantsSse = accept.includes("text/event-stream");

    // Notifications-only POST (e.g. cancelled) — 202
    const onlyNotifications = messages.every(
      (m) => m && (m.id === undefined || m.id === null),
    );
    if (onlyNotifications) {
      for (const msg of messages) {
        await handleMessage(entry.session, msg);
      }
      res.writeHead(202, {
        [HDR_SESSION]: sid,
        [HDR_PROTOCOL]: entry.session.protocolVersion || "2024-11-05",
      });
      res.end();
      return;
    }

    // If any tools/call with progress or delay, prefer SSE response stream
    const longRunning = messages.some(
      (m) =>
        m?.method === "tools/call" &&
        (m?.params?.arguments?.progress === true ||
          Number(m?.params?.arguments?.delay_ms) > 0),
    );

    if (wantsSse || longRunning) {
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
        [HDR_SESSION]: sid,
        [HDR_PROTOCOL]: entry.session.protocolVersion || "2024-11-05",
      });

      const originalSend = entry.send;
      const streamSend = (msg) => {
        res.write(`event: message\ndata: ${JSON.stringify(msg)}\n\n`);
        originalSend(msg);
      };
      entry.session.send = streamSend;

      const onReqClose = () => {
        if (entry.session.activeCalls.size > 0) {
          entry.logger.write("connection_close", {
            reason: "http_post_sse_client_abort",
            closed_at: new Date().toISOString(),
            active_calls: [...entry.session.activeCalls.values()].map((c) => ({
              request_id: c.requestId,
              tool: c.tool,
              elapsed_ms: Date.now() - c.startedAt,
              delay_ms: c.delayMs,
            })),
          });
          for (const [rid, call] of entry.session.activeCalls) {
            entry.logger.write("timeout_observed", {
              request_id: rid,
              tool: call.tool,
              elapsed_s: Math.round((Date.now() - call.startedAt) / 1000),
              elapsed_ms: Date.now() - call.startedAt,
              expected_delay_ms: call.delayMs,
              note: "client aborted HTTP SSE while call in flight",
            });
            call.abort.abort();
          }
          entry.session.activeCalls.clear();
        }
      };
      req.on("close", onReqClose);

      try {
        for (const msg of messages) {
          const response = await handleMessage(entry.session, msg);
          if (response) {
            res.write(`event: message\ndata: ${JSON.stringify(response)}\n\n`);
          }
        }
      } finally {
        entry.session.send = originalSend;
        req.off("close", onReqClose);
        res.end();
      }
      return;
    }

    // Simple JSON response (initialize, ping, tools/list, short calls)
    const responses = [];
    for (const msg of messages) {
      const response = await handleMessage(entry.session, msg);
      if (response) responses.push(response);
    }
    const payload = responses.length === 1 ? responses[0] : responses;
    res.writeHead(200, {
      "content-type": "application/json",
      [HDR_SESSION]: sid,
      [HDR_PROTOCOL]: entry.session.protocolVersion || "2024-11-05",
    });
    res.end(JSON.stringify(payload));
  }

  await new Promise((resolve, reject) => {
    server.listen(port, host, (err) => (err ? reject(err) : resolve()));
  });

  const addr = server.address();
  const boundPort = typeof addr === "object" && addr ? addr.port : port;
  const url = `http://${host}:${boundPort}/mcp`;

  const close = () =>
    new Promise((resolve, reject) => {
      for (const entry of sessions.values()) {
        if (!entry.session.closed) {
          onConnectionClose(entry.session, "server_shutdown");
        }
      }
      sessions.clear();
      server.close((err) => (err ? reject(err) : resolve()));
    });

  return { server, url, host, port: boundPort, close, sessions };
}

/** @param {import('node:http').IncomingMessage} req @param {string} name */
function header(req, name) {
  const v = req.headers[name.toLowerCase()];
  if (Array.isArray(v)) return v[0]?.trim() || "";
  return (v || "").trim();
}

/** @param {import('node:http').IncomingMessage} req */
function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

/** @param {import('node:http').ServerResponse} res @param {number} code @param {object} body */
function json(res, code, body) {
  res.writeHead(code, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}
