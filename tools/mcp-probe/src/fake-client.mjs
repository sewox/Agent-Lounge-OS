import { spawn } from "node:child_process";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const PROBE_BIN = path.resolve(__dirname, "../bin/mcp-probe.mjs");

/**
 * Minimal stdio MCP client for tests and run-matrix.
 */
export class StdioFakeClient {
  /**
   * @param {object} opts
   * @param {string} opts.logDir
   * @param {string} [opts.clientLabel]
   * @param {string} [opts.clientName]
   * @param {string} [opts.clientVersion]
   * @param {object} [opts.capabilities]
   * @param {boolean} [opts.withProgressToken]
   */
  constructor(opts) {
    this.logDir = opts.logDir;
    this.clientLabel = opts.clientLabel || "fake-client";
    this.clientName = opts.clientName || "mcp-probe-fake-client";
    this.clientVersion = opts.clientVersion || "0.0.1";
    this.capabilities = opts.capabilities || { roots: { listChanged: false } };
    this.withProgressToken = opts.withProgressToken !== false;
    this.proc = null;
    this._nextId = 1;
    this._pending = new Map();
    this._notifications = [];
    this._rl = null;
  }

  async start() {
    this.proc = spawn(process.execPath, [PROBE_BIN, "--transport", "stdio",
      "--log-dir", this.logDir,
      "--client-label", this.clientLabel,
    ], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env },
    });

    this._rl = readline.createInterface({
      input: this.proc.stdout,
      crlfDelay: Infinity,
    });
    this._rl.on("line", (line) => {
      if (!line.trim()) return;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        return;
      }
      if (msg.id !== undefined && msg.id !== null && this._pending.has(msg.id)) {
        const { resolve, reject } = this._pending.get(msg.id);
        this._pending.delete(msg.id);
        if (msg.error) reject(Object.assign(new Error(msg.error.message), { rpc: msg.error }));
        else resolve(msg.result);
      } else if (msg.method) {
        this._notifications.push(msg);
      }
    });

    this.proc.stderr?.on("data", () => {
      /* probe diagnostics — ignore in client */
    });

    await this.request("initialize", {
      protocolVersion: "2024-11-05",
      capabilities: this.capabilities,
      clientInfo: { name: this.clientName, version: this.clientVersion },
    });
    this.notify("notifications/initialized", {});
  }

  /** @param {string} method @param {object} params */
  request(method, params = {}, opts = {}) {
    const id = this._nextId++;
    const msg = { jsonrpc: "2.0", id, method, params };
    if (opts.meta) msg.params = { ...params, _meta: opts.meta };
    return new Promise((resolve, reject) => {
      const timer = opts.timeoutMs
        ? setTimeout(() => {
            this._pending.delete(id);
            this.notify("notifications/cancelled", {
              requestId: id,
              reason: "client_timeout",
            });
            reject(Object.assign(new Error(`client timeout after ${opts.timeoutMs}ms`), {
              code: "CLIENT_TIMEOUT",
              requestId: id,
              elapsed_ms: opts.timeoutMs,
            }));
          }, opts.timeoutMs)
        : null;
      this._pending.set(id, {
        resolve: (v) => {
          if (timer) clearTimeout(timer);
          resolve(v);
        },
        reject: (e) => {
          if (timer) clearTimeout(timer);
          reject(e);
        },
      });
      this.proc.stdin.write(`${JSON.stringify(msg)}\n`);
    });
  }

  /** @param {string} method @param {object} params */
  notify(method, params = {}) {
    this.proc.stdin.write(
      `${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`,
    );
  }

  async ping(note) {
    return this.request("tools/call", {
      name: "ping",
      arguments: note ? { note } : {},
    });
  }

  /**
   * @param {object} opts
   * @param {number} opts.delayMs
   * @param {boolean} [opts.progress]
   * @param {string} [opts.message]
   * @param {number} [opts.timeoutMs]
   */
  async slowEcho(opts) {
    const args = {
      name: "slow_echo",
      arguments: {
        delay_ms: opts.delayMs,
        progress: Boolean(opts.progress),
        message: opts.message || "echo",
      },
    };
    const reqOpts = { timeoutMs: opts.timeoutMs };
    if (this.withProgressToken && opts.progress) {
      reqOpts.meta = { progressToken: `pt-${this._nextId}` };
    }
    return this.request("tools/call", args, reqOpts);
  }

  get notifications() {
    return this._notifications;
  }

  async close() {
    if (!this.proc) return;
    try {
      this.proc.stdin.end();
    } catch {
      /* ignore */
    }
    await new Promise((resolve) => {
      const t = setTimeout(() => {
        this.proc.kill("SIGTERM");
        resolve();
      }, 2000);
      this.proc.once("exit", () => {
        clearTimeout(t);
        resolve();
      });
    });
    this._rl?.close();
    this.proc = null;
  }
}

/**
 * Minimal Streamable HTTP MCP client for tests.
 */
export class HttpFakeClient {
  /**
   * @param {object} opts
   * @param {string} opts.baseUrl  e.g. http://127.0.0.1:19891/mcp
   * @param {string} [opts.clientName]
   * @param {boolean} [opts.withProgressToken]
   */
  constructor(opts) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.clientName = opts.clientName || "mcp-probe-http-fake";
    this.withProgressToken = opts.withProgressToken !== false;
    this.sessionId = null;
    this._nextId = 1;
    this.notifications = [];
  }

  async initialize() {
    const result = await this.rpc("initialize", {
      protocolVersion: "2025-03-26",
      capabilities: {},
      clientInfo: { name: this.clientName, version: "0.0.1" },
    });
    await this.notify("notifications/initialized", {});
    return result;
  }

  async rpc(method, params = {}, opts = {}) {
    const id = this._nextId++;
    const body = {
      jsonrpc: "2.0",
      id,
      method,
      params: opts.meta ? { ...params, _meta: opts.meta } : params,
    };
    const headers = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
    };
    if (this.sessionId) headers["mcp-session-id"] = this.sessionId;

    const ac = new AbortController();
    const timer = opts.timeoutMs
      ? setTimeout(() => ac.abort(), opts.timeoutMs)
      : null;

    const onTimeout = async () => {
      await this.notify("notifications/cancelled", {
        requestId: id,
        reason: "client_timeout",
      }).catch(() => {});
      throw Object.assign(new Error(`client timeout after ${opts.timeoutMs}ms`), {
        code: "CLIENT_TIMEOUT",
        requestId: id,
      });
    };

    try {
      const res = await fetch(this.baseUrl, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
        signal: ac.signal,
      });

      const sid = res.headers.get("mcp-session-id");
      if (sid) this.sessionId = sid;

      const ct = res.headers.get("content-type") || "";
      if (ct.includes("text/event-stream")) {
        // Keep abort linked through body read (headers alone must not clear timeout).
        const text = await res.text();
        let result = null;
        for (const block of text.split("\n\n")) {
          const dataLine = block.split("\n").find((l) => l.startsWith("data: "));
          if (!dataLine) continue;
          const msg = JSON.parse(dataLine.slice(6));
          if (msg.method) this.notifications.push(msg);
          if (msg.id === id) {
            if (msg.error) {
              throw Object.assign(new Error(msg.error.message), { rpc: msg.error });
            }
            result = msg.result;
          }
        }
        return result;
      }

      const msg = await res.json();
      if (msg.error) {
        throw Object.assign(new Error(msg.error.message), { rpc: msg.error });
      }
      return msg.result;
    } catch (err) {
      if (ac.signal.aborted || (err && err.name === "AbortError")) {
        return onTimeout();
      }
      throw err;
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  async notify(method, params = {}) {
    const headers = {
      "content-type": "application/json",
      accept: "application/json",
    };
    if (this.sessionId) headers["mcp-session-id"] = this.sessionId;
    await fetch(this.baseUrl, {
      method: "POST",
      headers,
      body: JSON.stringify({ jsonrpc: "2.0", method, params }),
    });
  }

  async ping() {
    return this.rpc("tools/call", { name: "ping", arguments: {} });
  }

  async slowEcho(opts) {
    const params = {
      name: "slow_echo",
      arguments: {
        delay_ms: opts.delayMs,
        progress: Boolean(opts.progress),
        message: opts.message || "echo",
      },
    };
    const reqOpts = { timeoutMs: opts.timeoutMs };
    if (this.withProgressToken && opts.progress) {
      reqOpts.meta = { progressToken: `pt-http-${this._nextId}` };
    }
    return this.rpc("tools/call", params, reqOpts);
  }

  async close() {
    if (!this.sessionId) return;
    await fetch(this.baseUrl, {
      method: "DELETE",
      headers: { "mcp-session-id": this.sessionId },
    }).catch(() => {});
  }
}
