#!/usr/bin/env node
/**
 * Run slow_echo across a delay matrix and emit JSON + markdown report.
 *
 * Default delays (seconds): 5 15 25 30 45 60 120
 * Override: MCP_PROBE_DELAYS="0.05,0.1" (seconds, comma-separated)
 *           or --delays 0.05,0.1
 *
 * Also runs a progress=true variant for each delay (when tokens enabled).
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { StdioFakeClient } from "../src/fake-client.mjs";
import { readJsonl, summarizeEvents } from "../src/logger.mjs";
import { generateReport } from "./report.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const DEFAULT_DELAYS_S = [5, 15, 25, 30, 45, 60, 120];

/**
 * Parse delay list (seconds).
 * - `undefined` / `null` → default matrix (when caller did not supply an override).
 * - empty string, non-numeric, negative, or zero-length result → throws (CLI exits non-zero).
 *
 * @param {string|undefined|null} raw
 * @returns {number[]}
 */
export function parseDelays(raw) {
  if (raw === undefined || raw === null) {
    return [...DEFAULT_DELAYS_S];
  }
  const trimmed = String(raw).trim();
  if (!trimmed) {
    throw new Error("MCP_PROBE_DELAYS/delays is empty");
  }
  const parts = trimmed.split(/[,\s]+/).map((s) => s.trim()).filter(Boolean);
  if (parts.length === 0) {
    throw new Error("MCP_PROBE_DELAYS/delays produced zero rows");
  }
  /** @type {number[]} */
  const delays = [];
  for (const part of parts) {
    const n = Number(part);
    if (!Number.isFinite(n) || n < 0) {
      throw new Error(`invalid delay value: ${part}`);
    }
    delays.push(n);
  }
  if (delays.length === 0) {
    throw new Error("MCP_PROBE_DELAYS/delays produced zero rows");
  }
  return delays;
}

/**
 * @param {object} opts
 * @param {number[]} [opts.delaysS]
 * @param {string} [opts.logDir]
 * @param {string} [opts.outDir]
 * @param {boolean} [opts.withProgress]
 * @param {number} [opts.clientTimeoutMs] — if set, client aborts after this many ms
 * @param {boolean} [opts.progressToken]
 */
export async function runMatrix(opts = {}) {
  const delaysS =
    opts.delaysS ||
    (process.env.MCP_PROBE_DELAYS !== undefined
      ? parseDelays(process.env.MCP_PROBE_DELAYS)
      : parseDelays(undefined));
  if (!delaysS.length) {
    throw new Error("run-matrix: zero delay rows");
  }
  const outDir = path.resolve(opts.outDir || path.join(__dirname, "../logs/matrix"));
  const logDir = path.resolve(opts.logDir || path.join(outDir, "jsonl"));
  fs.mkdirSync(logDir, { recursive: true });
  fs.mkdirSync(outDir, { recursive: true });

  const withProgress = opts.withProgress !== false;
  const rows = [];
  const startedAt = new Date().toISOString();

  for (const delayS of delaysS) {
    const variants = withProgress ? [false, true] : [false];
    for (const progress of variants) {
      const label = `matrix-${delayS}s-${progress ? "prog" : "plain"}`;
      const client = new StdioFakeClient({
        logDir,
        clientLabel: label,
        clientName: "mcp-probe-run-matrix",
        withProgressToken: opts.progressToken !== false,
      });
      const row = {
        delay_s: delayS,
        delay_ms: Math.round(delayS * 1000),
        progress,
        status: "unknown",
        duration_ms: null,
        timeout_s: null,
        progress_token_present: false,
        progress_notifications: 0,
        cancelled: false,
        log_file: null,
        error: null,
      };
      const t0 = Date.now();
      try {
        await client.start();
        const timeoutMs =
          opts.clientTimeoutMs != null
            ? opts.clientTimeoutMs
            : undefined;
        await client.slowEcho({
          delayMs: row.delay_ms,
          progress,
          message: `matrix-${delayS}`,
          timeoutMs,
        });
        row.status = "ok";
        row.duration_ms = Date.now() - t0;
        const progressNotes = client.notifications.filter(
          (n) => n.method === "notifications/progress",
        );
        row.progress_notifications = progressNotes.length;
      } catch (err) {
        row.duration_ms = Date.now() - t0;
        if (err && err.code === "CLIENT_TIMEOUT") {
          row.status = "client_timeout";
          row.timeout_s = Math.round(row.duration_ms / 1000);
          row.cancelled = true;
        } else {
          row.status = "error";
          row.error = err instanceof Error ? err.message : String(err);
        }
      } finally {
        await client.close();
      }

      // Find newest matching log
      const files = fs
        .readdirSync(logDir)
        .filter((f) => f.startsWith(label) && f.endsWith(".jsonl"))
        .map((f) => ({
          f,
          m: fs.statSync(path.join(logDir, f)).mtimeMs,
        }))
        .sort((a, b) => b.m - a.m);
      if (files[0]) {
        row.log_file = path.join(logDir, files[0].f);
        const summary = summarizeEvents(readJsonl(row.log_file));
        row.progress_token_present = summary.progress_token_supported;
        if (summary.timeout_seconds.length) {
          row.timeout_s = summary.timeout_seconds[0];
        }
        const call = summary.calls.find((c) => c.tool === "slow_echo");
        if (call) {
          row.progress_notifications = Math.max(
            row.progress_notifications,
            call.progress_notifications_sent,
          );
        }
        if (summary.cancelled_count > 0) row.cancelled = true;
      }
      rows.push(row);
    }
  }

  const result = {
    meta: {
      started_at: startedAt,
      finished_at: new Date().toISOString(),
      delays_s: delaysS,
      transport: "stdio",
      harness: "mcp-probe/scripts/run-matrix.mjs",
    },
    rows,
  };

  const matrixPath = path.join(outDir, "matrix-results.json");
  fs.writeFileSync(matrixPath, JSON.stringify(result, null, 2), "utf8");
  const reportPath = path.join(outDir, "matrix-report.md");
  generateReport({
    logDir,
    matrixPath,
    outPath: reportPath,
  });

  return { result, matrixPath, reportPath, logDir };
}

function parseCli(argv) {
  /** @type {Record<string, string|boolean|null>} */
  const out = {
    delays: null,
    "log-dir": "",
    "out-dir": path.resolve(__dirname, "../logs/matrix"),
    "client-timeout-ms": "",
    "no-progress": false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--no-progress") {
      out["no-progress"] = true;
      continue;
    }
    if (a.startsWith("--") && argv[i + 1] && !argv[i + 1].startsWith("--")) {
      out[a.slice(2)] = argv[++i];
    }
  }
  return out;
}

const isMain =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (isMain) {
  const args = parseCli(process.argv.slice(2));
  try {
    let delaysS;
    if (args.delays !== null && args.delays !== undefined) {
      delaysS = parseDelays(String(args.delays));
    } else if (process.env.MCP_PROBE_DELAYS !== undefined) {
      delaysS = parseDelays(process.env.MCP_PROBE_DELAYS);
    } else {
      delaysS = parseDelays(undefined);
    }
    if (!delaysS.length) {
      throw new Error("run-matrix: zero delay rows");
    }
    const { result, matrixPath, reportPath } = await runMatrix({
      delaysS,
      outDir: String(args["out-dir"]),
      logDir: args["log-dir"] ? String(args["log-dir"]) : undefined,
      withProgress: !args["no-progress"],
      clientTimeoutMs: args["client-timeout-ms"]
        ? Number(args["client-timeout-ms"])
        : undefined,
    });
    process.stderr.write(
      `[mcp-probe] matrix rows=${result.rows.length} json=${matrixPath} report=${reportPath}\n`,
    );
    if (!result.rows.length) {
      process.stderr.write("[mcp-probe] zero result rows\n");
      process.exitCode = 1;
    }
    const failed = result.rows.filter((r) => r.status === "error");
    if (failed.length) {
      process.stderr.write(`[mcp-probe] ${failed.length} row(s) errored\n`);
      process.exitCode = 1;
    }
  } catch (err) {
    process.stderr.write(
      `[mcp-probe] ${err instanceof Error ? err.message : String(err)}\n`,
    );
    process.exit(1);
  }
}
