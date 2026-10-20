/**
 * Dev `assetPrefix` for Next ↔ Tauri.
 * Port resolution lives only in `scripts/dev-port.mjs` (imported below).
 */
export {
  assertDevPortFree,
  buildNextDevArgs,
  DEFAULT_DEV_HOST,
  DEFAULT_DEV_PORT,
  InvalidDevPortError,
  PortInUseError,
  resolveDevHost,
  resolveDevPort,
} from "../../scripts/dev-port.mjs";

import { DEFAULT_DEV_HOST, DEFAULT_DEV_PORT } from "../../scripts/dev-port.mjs";

export function resolveDevAssetPrefix(opts: {
  isProd: boolean;
  host?: string;
  port?: string;
}): string | undefined {
  if (opts.isProd) {
    return undefined;
  }
  const host = opts.host || DEFAULT_DEV_HOST;
  const port = opts.port || DEFAULT_DEV_PORT;
  return `http://${host}:${port}`;
}
