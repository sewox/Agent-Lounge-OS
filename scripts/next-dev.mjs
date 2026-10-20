/**
 * Cross-platform `next dev` pinned to PORT (default 3000).
 * Works on macOS, Windows, and Linux without shell-specific ${PORT:-3000}.
 * Extra CLI args after the script are forwarded to `next dev`.
 * Pre-flight bind check fails loudly if the port is busy (no fallback port).
 */
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  assertDevPortFree,
  buildNextDevArgs,
  resolveDevHost,
  resolveDevPort,
} from "./dev-port.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function fail(err) {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
}

let port;
try {
  port = resolveDevPort(process.env);
} catch (err) {
  fail(err);
}

const host = resolveDevHost(process.env);

try {
  await assertDevPortFree(port, host);
} catch (err) {
  fail(err);
}

const extraArgs = process.argv.slice(2);
const require = createRequire(import.meta.url);
const nextBin = require.resolve("next/dist/bin/next");
const nextArgs = buildNextDevArgs(port, extraArgs);

const child = spawn(process.execPath, [nextBin, ...nextArgs], {
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
