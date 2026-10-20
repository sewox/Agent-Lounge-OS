/**
 * Dev `assetPrefix` for Next ↔ Tauri.
 * Port resolution lives only in `scripts/dev-port.mjs` (imported below).
 */
export {
  buildNextDevArgs,
  InvalidDevPortError,
  resolveDevPort,
} from "../../scripts/dev-port.mjs";

export function resolveDevAssetPrefix(opts: {
  isProd: boolean;
  host?: string;
  port?: string;
}): string | undefined {
  if (opts.isProd) {
    return undefined;
  }
  const host = opts.host || "localhost";
  const port = opts.port || "3000";
  return `http://${host}:${port}`;
}
