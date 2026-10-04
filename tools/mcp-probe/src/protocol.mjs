import { randomUUID } from "node:crypto";

export const SERVER_NAME = "mcp-probe";
export const SERVER_VERSION = "0.1.0";

/** Supported protocol versions, oldest → newest. */
export const PROTOCOL_VERSIONS = [
  "2024-11-05",
  "2025-03-26",
  "2025-06-18",
  "2025-11-25",
];

/** Fallback when the client requests an unknown version: newest supported. */
export const LATEST_PROTOCOL_VERSION =
  PROTOCOL_VERSIONS[PROTOCOL_VERSIONS.length - 1];

/** @deprecated use LATEST_PROTOCOL_VERSION — kept as alias for newest. */
export const DEFAULT_PROTOCOL_VERSION = LATEST_PROTOCOL_VERSION;

/** Sentinel: cancelled request must not produce a JSON-RPC response (MCP SHOULD NOT). */
export const CANCELLED_NO_RESPONSE = Symbol("mcp-probe-cancelled-no-response");

/**
 * @param {string} requested
 * @returns {string}
 */
export function negotiateProtocolVersion(requested) {
  if (typeof requested === "string" && PROTOCOL_VERSIONS.includes(requested)) {
    return requested;
  }
  return LATEST_PROTOCOL_VERSION;
}

/** @typedef {{
 *   sessionId: string,
 *   transport: 'stdio'|'http',
 *   initialized: boolean,
 *   clientInfo: object|null,
 *   capabilities: object|null,
 *   protocolVersion: string|null,
 *   activeCalls: Map<string|number, ActiveCall>,
 *   logger: import('./logger.mjs').JsonlLogger,
 *   send: (msg: object) => void|Promise<void>,
 *   closed: boolean,
 *   lastSeenAt: number,
 * }} Session */

/** @typedef {{
 *   requestId: string|number,
 *   tool: string,
 *   startedAt: number,
 *   delayMs: number,
 *   progress: boolean,
 *   progressToken: string|number|null,
 *   progressSent: number,
 *   abort: AbortController,
 *   abortReason: string|null,
 *   timer: NodeJS.Timeout|null,
 *   progressTimer: NodeJS.Timeout|null,
 *   finished: boolean,
 * }} ActiveCall */

export function toolDefs() {
  return [
    {
      name: "ping",
      description:
        "Immediate round-trip. Use to verify the probe server is reachable.",
      inputSchema: {
        type: "object",
        properties: {
          note: { type: "string", description: "Optional note echoed back." },
        },
        additionalProperties: false,
      },
    },
    {
      name: "slow_echo",
      description:
        "Delayed echo for measuring client tool-timeout / progress / cancel behavior. " +
        "Waits delay_ms then returns the message. When progress=true and the client " +
        "supplies _meta.progressToken, sends notifications/progress heartbeats.",
      inputSchema: {
        type: "object",
        properties: {
          delay_ms: {
            type: "integer",
            minimum: 0,
            maximum: 600_000,
            description: "How long to wait before responding (milliseconds).",
          },
          message: {
            type: "string",
            description: "Payload to echo back (default: 'echo').",
          },
          progress: {
            type: "boolean",
            description:
              "If true, emit progress notifications while waiting (requires client progressToken).",
          },
        },
        required: ["delay_ms"],
        additionalProperties: false,
      },
    },
  ];
}

/**
 * Create a session object.
 * @param {object} opts
 * @param {import('./logger.mjs').JsonlLogger} opts.logger
 * @param {'stdio'|'http'} opts.transport
 * @param {(msg: object) => void|Promise<void>} opts.send
 * @param {string} [opts.sessionId]
 */
export function createSession(opts) {
  const sessionId = opts.sessionId || randomUUID();
  /** @type {Session} */
  const session = {
    sessionId,
    transport: opts.transport,
    initialized: false,
    clientInfo: null,
    capabilities: null,
    protocolVersion: null,
    activeCalls: new Map(),
    logger: opts.logger,
    send: opts.send,
    closed: false,
    lastSeenAt: Date.now(),
  };
  session.logger.write("session_start", {
    transport: opts.transport,
    started_at: new Date().toISOString(),
  });
  return session;
}

/**
 * Handle one JSON-RPC message (request or notification).
 * @param {Session} session
 * @param {object} msg
 * @returns {Promise<object|null>} response or null for notifications
 */
export async function handleMessage(session, msg) {
  session.lastSeenAt = Date.now();

  if (!msg || typeof msg !== "object") {
    return rpcError(null, -32700, "Parse error");
  }

  const { id, method, params } = msg;

  if (typeof method !== "string") {
    if (id !== undefined) {
      return rpcError(id, -32600, "Invalid Request: method required");
    }
    return null;
  }

  const isNotification = id === undefined || id === null;

  try {
    if (isNotification) {
      await handleNotification(session, method, params || {});
      return null;
    }
    const result = await handleRequest(session, id, method, params || {});
    // Spec: server SHOULD NOT respond to a request cancelled via notifications/cancelled.
    if (result === CANCELLED_NO_RESPONSE) return null;
    return { jsonrpc: "2.0", id, result };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const code = err && typeof err === "object" && "code" in err
      ? /** @type {any} */ (err).code
      : -32000;
    session.logger.write("error", { method, message, request_id: id ?? null });
    if (isNotification) return null;
    return rpcError(id, code, message);
  }
}

/**
 * @param {Session} session
 * @param {string} method
 * @param {object} params
 */
async function handleNotification(session, method, params) {
  switch (method) {
    case "notifications/initialized":
    case "initialized":
      session.initialized = true;
      session.logger.write("initialized_notification", {});
      break;
    case "notifications/cancelled": {
      const requestId = params.requestId ?? params.request_id;
      session.logger.write("cancelled", {
        request_id: requestId ?? null,
        reason: params.reason ?? null,
      });
      if (requestId !== undefined && requestId !== null) {
        abortCall(session, requestId, "client_cancelled");
      }
      break;
    }
    default:
      session.logger.write("unknown_notification", { method, params });
  }
}

/**
 * @param {Session} session
 * @param {string|number} id
 * @param {string} method
 * @param {object} params
 */
async function handleRequest(session, id, method, params) {
  switch (method) {
    case "initialize":
      return doInitialize(session, params);
    case "ping":
      return {};
    case "tools/list":
      return { tools: toolDefs() };
    case "tools/call":
      return doToolsCall(session, id, params);
    case "resources/list":
      return { resources: [] };
    case "prompts/list":
      return { prompts: [] };
    default: {
      const err = new Error(`Method not found: ${method}`);
      /** @type {any} */ (err).code = -32601;
      throw err;
    }
  }
}

/**
 * @param {Session} session
 * @param {object} params
 */
function doInitialize(session, params) {
  const clientInfo = params.clientInfo ?? {};
  const capabilities = params.capabilities ?? {};
  const requested = params.protocolVersion || LATEST_PROTOCOL_VERSION;
  const protocolVersion = negotiateProtocolVersion(requested);

  session.clientInfo = clientInfo;
  session.capabilities = capabilities;
  session.protocolVersion = protocolVersion;
  session.initialized = true;

  session.logger.write("initialize", {
    clientInfo,
    capabilities,
    protocolVersion,
    requestedProtocolVersion: requested,
    negotiated_fallback: protocolVersion !== requested,
  });

  return {
    protocolVersion,
    capabilities: {
      tools: { listChanged: false },
      logging: {},
    },
    serverInfo: {
      name: SERVER_NAME,
      version: SERVER_VERSION,
      title: "MCP Probe (client-matrix harness)",
    },
    instructions:
      "Probe server for measuring MCP client timeouts, progress, cancel, and reconnect. " +
      "Call slow_echo(delay_ms, progress?) and inspect JSONL logs under the configured log dir.",
  };
}

/**
 * @param {Session} session
 * @param {string|number} requestId
 * @param {object} params
 */
async function doToolsCall(session, requestId, params) {
  const name = params.name;
  const args = params.arguments ?? {};
  const meta = params._meta ?? {};
  const progressToken =
    meta.progressToken ?? meta.progress_token ?? null;

  if (typeof name !== "string") {
    const err = new Error("tools/call: name required");
    /** @type {any} */ (err).code = -32602;
    throw err;
  }

  if (name === "ping") {
    const started = Date.now();
    session.logger.write("tools_call_start", {
      request_id: requestId,
      tool: "ping",
      args,
      progress_token: null,
    });
    const result = toolText(
      JSON.stringify({
        ok: true,
        tool: "ping",
        note: args.note ?? null,
        session_id: session.sessionId,
        ts: new Date().toISOString(),
      }),
    );
    session.logger.write("tools_call_end", {
      request_id: requestId,
      tool: "ping",
      status: "ok",
      duration_ms: Date.now() - started,
      delay_ms: 0,
      progress_token_present: false,
      progress_notifications_sent: 0,
    });
    return result;
  }

  if (name === "slow_echo") {
    return runSlowEcho(session, requestId, args, progressToken);
  }

  return toolText(`Unknown tool: ${name}`, true);
}

/**
 * @param {Session} session
 * @param {string|number} requestId
 * @param {object} args
 * @param {string|number|null} progressToken
 */
function runSlowEcho(session, requestId, args, progressToken) {
  let delayMs = Number(args.delay_ms);
  if (!Number.isFinite(delayMs) || delayMs < 0) delayMs = 0;
  if (delayMs > 600_000) delayMs = 600_000;
  const message = typeof args.message === "string" ? args.message : "echo";
  const wantProgress = Boolean(args.progress);
  const progressTokenPresent = progressToken !== null && progressToken !== undefined;

  if (wantProgress && progressTokenPresent) {
    session.logger.write("progress_token_present", {
      request_id: requestId,
      progress_token: progressToken,
    });
  } else if (wantProgress && !progressTokenPresent) {
    session.logger.write("progress_token_missing", {
      request_id: requestId,
      note: "progress=true but client did not supply _meta.progressToken",
    });
  }

  const abort = new AbortController();
  /** @type {ActiveCall} */
  const call = {
    requestId,
    tool: "slow_echo",
    startedAt: Date.now(),
    delayMs,
    progress: wantProgress,
    progressToken: progressTokenPresent ? progressToken : null,
    progressSent: 0,
    abort,
    abortReason: null,
    timer: null,
    progressTimer: null,
    finished: false,
  };
  session.activeCalls.set(requestId, call);

  session.logger.write("tools_call_start", {
    request_id: requestId,
    tool: "slow_echo",
    args: { delay_ms: delayMs, message, progress: wantProgress },
    progress_token: call.progressToken,
    progress_token_present: progressTokenPresent,
  });

  return new Promise((resolve, reject) => {
    const finish = (status, resultOrErr) => {
      if (call.finished) return;
      call.finished = true;
      clearTimers(call);
      session.activeCalls.delete(requestId);
      const duration_ms = Date.now() - call.startedAt;
      session.logger.write("tools_call_end", {
        request_id: requestId,
        tool: "slow_echo",
        status,
        duration_ms,
        delay_ms: delayMs,
        progress_token_present: progressTokenPresent,
        progress_notifications_sent: call.progressSent,
        abort_reason: call.abortReason,
      });
      if (status === "ok") resolve(resultOrErr);
      else if (status === "cancelled_silent") {
        // MCP: SHOULD NOT send a response for a cancelled request — resolve sentinel.
        resolve(CANCELLED_NO_RESPONSE);
      } else if (status === "aborted_silent") {
        resolve(CANCELLED_NO_RESPONSE);
      } else reject(resultOrErr);
    };

    abort.signal.addEventListener("abort", () => {
      if (call.abortReason === "client_cancelled") {
        finish("cancelled_silent", null);
      } else {
        finish("aborted_silent", null);
      }
    });

    const sendProgress = (forceProgress) => {
      if (abort.signal.aborted || session.closed) return;
      const elapsed = Date.now() - call.startedAt;
      const progress =
        forceProgress != null
          ? forceProgress
          : Math.min(0.99, elapsed / Math.max(delayMs, 1));
      call.progressSent += 1;
      const notification = {
        jsonrpc: "2.0",
        method: "notifications/progress",
        params: {
          progressToken,
          progress,
          total: 1,
          message: `slow_echo waiting ${elapsed}ms / ${delayMs}ms`,
        },
      };
      session.logger.write("progress_sent", {
        request_id: requestId,
        progress,
        progress_token: progressToken,
        n: call.progressSent,
      });
      Promise.resolve(session.send(notification)).catch(() => {
        // send failures observed via connection_close
      });
    };

    if (wantProgress && progressTokenPresent) {
      // Immediate heartbeat so short delays still prove progress support.
      sendProgress(0);
      const heartbeatMs = Math.min(
        10_000,
        Math.max(40, Math.floor(delayMs / 5) || 40),
      );
      call.progressTimer = setInterval(() => sendProgress(), heartbeatMs);
    }

    call.timer = setTimeout(() => {
      if (abort.signal.aborted) return;
      finish(
        "ok",
        toolText(
          JSON.stringify({
            ok: true,
            tool: "slow_echo",
            message,
            delay_ms: delayMs,
            waited_ms: Date.now() - call.startedAt,
            progress_requested: wantProgress,
            progress_token_present: progressTokenPresent,
            progress_notifications_sent: call.progressSent,
            session_id: session.sessionId,
            ts: new Date().toISOString(),
          }),
        ),
      );
    }, delayMs);
  });
}

/**
 * @param {Session} session
 * @param {string|number} requestId
 * @param {string} reason
 */
export function abortCall(session, requestId, reason) {
  const call = session.activeCalls.get(requestId);
  if (!call) return false;
  call.abortReason = reason;
  session.logger.write("call_aborted", {
    request_id: requestId,
    tool: call.tool,
    reason,
    elapsed_ms: Date.now() - call.startedAt,
  });
  call.abort.abort();
  return true;
}

/**
 * Mark connection closed; abort in-flight calls and log active ones as disconnects.
 * @param {Session} session
 * @param {string} reason
 */
export function onConnectionClose(session, reason) {
  if (session.closed) return;
  session.closed = true;
  const active = [...session.activeCalls.values()].map((c) => ({
    request_id: c.requestId,
    tool: c.tool,
    elapsed_ms: Date.now() - c.startedAt,
    delay_ms: c.delayMs,
  }));
  for (const c of active) {
    session.logger.write("timeout_observed", {
      request_id: c.request_id,
      tool: c.tool,
      elapsed_s: Math.round(c.elapsed_ms / 1000),
      elapsed_ms: c.elapsed_ms,
      expected_delay_ms: c.delay_ms,
      note: "connection closed while call in flight (client timeout or drop)",
    });
  }
  session.logger.write("connection_close", {
    reason,
    closed_at: new Date().toISOString(),
    active_calls: active,
  });
  for (const call of [...session.activeCalls.values()]) {
    call.abortReason = reason;
    clearTimers(call);
    call.abort.abort();
  }
  session.activeCalls.clear();
  session.logger.close(reason);
}

/** @param {ActiveCall} call */
function clearTimers(call) {
  if (call.timer) {
    clearTimeout(call.timer);
    call.timer = null;
  }
  if (call.progressTimer) {
    clearInterval(call.progressTimer);
    call.progressTimer = null;
  }
}

/** @param {string} text @param {boolean} [isError] */
export function toolText(text, isError = false) {
  return {
    content: [{ type: "text", text }],
    isError: Boolean(isError),
  };
}

/** @param {string|number|null} id @param {number} code @param {string} message */
export function rpcError(id, code, message) {
  return {
    jsonrpc: "2.0",
    id,
    error: { code, message },
  };
}

/**
 * Parse a single JSON-RPC line / body. Supports one object or a batch array.
 * @param {string} raw
 */
export function parseRpc(raw) {
  const trimmed = raw.trim();
  if (!trimmed) return [];
  const parsed = JSON.parse(trimmed);
  return Array.isArray(parsed) ? parsed : [parsed];
}
