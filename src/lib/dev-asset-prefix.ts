/**
 * Dev `assetPrefix` for Next ↔ Tauri. Must track the same PORT as
 * `scripts/next-dev.mjs` / `scripts/dev-port.mjs` (default 3000).
 */
export function resolveDevPort(
  env: NodeJS.ProcessEnv | Record<string, string | undefined> = process.env,
): string {
  const raw = env.PORT;
  if (raw === undefined || raw === "") {
    return "3000";
  }
  return String(raw);
}

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
