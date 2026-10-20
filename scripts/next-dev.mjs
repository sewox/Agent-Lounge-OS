/**
 * Cross-platform `next dev` pinned to PORT (default 3000).
 * Works on macOS, Windows, and Linux without shell-specific ${PORT:-3000}.
 */
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveDevPort } from "./dev-port.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const port = resolveDevPort(process.env);
const require = createRequire(import.meta.url);
const nextBin = require.resolve("next/dist/bin/next");

const child = spawn(process.execPath, [nextBin, "dev", "-p", port], {
  cwd: root,
  stdio: "inherit",
  env: process.env,
});

child.on("error", (err) => {
  console.error(err);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
