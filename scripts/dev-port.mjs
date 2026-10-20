/**
 * Single source of truth for the Next.js / Tauri frontend dev port.
 * Consumed by `scripts/next-dev.mjs` and `src/lib/dev-asset-prefix.ts` (via next.config).
 */
import net from "node:net";

/** Default when PORT is unset/empty — shared by resolver and assetPrefix. */
export const DEFAULT_DEV_PORT = "3000";

/** Host Next/assetPrefix use when TAURI_DEV_HOST is unset (matches next.config.ts). */
export const DEFAULT_DEV_HOST = "localhost";

export class InvalidDevPortError extends Error {
  /** @param {unknown} raw */
  constructor(raw) {
    super(
      `Invalid PORT ${JSON.stringify(String(raw))}: must be an integer from 1 to 65535`,
    );
    this.name = "InvalidDevPortError";
  }
}

export class PortInUseError extends Error {
  /**
   * @param {string|number} port
   * @param {string} host
   */
  constructor(port, host) {
    super(
      `Port ${port} is already in use on ${host}. Set PORT=<free-port> (1–65535) and retry — next-dev will not pick another port.`,
    );
    this.name = "PortInUseError";
    this.port = String(port);
    this.host = host;
  }
}

/**
 * @param {NodeJS.ProcessEnv | Record<string, string | undefined>} [env]
 * @returns {string}
 */
export function resolveDevHost(env = process.env) {
  const raw = env.TAURI_DEV_HOST;
  if (raw === undefined || raw === "") {
    return DEFAULT_DEV_HOST;
  }
  const trimmed = String(raw).trim();
  return trimmed === "" ? DEFAULT_DEV_HOST : trimmed;
}

/**
 * @param {NodeJS.ProcessEnv | Record<string, string | undefined>} [env]
 * @returns {string}
 */
export function resolveDevPort(env = process.env) {
  const raw = env.PORT;
  if (raw === undefined || raw === "") {
    return DEFAULT_DEV_PORT;
  }
  const trimmed = String(raw).trim();
  if (trimmed === "") {
    return DEFAULT_DEV_PORT;
  }
  // Reject floats, signs, hex, etc. — digits only, then range-check.
  if (!/^\d+$/.test(trimmed)) {
    throw new InvalidDevPortError(raw);
  }
  const n = Number(trimmed);
  if (!Number.isInteger(n) || n < 1 || n > 65535) {
    throw new InvalidDevPortError(raw);
  }
  return String(n);
}

/**
 * Pre-flight: bind the resolved port on the same host Next will use.
 * On EADDRINUSE, throws PortInUseError — never falls back to another port.
 *
 * @param {string|number} port
 * @param {string} [host]
 * @returns {Promise<void>}
 */
export function assertDevPortFree(port, host = DEFAULT_DEV_HOST) {
  const portNum = Number(port);
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", (err) => {
      if (err && /** @type {NodeJS.ErrnoException} */ (err).code === "EADDRINUSE") {
        reject(new PortInUseError(portNum, host));
        return;
      }
      reject(err);
    });
    server.once("listening", () => {
      server.close((closeErr) => {
        if (closeErr) {
          reject(closeErr);
          return;
        }
        resolve();
      });
    });
    server.listen(portNum, host);
  });
}

/**
 * Argv for `next` after the binary: always pins `-p <port>`, then forwards extras.
 * @param {string} port
 * @param {string[]} [extraArgs] process.argv.slice(2) from next-dev.mjs
 * @returns {string[]}
 */
export function buildNextDevArgs(port, extraArgs = []) {
  return ["dev", "-p", port, ...extraArgs];
}
