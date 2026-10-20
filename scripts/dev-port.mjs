/**
 * Single source of truth for the Next.js / Tauri frontend dev port.
 * Consumed by `scripts/next-dev.mjs` and `src/lib/dev-asset-prefix.ts` (via next.config).
 */
import net from "node:net";

/** Default when PORT is unset/empty — shared by resolver and assetPrefix. */
export const DEFAULT_DEV_PORT = "3000";

/** Host assetPrefix uses when TAURI_DEV_HOST is unset (matches next.config.ts). */
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
  /** @param {string|number} port */
  constructor(port) {
    super(
      `Port ${port} is already in use. Set PORT=<free-port> (1–65535) and retry — next-dev will not pick another port.`,
    );
    this.name = "PortInUseError";
    this.port = String(port);
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
 * @param {number} portNum
 * @param {string|undefined} host undefined = no-host listen (Next default)
 * @param {typeof net.createServer} createServer
 * @returns {Promise<void>}
 */
function tryBindOnce(portNum, host, createServer) {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", (err) => {
      if (err && /** @type {NodeJS.ErrnoException} */ (err).code === "EADDRINUSE") {
        reject(new PortInUseError(portNum));
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
    if (host === undefined) {
      // Match `next dev -p` without `-H` (every address).
      server.listen(portNum);
    } else {
      server.listen(portNum, host);
    }
  });
}

/**
 * Pre-flight: prove the port is free the way Next binds (no host), and also
 * that IPv4 / IPv6 loopback are free. On macOS/Windows, exclusivity is not
 * symmetric — a localhost-only holder can leave an all-address probe looking
 * free (and the reverse), so we check both styles. Never falls back to
 * another port; never adds `-H` to Next.
 *
 * @param {string|number} port
 * @param {{ createServer?: typeof net.createServer }} [opts]
 * @returns {Promise<void>}
 */
export async function assertDevPortFree(port, opts = {}) {
  const portNum = Number(port);
  const createServer = opts.createServer ?? net.createServer;

  await tryBindOnce(portNum, undefined, createServer);
  await tryBindOnce(portNum, "127.0.0.1", createServer);

  if (await canBindIpv6Loopback({ createServer })) {
    await tryBindOnce(portNum, "::1", createServer);
  }
}

/**
 * Deterministic probe: can this process bind IPv6 loopback (`::1`)?
 * Used by tests and by assertDevPortFree — not a skip gate.
 *
 * @param {{ createServer?: typeof net.createServer }} [opts]
 * @returns {Promise<boolean>}
 */
export function canBindIpv6Loopback(opts = {}) {
  const createServer = opts.createServer ?? net.createServer;
  return new Promise((resolve) => {
    const server = createServer();
    server.once("error", () => resolve(false));
    server.once("listening", () => {
      server.close(() => resolve(true));
    });
    server.listen(0, "::1");
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
