/**
 * Single source of truth for the Next.js / Tauri frontend dev port.
 * Consumed by `scripts/next-dev.mjs` and `src/lib/dev-asset-prefix.ts` (via next.config).
 */

export class InvalidDevPortError extends Error {
  /** @param {unknown} raw */
  constructor(raw) {
    super(
      `Invalid PORT ${JSON.stringify(String(raw))}: must be an integer from 1 to 65535`,
    );
    this.name = "InvalidDevPortError";
  }
}

/**
 * @param {NodeJS.ProcessEnv | Record<string, string | undefined>} [env]
 * @returns {string}
 */
export function resolveDevPort(env = process.env) {
  const raw = env.PORT;
  if (raw === undefined || raw === "") {
    return "3000";
  }
  const trimmed = String(raw).trim();
  if (trimmed === "") {
    return "3000";
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
 * Argv for `next` after the binary: always pins `-p <port>`, then forwards extras.
 * @param {string} port
 * @param {string[]} [extraArgs] process.argv.slice(2) from next-dev.mjs
 * @returns {string[]}
 */
export function buildNextDevArgs(port, extraArgs = []) {
  return ["dev", "-p", port, ...extraArgs];
}
