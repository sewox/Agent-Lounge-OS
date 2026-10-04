import http from "node:http";
import { randomUUID } from "node:crypto";
import {
  createSession,
  handleMessage,
  onConnectionClose,
  parseRpc,
  LATEST_PROTOCOL_VERSION,
  SERVER_NAME,
  SERVER_VERSION,
} from "./protocol.mjs";
import { isValidSessionId, JsonlLogger } from "./logger.mjs";

const HDR_SESSION = "mcp-session-id";
const HDR_PROTOCOL = "mcp-protocol-version";
export const MAX_BODY_BYTES = 1 * 1024 * 1024; // 1 MiB
export const MAX_SESSIONS = 64;

/**
 * Streamable HTTP + SSE MCP probe server.
 *
 * - POST /mcp  — JSON-RPC (JSON or SSE response when progress/long-running)
 * - GET  /mcp  — SSE stream for server→client messages (GET channel only)
 * - DELETE /mcp — end session
 * - GET /health — health
 *
 * Progress/notifications during a POST SSE response go ONLY to that POST stream
 * (not duplicated onto GET SSE). GET SSE receives messages only when session.send
 * is the default fan-out (no active POST SSE response).
 *
 * @param {object} opts
 * @param {string} opts.logDir
 * @param {string} [opts.host]
 * @param {number} [opts.port]
 * @param {string} [opts.clientLabel]
 * @param {number} [opts.maxSessions]
 * @param {number} [opts.maxBodyBytes]
 * @returns {Promise<{server: import('node:http').Server, url: string, close: () => Promise<void>}>}
 */
export async function startHttpServer(opts) {
  const host = opts.host || "127.0.0.1";
  const port = opts.port ?? 19891;
  const logDir = opts.logDir;
  const defaultLabel = opts.clientLabel || process.env.MCP_PROBE_CLIENT || "http";
  const maxSessions = opts.maxSessions ?? MAX_SESSIONS;
  const maxBodyBytes = opts.maxBodyBytes ?? MAX_BODY_BYTES;

  /** @type {Map<string, ReturnType<typeof makeHttpSession>>} */
  const sessions = new Map();

  function makeHttpSession(sessionId, clientLabel) {
    /** @type {import('node:http').ServerResponse[]} */
    const sseClients = [];
    /** Active POST SSE response, if any — progress goes here exclusively. */
    /** @type {import('node:http').ServerResponse|null} */
    let postSse = null;

    const logger = new JsonlLogger({
      logDir,
      clientLabel,
      sessionId,
    });

    const sendToGetSse = (msg) => {
      const data = `event: message\ndata: ${JSON.stringify(msg)}\n\n`;
      for (const res of [...sseClients]) {
        try {
          res.write(data);
        } catch {
          /* drop */
        }
      }
    };

    const send = (msg) => {
      if (postSse && !postSse.writableEnded) {
        try {
          postSse.write(`event: message\ndata: ${JSON.stringify(msg)}\n\n`);
        } catch {
          /* drop */
        }
        return;
      }
      sendToGetSse(msg);
    };

    const session = createSession({
      logger,
      transport: "http",
      sessionId,
      send,
    });

    return {
      session,
      logger,
      sseClients,
      send,
      get postSse() {
        return postSse;
      },
      setPostSse(res) {
        postSse = res;
      },
      clearPostSse(res) {
        if (postSse === res) postSse = null;
      },
    };
  }

  /**
   * @param {import('node:http').IncomingMessage} req
   * @param {boolean} isInitialize
   * @returns {{ entry: ReturnType<typeof makeHttpSession> } | { httpError: number, message: string }}
   */
  function getOrCreateSession(req, isInitialize) {
    const headerId = header(req, HDR_SESSION);

    if (headerId) {
      if (!isValidSessionId(headerId)) {
        return { httpError: 400, message: "invalid Mcp-Session-Id" };
      }
      if (sessions.has(headerId)) {
        const existing = sessions.get(headerId);
        existing.session.lastSeenAt = Date.now();
        if (existing.session.closed) {
          existing.session.closed = false;
          existing.logger.write("connection_reconnect", {
            previous_session_id: headerId,
            reconnected_at: new Date().toISOString(),
          });
        }
        return { entry: existing };
      }
      // Unknown session id: never open a new logger for non-initialize traffic.
      if (!isInitialize) {
        return { httpError: 404, message: "unknown session" };
      }
    } else if (!isInitialize) {
      return { httpError: 400, message: "Mcp-Session-Id required" };
    }

    if (sessions.size >= maxSessions) {
      return { httpError: 503, message: "too many sessions" };
    }

    const sessionId = headerId || randomUUID();
    if (!isValidSessionId(sessionId)) {
      return { httpError: 500, message: "failed to allocate session id" };
    }
    const created = makeHttpSession(sessionId, defaultLabel);
    sessions.set(sessionId, created);
    return { entry: created };
  }

  const server = http.createServer(async (req, res) => {
    try {
      if (!assertLocalRequest(req, res)) return;
      await route(req, res);
    } catch (err) {
      const code = err && typeof err === "object" && "httpCode" in err
        ? /** @type {any} */ (err).httpCode
        : 500;
      if (!res.headersSent) {
        res.writeHead(code, { "content-type": "application/json" });
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
        max_sessions: maxSessions,
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
      if (!sid || !isValidSessionId(sid) || !sessions.has(sid)) {
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
    if (!sid || !isValidSessionId(sid) || !sessions.has(sid)) {
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
    entry.logger.write("sse_attached", { channel: "get" });

    const onClose = () => {
      const idx = entry.sseClients.indexOf(res);
      if (idx >= 0) entry.sseClients.splice(idx, 1);
      entry.logger.write("sse_detached", {
        channel: "get",
        remaining_sse_clients: entry.sseClients.length,
      });
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
    let body;
    try {
      body = await readBody(req, maxBodyBytes);
    } catch (err) {
      const code = /** @type {any} */ (err).httpCode || 400;
      json(res, code, { error: err instanceof Error ? err.message : String(err) });
      return;
    }

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
    const resolved = getOrCreateSession(req, isInitialize);
    if ("httpError" in resolved) {
      json(res, resolved.httpError, { error: resolved.message });
      return;
    }
    const entry = resolved.entry;
    const sid = entry.session.sessionId;

    const labelHdr = header(req, "x-mcp-probe-client");
    if (labelHdr && isValidSessionId(labelHdr.replace(/[^A-Za-z0-9._-]/g, "_").slice(0, 64))) {
      // label is not session id; keep sanitize via logger API — only override display label safely
      entry.logger.clientLabel = labelHdr.replace(/[^A-Za-z0-9._-]+/g, "_").slice(0, 64) || entry.logger.clientLabel;
    }

    const accept = (header(req, "accept") || "").toLowerCase();
    const wantsSse = accept.includes("text/event-stream");
    const proto = entry.session.protocolVersion || LATEST_PROTOCOL_VERSION;

    const onlyNotifications = messages.every(
      (m) => m && (m.id === undefined || m.id === null),
    );
    if (onlyNotifications) {
      for (const msg of messages) {
        await handleMessage(entry.session, msg);
      }
      res.writeHead(202, {
        [HDR_SESSION]: sid,
        [HDR_PROTOCOL]: proto,
      });
      res.end();
      return;
    }

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
        [HDR_PROTOCOL]: proto,
      });

      // Route session.send exclusively to this POST SSE (no GET-SSE duplicate).
      entry.setPostSse(res);

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
          for (const [rid, call] of [...entry.session.activeCalls]) {
            entry.logger.write("timeout_observed", {
              request_id: rid,
              tool: call.tool,
              elapsed_s: Math.round((Date.now() - call.startedAt) / 1000),
              elapsed_ms: Date.now() - call.startedAt,
              expected_delay_ms: call.delayMs,
              note: "client aborted HTTP SSE while call in flight",
            });
            call.abortReason = "http_post_sse_client_abort";
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
        entry.clearPostSse(res);
        req.off("close", onReqClose);
        res.end();
      }
      return;
    }

    const responses = [];
    for (const msg of messages) {
      const response = await handleMessage(entry.session, msg);
      if (response) responses.push(response);
    }
    const payload = responses.length === 1 ? responses[0] : responses;
    res.writeHead(200, {
      "content-type": "application/json",
      [HDR_SESSION]: sid,
      [HDR_PROTOCOL]: proto,
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

/** @param {string} hostname */
export function isLoopbackHost(hostname) {
  const h = String(hostname || "").toLowerCase().replace(/^\[|\]$/g, "");
  return h === "localhost" || h === "127.0.0.1" || h === "::1" || h === "0.0.0.0";
}

/**
 * Reject non-loopback Origin / Host (probe is localhost-only).
 * @param {import('node:http').IncomingMessage} req
 * @param {import('node:http').ServerResponse} res
 */
export function assertLocalRequest(req, res) {
  const origin = header(req, "origin");
  if (origin) {
    try {
      const u = new URL(origin);
      if (!isLoopbackHost(u.hostname)) {
        json(res, 403, { error: "origin not allowed" });
        return false;
      }
    } catch {
      json(res, 403, { error: "invalid origin" });
      return false;
    }
  }

  const hostHdr = header(req, "host");
  if (hostHdr) {
    const hostname = hostHdr.split(":")[0];
    if (hostname && !isLoopbackHost(hostname)) {
      json(res, 403, { error: "host not allowed" });
      return false;
    }
  }
  return true;
}

/** @param {import('node:http').IncomingMessage} req @param {string} name */
function header(req, name) {
  const v = req.headers[name.toLowerCase()];
  if (Array.isArray(v)) return v[0]?.trim() || "";
  return (v || "").trim();
}

/**
 * @param {import('node:http').IncomingMessage} req
 * @param {number} maxBytes
 */
export function readBody(req, maxBytes = MAX_BODY_BYTES) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    let done = false;
    req.on("data", (c) => {
      if (done) return;
      size += c.length;
      if (size > maxBytes) {
        done = true;
        const err = new Error(`body too large (max ${maxBytes} bytes)`);
        /** @type {any} */ (err).httpCode = 413;
        reject(err);
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on("end", () => {
      if (done) return;
      done = true;
      resolve(Buffer.concat(chunks).toString("utf8"));
    });
    req.on("error", (err) => {
      if (done) return;
      done = true;
      reject(err);
    });
  });
}

/** @param {import('node:http').ServerResponse} res @param {number} code @param {object} body */
function json(res, code, body) {
  res.writeHead(code, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}
