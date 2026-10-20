/**
 * Single source for the Next.js / Tauri frontend dev port.
 * Read by `scripts/next-dev.mjs` and mirrored in `next.config.ts` via PORT.
 */
export function resolveDevPort(env = process.env) {
  const raw = env.PORT;
  if (raw === undefined || raw === "") {
    return "3000";
  }
  return String(raw);
}
